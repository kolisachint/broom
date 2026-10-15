//! File-system watcher. Wraps `notify` 8.x and forwards events on a channel.
//!
//! Events for paths under a `.git` folder are dropped. Consumers should treat
//! [`WatchEvent::Rescan`] as "anything may have changed, re-list folders".

use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecursiveMode, Watcher as _};
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

/// A simplified file-system event.
#[derive(Clone, Debug, PartialEq, Eq)]
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
        let (tx, rx) = std::sync::mpsc::channel();
        let inner = notify::recommended_watcher(move |result: notify::Result<Event>| {
            send_result(&tx, result);
        })?;
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

// Converts one backend result into events and sends them. Backend errors
// become `Rescan`, since the consumer must re-list anyway.
fn send_result(tx: &Sender<WatchEvent>, result: notify::Result<Event>) {
    match result {
        Ok(event) => {
            for item in map_event(&event) {
                // The receiver may be gone; that just means nobody is listening.
                let _ = tx.send(item);
            }
        }
        Err(_) => {
            let _ = tx.send(WatchEvent::Rescan);
        }
    }
}

// True for any path with a `.git` component.
fn is_git_path(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, Component::Normal(name) if name == ".git"))
}

// Maps one `notify` event to zero or more [`WatchEvent`]s. Paths under `.git`
// are dropped. Access events are dropped.
fn map_event(event: &Event) -> Vec<WatchEvent> {
    let paths: Vec<PathBuf> = event
        .paths
        .iter()
        .filter(|p| !is_git_path(p))
        .cloned()
        .collect();
    if paths.is_empty() {
        // Nothing usable. A bare `Other` is the backend's rescan request.
        return match event.kind {
            EventKind::Other => vec![WatchEvent::Rescan],
            _ => Vec::new(),
        };
    }
    match event.kind {
        EventKind::Create(_) => paths.into_iter().map(WatchEvent::Created).collect(),
        EventKind::Remove(_) => paths.into_iter().map(WatchEvent::Removed).collect(),
        EventKind::Modify(ModifyKind::Name(mode)) => map_rename(mode, paths),
        EventKind::Modify(_) | EventKind::Any => {
            paths.into_iter().map(WatchEvent::Modified).collect()
        }
        EventKind::Access(_) => Vec::new(),
        EventKind::Other => vec![WatchEvent::Rescan],
    }
}

fn map_rename(mode: RenameMode, mut paths: Vec<PathBuf>) -> Vec<WatchEvent> {
    match mode {
        RenameMode::Both if paths.len() == 2 => {
            let to = paths.pop().unwrap_or_default();
            let from = paths.pop().unwrap_or_default();
            vec![WatchEvent::Renamed { from, to }]
        }
        RenameMode::From => paths.into_iter().map(WatchEvent::Removed).collect(),
        RenameMode::To => paths.into_iter().map(WatchEvent::Created).collect(),
        _ => paths
            .into_iter()
            .map(|p| {
                if p.exists() {
                    WatchEvent::Created(p)
                } else {
                    WatchEvent::Removed(p)
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, DataChange, RemoveKind};
    use std::time::{Duration, Instant};

    fn event(kind: EventKind, paths: &[&str]) -> Event {
        let mut ev = Event::new(kind);
        for p in paths {
            ev = ev.add_path(PathBuf::from(p));
        }
        ev
    }

    #[test]
    fn maps_create_modify_remove() {
        let created = map_event(&event(EventKind::Create(CreateKind::File), &["/a/x.md"]));
        assert_eq!(created, vec![WatchEvent::Created(PathBuf::from("/a/x.md"))]);
        let modified = map_event(&event(
            EventKind::Modify(ModifyKind::Data(DataChange::Any)),
            &["/a/x.md"],
        ));
        assert_eq!(modified, vec![WatchEvent::Modified(PathBuf::from("/a/x.md"))]);
        let removed = map_event(&event(EventKind::Remove(RemoveKind::File), &["/a/x.md"]));
        assert_eq!(removed, vec![WatchEvent::Removed(PathBuf::from("/a/x.md"))]);
    }

    #[test]
    fn maps_rename_pair() {
        let ev = event(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &["/a/old.md", "/a/new.md"],
        );
        assert_eq!(
            map_event(&ev),
            vec![WatchEvent::Renamed {
                from: PathBuf::from("/a/old.md"),
                to: PathBuf::from("/a/new.md"),
            }]
        );
    }

    #[test]
    fn drops_git_paths_and_access() {
        let git = event(EventKind::Create(CreateKind::File), &["/r/.git/HEAD"]);
        assert!(map_event(&git).is_empty());
        let access = event(
            EventKind::Access(notify::event::AccessKind::Any),
            &["/r/a.md"],
        );
        assert!(map_event(&access).is_empty());
    }

    #[test]
    fn other_kind_requests_rescan() {
        let ev = Event::new(EventKind::Other);
        assert_eq!(map_event(&ev), vec![WatchEvent::Rescan]);
    }

    #[test]
    fn reports_file_created_under_watched_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (mut watcher, rx) = Watcher::new().unwrap();
        watcher.watch(dir.path(), true).unwrap();
        let target = dir.path().join("made.md");
        std::fs::write(&target, "hi").unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut seen = false;
        while Instant::now() < deadline && !seen {
            if let Ok(ev) = rx.recv_timeout(Duration::from_millis(200)) {
                seen = match ev {
                    WatchEvent::Created(p) | WatchEvent::Modified(p) => p.ends_with("made.md"),
                    WatchEvent::Rescan => true,
                    _ => false,
                };
            }
        }
        assert!(seen, "no event for the new file");
        watcher.unwatch(dir.path()).unwrap();
    }
}
