//! File-system watcher. Wraps `notify` 8.x and forwards events on a channel.
//!
//! Events for paths under a `.git` folder are dropped. Consumers should treat
//! [`WatchEvent::Rescan`] as "anything may have changed, re-list folders".
//!
//! Raw events are coalesced: a batch is sent after [`QUIET`] with no new
//! events, or after [`MAX_BATCH`] at the latest. Within a batch, duplicates
//! are removed and a `Modified` is dropped when the same path was also
//! created or removed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use notify::event::{ModifyKind, RenameMode};
use notify::{EventKind, RecursiveMode, Watcher as _};

use crate::IGNORED_DIR_NAMES;

/// Quiet time after the last raw event before a batch is sent.
const QUIET: Duration = Duration::from_millis(100);

/// Longest a batch is held back while events keep arriving.
const MAX_BATCH: Duration = Duration::from_millis(500);

/// A simplified file-system event.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WatchEvent {
    Created(PathBuf),
    Modified(PathBuf),
    Removed(PathBuf),
    Renamed {
        from: PathBuf,
        to: PathBuf,
    },
    /// Events were lost or the backend asked for a full rescan.
    Rescan,
}

/// Owns the underlying `notify` watcher. Dropping it stops watching.
pub struct Watcher {
    inner: notify::RecommendedWatcher,
}

impl Watcher {
    /// Creates a watcher with no paths yet. Returns it with the receiving end
    /// of the event channel.
    pub fn new() -> notify::Result<(Watcher, Receiver<WatchEvent>)> {
        let (raw_tx, raw_rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let (tx, rx) = mpsc::channel();
        let inner = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            // The receiver is gone only when the Watcher was dropped.
            let _ = raw_tx.send(res);
        })?;
        std::thread::Builder::new()
            .name("broom-fs-watch".into())
            .spawn(move || coalesce_loop(raw_rx, tx))
            .map_err(notify::Error::io)?;
        Ok((Watcher { inner }, rx))
    }

    /// Starts watching `path`. `recursive` = watch all sub-folders too.
    pub fn watch(&mut self, path: &Path, recursive: bool) -> notify::Result<()> {
        let mode = if recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        self.inner.watch(path, mode)
    }

    /// Stops watching `path`.
    pub fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        self.inner.unwatch(path)
    }
}

/// Reads raw events and sends coalesced batches. Exits when the watcher is
/// dropped (raw channel closed) or the receiver is dropped (send fails).
fn coalesce_loop(raw_rx: Receiver<notify::Result<notify::Event>>, tx: Sender<WatchEvent>) {
    while let Ok(first) = raw_rx.recv() {
        let mut batch = Vec::new();
        translate(first, &mut batch);

        let started = Instant::now();
        let mut open = true;
        loop {
            let left = MAX_BATCH.saturating_sub(started.elapsed());
            if left.is_zero() {
                break;
            }
            match raw_rx.recv_timeout(left.min(QUIET)) {
                Ok(next) => translate(next, &mut batch),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    open = false;
                    break;
                }
            }
        }

        for event in coalesce(batch) {
            if tx.send(event).is_err() {
                return;
            }
        }
        if !open {
            return;
        }
    }
}

/// Converts one raw `notify` result into zero or more [`WatchEvent`]s.
fn translate(res: notify::Result<notify::Event>, out: &mut Vec<WatchEvent>) {
    let event = match res {
        Ok(event) => event,
        Err(_) => {
            out.push(WatchEvent::Rescan);
            return;
        }
    };
    if event.need_rescan() {
        out.push(WatchEvent::Rescan);
        return;
    }

    let paths: Vec<PathBuf> = event
        .paths
        .into_iter()
        .filter(|path| !is_git_path(path))
        .collect();
    if paths.is_empty() {
        return;
    }

    match event.kind {
        EventKind::Access(_) => {}
        EventKind::Create(_) => out.extend(paths.into_iter().map(WatchEvent::Created)),
        EventKind::Remove(_) => out.extend(paths.into_iter().map(WatchEvent::Removed)),
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) if paths.len() == 2 => {
            let mut it = paths.into_iter();
            if let (Some(from), Some(to)) = (it.next(), it.next()) {
                out.push(WatchEvent::Renamed { from, to });
            }
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
            out.extend(paths.into_iter().map(WatchEvent::Removed));
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
            out.extend(paths.into_iter().map(WatchEvent::Created));
        }
        // Rename with unknown direction: the path's current state tells us.
        EventKind::Modify(ModifyKind::Name(_)) => {
            for path in paths {
                if path.exists() {
                    out.push(WatchEvent::Created(path));
                } else {
                    out.push(WatchEvent::Removed(path));
                }
            }
        }
        EventKind::Modify(_) | EventKind::Any | EventKind::Other => {
            out.extend(paths.into_iter().map(WatchEvent::Modified));
        }
    }
}

/// True when any component of `path` is an ignored folder such as `.git`.
fn is_git_path(path: &Path) -> bool {
    path.components()
        .any(|c| IGNORED_DIR_NAMES.iter().any(|name| c.as_os_str() == *name))
}

/// Removes exact duplicates and redundant `Modified` events, keeping order.
fn coalesce(batch: Vec<WatchEvent>) -> Vec<WatchEvent> {
    let structural: HashSet<PathBuf> = batch
        .iter()
        .filter_map(|event| match event {
            WatchEvent::Created(path) | WatchEvent::Removed(path) => Some(path.clone()),
            _ => None,
        })
        .collect();

    let mut seen = HashSet::new();
    batch
        .into_iter()
        .filter(|event| {
            if let WatchEvent::Modified(path) = event
                && structural.contains(path)
            {
                return false;
            }
            seen.insert(event.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, RemoveKind};
    use notify::{Event, EventKind};
    use std::fs;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn raw(kind: EventKind, paths: &[&str]) -> notify::Result<Event> {
        let mut event = Event::new(kind);
        for path in paths {
            event = event.add_path(p(path));
        }
        Ok(event)
    }

    fn run(events: Vec<notify::Result<Event>>) -> Vec<WatchEvent> {
        let mut out = Vec::new();
        for event in events {
            translate(event, &mut out);
        }
        out
    }

    #[test]
    fn translates_create_and_remove() {
        let out = run(vec![
            raw(EventKind::Create(CreateKind::File), &["/r/a.md"]),
            raw(EventKind::Remove(RemoveKind::File), &["/r/b.md"]),
        ]);
        assert_eq!(
            out,
            vec![
                WatchEvent::Created(p("/r/a.md")),
                WatchEvent::Removed(p("/r/b.md")),
            ]
        );
    }

    #[test]
    fn translates_data_change_to_modified_and_ignores_access() {
        use notify::event::{AccessKind, DataChange};
        let out = run(vec![
            raw(
                EventKind::Modify(ModifyKind::Data(DataChange::Any)),
                &["/r/a.md"],
            ),
            raw(EventKind::Access(AccessKind::Any), &["/r/a.md"]),
        ]);
        assert_eq!(out, vec![WatchEvent::Modified(p("/r/a.md"))]);
    }

    #[test]
    fn translates_rename_pair() {
        let out = run(vec![raw(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &["/r/old.md", "/r/new.md"],
        )]);
        assert_eq!(
            out,
            vec![WatchEvent::Renamed {
                from: p("/r/old.md"),
                to: p("/r/new.md"),
            }]
        );
    }

    #[test]
    fn translates_one_sided_renames() {
        let out = run(vec![
            raw(
                EventKind::Modify(ModifyKind::Name(RenameMode::From)),
                &["/r/gone.md"],
            ),
            raw(
                EventKind::Modify(ModifyKind::Name(RenameMode::To)),
                &["/r/here.md"],
            ),
        ]);
        assert_eq!(
            out,
            vec![
                WatchEvent::Removed(p("/r/gone.md")),
                WatchEvent::Created(p("/r/here.md")),
            ]
        );
    }

    #[test]
    fn unknown_rename_direction_uses_existence() {
        let dir = tempfile::tempdir().unwrap();
        let present = dir.path().join("present.md");
        fs::write(&present, "").unwrap();
        let missing = dir.path().join("missing.md");
        let out = run(vec![raw(
            EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
            &[present.to_str().unwrap(), missing.to_str().unwrap()],
        )]);
        assert_eq!(
            out,
            vec![
                WatchEvent::Created(present.clone()),
                WatchEvent::Removed(missing.clone()),
            ]
        );
    }

    #[test]
    fn drops_paths_under_git() {
        let out = run(vec![
            raw(EventKind::Create(CreateKind::File), &["/r/.git/HEAD"]),
            raw(EventKind::Modify(ModifyKind::Any), &["/r/.git/index.lock"]),
            raw(EventKind::Create(CreateKind::File), &["/r/x/.git/y"]),
        ]);
        assert!(out.is_empty(), "got {out:?}");
    }

    #[test]
    fn rename_with_one_git_side_is_a_single_path_event() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("index");
        fs::write(&target, "").unwrap();
        let out = run(vec![raw(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &[
                dir.path().join(".git/index.lock").to_str().unwrap(),
                target.to_str().unwrap(),
            ],
        )]);
        assert_eq!(out, vec![WatchEvent::Created(target)]);
    }

    #[test]
    fn rescan_flag_and_errors_become_rescan() {
        let flagged = Event::new(EventKind::Any).set_flag(notify::event::Flag::Rescan);
        let out = run(vec![
            Ok(flagged),
            Err(notify::Error::generic("queue overflow")),
        ]);
        assert_eq!(out, vec![WatchEvent::Rescan, WatchEvent::Rescan]);
    }

    #[test]
    fn coalesce_removes_exact_duplicates_and_keeps_order() {
        let out = coalesce(vec![
            WatchEvent::Created(p("/a")),
            WatchEvent::Created(p("/b")),
            WatchEvent::Created(p("/a")),
            WatchEvent::Rescan,
            WatchEvent::Rescan,
        ]);
        assert_eq!(
            out,
            vec![
                WatchEvent::Created(p("/a")),
                WatchEvent::Created(p("/b")),
                WatchEvent::Rescan,
            ]
        );
    }

    #[test]
    fn coalesce_drops_modified_when_path_also_created_or_removed() {
        let out = coalesce(vec![
            WatchEvent::Created(p("/a")),
            WatchEvent::Modified(p("/a")),
            WatchEvent::Modified(p("/b")),
            WatchEvent::Removed(p("/c")),
            WatchEvent::Modified(p("/c")),
        ]);
        assert_eq!(
            out,
            vec![
                WatchEvent::Created(p("/a")),
                WatchEvent::Modified(p("/b")),
                WatchEvent::Removed(p("/c")),
            ]
        );
    }

    #[test]
    fn watch_receives_create_event_within_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let (mut watcher, rx) = Watcher::new().unwrap();
        watcher.watch(dir.path(), true).unwrap();
        // Let the backend start its stream before the file appears.
        std::thread::sleep(Duration::from_millis(500));

        let file = dir.path().join("created.md");
        fs::write(&file, "hello").unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        let found = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(WatchEvent::Created(path)) if path.ends_with("created.md") => break true,
                Ok(event) => seen.push(event),
                Err(_) => break false,
            }
        };
        assert!(found, "no Created event for created.md; saw {seen:?}");
    }

    #[test]
    fn watch_recursive_sees_nested_file_and_ignores_git() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("nested/.git")).unwrap();
        let (mut watcher, rx) = Watcher::new().unwrap();
        watcher.watch(dir.path(), true).unwrap();
        std::thread::sleep(Duration::from_millis(500));

        fs::write(dir.path().join("nested/.git/HEAD"), "ref").unwrap();
        fs::write(dir.path().join("nested/deep.md"), "x").unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        let found = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(WatchEvent::Created(path)) if path.ends_with("deep.md") => break true,
                Ok(event) => seen.push(event),
                Err(_) => break false,
            }
        };
        assert!(found, "no event for nested/deep.md; saw {seen:?}");
        for event in &seen {
            let paths: Vec<&PathBuf> = match event {
                WatchEvent::Created(p) | WatchEvent::Modified(p) | WatchEvent::Removed(p) => {
                    vec![p]
                }
                WatchEvent::Renamed { from, to } => vec![from, to],
                WatchEvent::Rescan => vec![],
            };
            assert!(
                paths.iter().all(|p| !is_git_path(p)),
                "leaked .git event: {event:?}"
            );
        }
    }

    #[test]
    fn unwatch_stops_events_for_that_path() {
        let dir = tempfile::tempdir().unwrap();
        let (mut watcher, rx) = Watcher::new().unwrap();
        watcher.watch(dir.path(), false).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        watcher.unwatch(dir.path()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        while rx.try_recv().is_ok() {}

        fs::write(dir.path().join("after.md"), "x").unwrap();
        let got = rx.recv_timeout(Duration::from_millis(1500));
        assert!(
            !matches!(&got, Ok(WatchEvent::Created(p)) if p.ends_with("after.md")),
            "event after unwatch: {got:?}"
        );
    }
}
