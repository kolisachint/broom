//! Keymap: chords to [`Command`]s, per context, with vim leader tables.
//!
//! Chord syntax (strings in `keymap.toml`):
//! - Modifiers joined by `+`: `cmd`, `ctrl`, `alt` (option), `shift`.
//! - Then one key: a letter, digit, punctuation (`=`, `-`, `/`, `?`),
//!   or a name: `space`, `enter`, `backspace`, `tab`, `escape`, `up`, `down`,
//!   `left`, `right`, `f2`.
//! - Chords in a sequence are separated by spaces: `"cmd+k z"`.
//! - Shifted punctuation is written as the character it makes: `"cmd+?"`.
//!
//! Resolution: a [`Context`] is searched first, then `Global`.
//! A key may be a prefix of a longer sequence. [`Keymap::lookup`] reports that
//! as [`KeyMatch::Prefix`] so the UI can wait for the next key. When the wait
//! ends, [`SequenceState`] runs the exact binding for the keys typed so far
//! (for example `Cmd+K` alone becomes InsertLink after 800 ms).

use crate::{Command, ConfigError};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

/// Where a binding applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Context {
    /// Everywhere. Lowest priority. Not seen from [`Context::TextInput`].
    Global,
    /// Focus is in the editor.
    Editor,
    /// Focus is in the file tree.
    Tree,
    /// A text field (rename box, search box). Only TextInput bindings apply,
    /// so Global and Editor keys such as Cmd+B do nothing here and the field
    /// keeps the platform keys.
    TextInput,
}

impl Context {
    /// Parses a context name from `keymap.toml`. Case-insensitive, and `_` and
    /// `-` are ignored, so `"Editor"`, `"text_input"` and `"TextInput"` all work.
    pub fn from_name(name: &str) -> Option<Context> {
        let key = name
            .chars()
            .filter(|c| *c != '_' && *c != '-')
            .collect::<String>()
            .to_ascii_lowercase();
        match key.as_str() {
            "global" => Some(Context::Global),
            "editor" => Some(Context::Editor),
            "tree" => Some(Context::Tree),
            "textinput" => Some(Context::TextInput),
            _ => None,
        }
    }
}

/// One key press: modifiers plus a key name.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Normalized key: lowercase letter, digit, punctuation, or name (`"space"`).
    pub key: String,
}

/// Key names accepted in chords. Each maps to the normalized name in `key`.
const KEY_NAMES: &[(&str, &str)] = &[
    ("space", "space"),
    ("enter", "enter"),
    ("return", "enter"),
    ("backspace", "backspace"),
    ("delete", "delete"),
    ("tab", "tab"),
    ("escape", "escape"),
    ("esc", "escape"),
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("home", "home"),
    ("end", "end"),
    ("pageup", "pageup"),
    ("pagedown", "pagedown"),
];

/// Shifted punctuation and the key it sits on (US layout). `?` is `/` + shift.
const SHIFTED_PUNCTUATION: &[(char, char)] = &[
    ('!', '1'),
    ('@', '2'),
    ('#', '3'),
    ('$', '4'),
    ('%', '5'),
    ('^', '6'),
    ('&', '7'),
    ('*', '8'),
    ('(', '9'),
    (')', '0'),
    ('_', '-'),
    ('+', '='),
    ('{', '['),
    ('}', ']'),
    (':', ';'),
    ('"', '\''),
    ('<', ','),
    ('>', '.'),
    ('?', '/'),
    ('~', '`'),
    ('|', '\\'),
];

/// Normalizes one key token. Returns the key and whether the token implies
/// shift (`?` implies shift on `/`). `None` when the token is not a valid key.
fn normalize_key(token: &str) -> Option<(String, bool)> {
    let lower = token.to_ascii_lowercase();
    if let Some((_, name)) = KEY_NAMES.iter().find(|(n, _)| *n == lower) {
        return Some(((*name).to_string(), false));
    }
    if let Some(num) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
        && (1..=12).contains(&num)
    {
        return Some((format!("f{num}"), false));
    }
    let mut chars = token.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if let Some((_, base)) = SHIFTED_PUNCTUATION.iter().find(|(s, _)| *s == c) {
        return Some((base.to_string(), true));
    }
    if c.is_alphabetic() {
        Some((c.to_lowercase().collect(), false))
    } else if !c.is_whitespace() && !c.is_control() {
        Some((c.to_string(), false))
    } else {
        None
    }
}

/// The shifted character for a base key, for labels: `"/"` gives `?`.
/// Digits are left alone, so `Shift+1` stays `Shift+1`.
fn shifted_glyph(key: &str) -> Option<char> {
    SHIFTED_PUNCTUATION
        .iter()
        .find(|(_, base)| !base.is_ascii_digit() && base.to_string() == key)
        .map(|(shifted, _)| *shifted)
}

impl Chord {
    /// Parses `"cmd+shift+p"`, `"ctrl+?"`, `"f2"`, `"space"`.
    /// Errors with [`ConfigError::InvalidChord`].
    pub fn parse(s: &str) -> Result<Chord, ConfigError> {
        let invalid = || ConfigError::InvalidChord(s.to_string());
        let text = s.trim();
        if text.is_empty() || text.contains(char::is_whitespace) {
            return Err(invalid());
        }

        // The key may itself be "+", as in "cmd++" or "+".
        let (mods, key) = if text == "+" {
            ("", "+")
        } else if let Some(head) = text.strip_suffix("++") {
            (head, "+")
        } else {
            text.rsplit_once('+').unwrap_or(("", text))
        };

        let (key, implied_shift) = normalize_key(key).ok_or_else(invalid)?;

        let mut chord = Chord {
            cmd: false,
            ctrl: false,
            alt: false,
            shift: implied_shift,
            key,
        };
        if !mods.is_empty() {
            for token in mods.split('+') {
                let flag = match token.to_ascii_lowercase().as_str() {
                    "cmd" | "command" | "meta" => &mut chord.cmd,
                    "ctrl" | "control" => &mut chord.ctrl,
                    "alt" | "option" | "opt" => &mut chord.alt,
                    "shift" => &mut chord.shift,
                    _ => return Err(invalid()),
                };
                if *flag {
                    // Same modifier twice is a typo, not a chord.
                    return Err(invalid());
                }
                *flag = true;
            }
        }
        Ok(chord)
    }

    /// Builds a chord from a key the UI read, plus modifier flags. `key` is the
    /// base key (`"/"` with shift held) or the character it made (`"?"`). Both
    /// give the same chord. `None` for an unknown key.
    pub fn from_keystroke(
        key: &str,
        cmd: bool,
        ctrl: bool,
        alt: bool,
        shift: bool,
    ) -> Option<Chord> {
        let (key, implied_shift) = normalize_key(key)?;
        Some(Chord {
            cmd,
            ctrl,
            alt,
            shift: shift || implied_shift,
            key,
        })
    }

    /// Human-readable label for the cheatsheet, for example `"Cmd+Shift+P"`.
    /// Shift on punctuation reads as the character it makes: `Ctrl+Shift+/` is `"Ctrl+?"`.
    pub fn label(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.cmd {
            parts.push("Cmd".to_string());
        }
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        match shifted_glyph(&self.key) {
            Some(glyph) if self.shift => parts.push(glyph.to_string()),
            _ => {
                if self.shift {
                    parts.push("Shift".to_string());
                }
                parts.push(key_label(&self.key));
            }
        }
        parts.join("+")
    }
}

/// Label for a normalized key name.
fn key_label(key: &str) -> String {
    match key {
        "space" => "Space".to_string(),
        "enter" => "Enter".to_string(),
        "backspace" => "Backspace".to_string(),
        "delete" => "Delete".to_string(),
        "tab" => "Tab".to_string(),
        "escape" => "Esc".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "pageup" => "Page Up".to_string(),
        "pagedown" => "Page Down".to_string(),
        k if k.starts_with('f') && k[1..].parse::<u8>().is_ok() => k.to_ascii_uppercase(),
        k => k.to_uppercase(),
    }
}

/// Parses a chord sequence such as `"cmd+k z"` into its chords.
/// Errors with [`ConfigError::InvalidChord`] on any bad chord or an empty string.
pub fn parse_sequence(s: &str) -> Result<Vec<Chord>, ConfigError> {
    let chords = s
        .split_whitespace()
        .map(Chord::parse)
        .collect::<Result<Vec<_>, _>>()?;
    if chords.is_empty() {
        return Err(ConfigError::InvalidChord(s.to_string()));
    }
    Ok(chords)
}

/// One binding line from the keymap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub context: Context,
    /// One or more chords. More than one means a sequence (`cmd+k` then `z`).
    pub chords: Vec<Chord>,
    pub command: Command,
}

/// Result of looking up a key sequence in a context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyMatch {
    /// The sequence is bound. Run this command.
    Exact(Command),
    /// The sequence is the start of a longer binding. Wait for another key.
    Prefix,
    /// Nothing bound. Let the key fall through to text input.
    None,
}

/// Vim-tree keys: `j` `k` `h` `l` `a` `r` `d` `Space`. They are not in
/// [`Keymap::bindings`] because `d` or `a` must not act when vim is off.
/// Active only when vim is enabled. See [`vim_tree_command`].
pub const VIM_TREE_KEYS: &[(&str, Command)] = &[
    ("j", Command::TreeDown),
    ("k", Command::TreeUp),
    ("h", Command::TreeCollapse),
    ("l", Command::TreeExpand),
    ("a", Command::TreeNewFile),
    ("r", Command::TreeRename),
    ("d", Command::TreeDelete),
    ("space", Command::TreePreview),
];

/// The command for a vim-tree key (`"j"`, `"space"`), or `None`.
pub fn vim_tree_command(key: &str) -> Option<Command> {
    VIM_TREE_KEYS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, c)| *c)
}

/// Vim leader and sequence tables. Used by `broom-vim`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VimBindings {
    /// `<Space>` + key. Default: f → QuickOpen, e → ToggleTree,
    /// `/` → SearchFolder, o → Outline, p → CommandPalette, z → ZenMode.
    pub leader: BTreeMap<char, Command>,
    /// Normal-mode sequences. Default: `gd` → FollowLink,
    /// `]]` → NextHeading, `[[` → PrevHeading.
    pub sequences: Vec<(String, Command)>,
}

/// All bindings plus the vim tables.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keymap {
    pub bindings: Vec<Binding>,
    pub vim: VimBindings,
}

/// Adds a default binding. The chord strings are constants, checked by tests.
fn bind(bindings: &mut Vec<Binding>, context: Context, keys: &str, command: Command) {
    let chords =
        parse_sequence(keys).unwrap_or_else(|e| panic!("built-in chord `{keys}` is invalid: {e}"));
    bindings.push(Binding {
        context,
        chords,
        command,
    });
}

impl Keymap {
    /// The defaults from SPEC.md, including tree and vim-tree bindings.
    pub fn defaults() -> Keymap {
        use Command::*;
        use Context::{Editor, Global, Tree};

        let mut b = Vec::new();

        // Global (VSCode-style where SPEC is silent).
        bind(&mut b, Global, "cmd+p", QuickOpen);
        bind(&mut b, Global, "cmd+shift+p", CommandPalette);
        bind(&mut b, Global, "cmd+b", ToggleTree);
        bind(&mut b, Global, "cmd+shift+e", FocusTree);
        bind(&mut b, Global, "cmd+shift+f", SearchFolder);
        bind(&mut b, Global, "cmd+shift+o", Outline);
        bind(&mut b, Global, "cmd+/", ToggleSourceMode);
        bind(&mut b, Global, "cmd+alt+v", ToggleVim);
        bind(&mut b, Global, "cmd+=", ZoomIn);
        bind(&mut b, Global, "cmd+-", ZoomOut);
        bind(&mut b, Global, "cmd+0", ZoomReset);
        bind(&mut b, Global, "cmd+s", Save);
        bind(&mut b, Global, "cmd+w", CloseTab);
        bind(&mut b, Global, "cmd+n", NewUntitled);
        bind(&mut b, Global, "cmd+k z", ZenMode);
        bind(&mut b, Global, "ctrl+?", Cheatsheet);
        bind(&mut b, Global, "ctrl+-", Back);

        // Editor. These shadow the Global chords of the same keys.
        bind(&mut b, Editor, "cmd+b", Bold);
        bind(&mut b, Editor, "cmd+i", Italic);
        bind(&mut b, Editor, "cmd+shift+x", Strike);
        bind(&mut b, Editor, "cmd+k", InsertLink);
        bind(&mut b, Editor, "cmd+z", Undo);
        bind(&mut b, Editor, "cmd+shift+z", Redo);
        bind(&mut b, Editor, "cmd+shift+1", Heading1);
        bind(&mut b, Editor, "cmd+shift+2", Heading2);
        bind(&mut b, Editor, "cmd+shift+3", Heading3);
        bind(&mut b, Editor, "cmd+shift+4", Heading4);
        bind(&mut b, Editor, "cmd+shift+5", Heading5);
        bind(&mut b, Editor, "cmd+shift+6", Heading6);

        // File tree.
        bind(&mut b, Tree, "up", TreeUp);
        bind(&mut b, Tree, "down", TreeDown);
        bind(&mut b, Tree, "left", TreeCollapse);
        bind(&mut b, Tree, "right", TreeExpand);
        bind(&mut b, Tree, "enter", TreeOpenPinned);
        bind(&mut b, Tree, "space", TreePreview);
        bind(&mut b, Tree, "cmd+backspace", TreeDelete);
        bind(&mut b, Tree, "f2", TreeRename);

        Keymap {
            bindings: b,
            vim: VimBindings {
                leader: BTreeMap::from([
                    ('f', QuickOpen),
                    ('e', ToggleTree),
                    ('/', SearchFolder),
                    ('o', Outline),
                    ('p', CommandPalette),
                    ('z', ZenMode),
                ]),
                sequences: vec![
                    ("gd".to_string(), FollowLink),
                    ("]]".to_string(), NextHeading),
                    ("[[".to_string(), PrevHeading),
                ],
            },
        }
    }

    /// Reads `path` and applies its `[[bind]]` entries over [`Keymap::defaults`].
    /// A user binding with the same context and chords replaces the default.
    /// `command = "none"` (or `unbind = true`) removes a default instead.
    /// A missing file gives the defaults.
    ///
    /// Strict: the first bad line is an error. For hot reload, keep the
    /// previous keymap on `Err`. [`Keymap::load_lenient`] keeps the good lines.
    pub fn load(path: &Path) -> Result<Keymap, ConfigError> {
        let (keymap, problems) = Keymap::load_lenient(path)?;
        match problems.into_iter().next() {
            Some(first) => Err(first),
            None => Ok(keymap),
        }
    }

    /// Like [`Keymap::load`], but a bad `[[bind]]` line is skipped and returned
    /// in the `Vec` instead of failing. Returns `Err` only when the file cannot
    /// be read or is not valid TOML. On `Err`, keep the previous keymap; do not
    /// fall back to defaults.
    pub fn load_lenient(path: &Path) -> Result<(Keymap, Vec<ConfigError>), ConfigError> {
        let mut keymap = Keymap::defaults();
        let problems = match read_optional(path)? {
            Some(text) => apply_user_text(&mut keymap, &text)?,
            None => Vec::new(),
        };
        Ok((keymap, problems))
    }

    /// Looks up `sequence` in `ctx`, then in `Context::Global`.
    ///
    /// Returns [`KeyMatch::Prefix`] whenever a longer binding (in `ctx` or
    /// `Global`) starts with `sequence`, even if `sequence` is also bound.
    /// This keeps `Cmd+K` waiting for `Z` (ZenMode) while `Cmd+K` alone can
    /// still run InsertLink on timeout. Use [`Keymap::exact`] for that fallback.
    pub fn lookup(&self, ctx: Context, sequence: &[Chord]) -> KeyMatch {
        if sequence.is_empty() {
            return KeyMatch::None;
        }
        let longer = self
            .visible(ctx)
            .any(|b| b.chords.len() > sequence.len() && b.chords[..sequence.len()] == *sequence);
        if longer {
            return KeyMatch::Prefix;
        }
        match self.exact(ctx, sequence) {
            Some(command) => KeyMatch::Exact(command),
            None => KeyMatch::None,
        }
    }

    /// The command bound to exactly `sequence`, searching `ctx` then `Global`.
    /// Ignores longer bindings, so it is the fallback when a prefix times out.
    pub fn exact(&self, ctx: Context, sequence: &[Chord]) -> Option<Command> {
        if sequence.is_empty() {
            return None;
        }
        let search = |wanted: Context| {
            self.bindings
                .iter()
                .find(|b| b.context == wanted && b.chords == sequence)
                .map(|b| b.command)
        };
        let fallback = if ctx == Context::TextInput {
            None
        } else {
            search(Context::Global)
        };
        search(ctx).or(fallback)
    }

    /// Bindings that apply in `ctx`: its own, then `Global` (not in TextInput).
    fn visible(&self, ctx: Context) -> impl Iterator<Item = &Binding> {
        self.bindings.iter().filter(move |b| {
            b.context == ctx || (b.context == Context::Global && ctx != Context::TextInput)
        })
    }
}

/// Reads a file. A missing file is `Ok(None)`, not an error.
pub(crate) fn read_optional(path: &Path) -> Result<Option<String>, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// The shape of `keymap.toml`. Each `[[bind]]` stays a raw value, so one bad
/// line is reported and skipped without losing the others.
#[derive(Deserialize, Default)]
#[serde(default)]
struct KeymapFile {
    bind: Vec<toml::Value>,
}

/// One `[[bind]]` entry. The context is a string, checked by [`Context::from_name`].
#[derive(Deserialize)]
struct BindLine {
    keys: String,
    #[serde(default)]
    command: String,
    #[serde(default = "default_context_name")]
    context: String,
    #[serde(default)]
    unbind: bool,
}

fn default_context_name() -> String {
    "global".to_string()
}

/// Applies the `[[bind]]` entries in `text` over `keymap`.
///
/// Returns `Err` only when the TOML itself is bad (nothing is applied).
/// Bad individual lines are skipped and returned in the `Vec`.
fn apply_user_text(keymap: &mut Keymap, text: &str) -> Result<Vec<ConfigError>, ConfigError> {
    let file: KeymapFile = toml::from_str(text)?;
    let mut problems = Vec::new();
    for value in file.bind {
        match value.try_into::<BindLine>() {
            Ok(line) => {
                if let Err(e) = apply_line(keymap, &line) {
                    problems.push(e);
                }
            }
            Err(e) => problems.push(ConfigError::Parse(e)),
        }
    }
    Ok(problems)
}

/// Applies one bind line. Everything is validated before the keymap changes,
/// so a bad line never removes a default.
fn apply_line(keymap: &mut Keymap, line: &BindLine) -> Result<(), ConfigError> {
    let context = Context::from_name(&line.context)
        .ok_or_else(|| ConfigError::UnknownContext(line.context.clone()))?;
    let chords = parse_sequence(&line.keys)?;
    let command = if line.unbind || line.command == "none" {
        None
    } else {
        Some(Command::from_name(&line.command)?)
    };
    keymap
        .bindings
        .retain(|b| !(b.context == context && b.chords == chords));
    if let Some(command) = command {
        keymap.bindings.push(Binding {
            context,
            chords,
            command,
        });
    }
    Ok(())
}

/// How long a partial sequence waits for its next key (SPEC: 800 ms).
pub const SEQUENCE_TIMEOUT: Duration = Duration::from_millis(800);

/// What one key press did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyOutcome {
    /// The keys completed a binding. Run this command.
    Run(Command),
    /// The keys are the start of a sequence. Wait for the next key or timeout.
    Pending,
    /// Nothing bound. Let the key fall through to text input.
    Unhandled,
}

/// Result of [`SequenceState::press`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyStep {
    /// A pending prefix that was resolved before this key: it timed out, or
    /// this key broke the sequence. Run it before `outcome`.
    pub flushed: Option<Command>,
    /// What this key did.
    pub outcome: KeyOutcome,
}

/// Pure multi-key sequence state machine. Time is passed in, so it is testable.
///
/// Call [`SequenceState::press`] for each key. Call [`SequenceState::tick`]
/// from a timer so a lone prefix such as `Cmd+K` fires on time. Call
/// [`SequenceState::cancel`] when focus moves to a different context.
#[derive(Clone, Debug)]
pub struct SequenceState {
    pending: Vec<Chord>,
    last_key: Option<Instant>,
    timeout: Duration,
}

impl Default for SequenceState {
    fn default() -> Self {
        SequenceState::new()
    }
}

impl SequenceState {
    /// Empty state with the SPEC timeout ([`SEQUENCE_TIMEOUT`]).
    pub fn new() -> SequenceState {
        SequenceState::with_timeout(SEQUENCE_TIMEOUT)
    }

    /// Empty state with a custom timeout.
    pub fn with_timeout(timeout: Duration) -> SequenceState {
        SequenceState {
            pending: Vec::new(),
            last_key: None,
            timeout,
        }
    }

    /// The chords waiting for more keys. Empty when idle.
    pub fn pending(&self) -> &[Chord] {
        &self.pending
    }

    /// Drops any pending prefix without running it.
    pub fn cancel(&mut self) {
        self.pending.clear();
        self.last_key = None;
    }

    /// Runs the pending prefix's exact binding if its timeout has passed at `now`.
    pub fn tick(&mut self, keymap: &Keymap, ctx: Context, now: Instant) -> Option<Command> {
        if self.is_expired(now) {
            self.flush(keymap, ctx)
        } else {
            None
        }
    }

    /// Handles one chord pressed at `now` in `ctx`.
    ///
    /// `is_repeat` is true for OS key-repeat events (a held key). A repeat
    /// never starts or extends a sequence, so holding Cmd+K does not arm
    /// InsertLink. It is ignored while a sequence is pending. Repeats of
    /// single-chord commands (arrow keys in the tree) still run. The UI should
    /// pass the OS repeat flag through unchanged.
    pub fn press(
        &mut self,
        keymap: &Keymap,
        ctx: Context,
        chord: Chord,
        now: Instant,
        is_repeat: bool,
    ) -> KeyStep {
        if is_repeat {
            let starts_sequence = matches!(
                keymap.lookup(ctx, std::slice::from_ref(&chord)),
                KeyMatch::Prefix
            );
            if !self.pending.is_empty() || starts_sequence {
                return KeyStep {
                    flushed: None,
                    outcome: KeyOutcome::Unhandled,
                };
            }
        }

        let mut flushed = None;
        if self.is_expired(now) {
            flushed = self.flush(keymap, ctx);
        }

        if !self.pending.is_empty() {
            let mut candidate = self.pending.clone();
            candidate.push(chord.clone());
            match keymap.lookup(ctx, &candidate) {
                KeyMatch::Exact(command) => {
                    self.cancel();
                    return KeyStep {
                        flushed,
                        outcome: KeyOutcome::Run(command),
                    };
                }
                KeyMatch::Prefix => {
                    self.pending = candidate;
                    self.last_key = Some(now);
                    return KeyStep {
                        flushed,
                        outcome: KeyOutcome::Pending,
                    };
                }
                KeyMatch::None => {
                    // The key breaks the sequence. Run what was pending, then
                    // treat this key as a fresh start.
                    flushed = self.flush(keymap, ctx);
                }
            }
        }

        let outcome = match keymap.lookup(ctx, std::slice::from_ref(&chord)) {
            KeyMatch::Exact(command) => KeyOutcome::Run(command),
            KeyMatch::Prefix => {
                self.pending = vec![chord];
                self.last_key = Some(now);
                KeyOutcome::Pending
            }
            KeyMatch::None => KeyOutcome::Unhandled,
        };
        KeyStep { flushed, outcome }
    }

    fn is_expired(&self, now: Instant) -> bool {
        match self.last_key {
            Some(last) if !self.pending.is_empty() => {
                now.saturating_duration_since(last) >= self.timeout
            }
            _ => false,
        }
    }

    /// Clears the pending prefix and returns its exact binding, if any.
    fn flush(&mut self, keymap: &Keymap, ctx: Context) -> Option<Command> {
        if self.pending.is_empty() {
            return None;
        }
        let command = keymap.exact(ctx, &self.pending);
        self.cancel();
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(s: &str) -> Chord {
        Chord::parse(s).expect("test chord")
    }

    fn seq(s: &str) -> Vec<Chord> {
        parse_sequence(s).expect("test sequence")
    }

    // ---- chord parsing ----

    #[test]
    fn parses_modifiers_and_letter() {
        let c = chord("cmd+shift+p");
        assert!(c.cmd && c.shift && !c.ctrl && !c.alt);
        assert_eq!(c.key, "p");
    }

    #[test]
    fn letters_are_case_insensitive() {
        assert_eq!(chord("Cmd+Shift+P"), chord("cmd+shift+p"));
        assert_eq!(chord("CMD+K").key, "k");
    }

    #[test]
    fn parses_aliases() {
        assert!(chord("command+k").cmd);
        assert!(chord("control+k").ctrl);
        assert!(chord("option+k").alt);
        assert!(chord("opt+k").alt);
    }

    #[test]
    fn ctrl_question_mark_is_ctrl_shift_slash() {
        let c = chord("ctrl+?");
        assert!(c.ctrl);
        assert!(c.shift);
        assert_eq!(c.key, "/");
        assert_eq!(c, chord("ctrl+shift+/"));
    }

    #[test]
    fn shifted_punctuation_canonicalizes_to_base_key() {
        assert_eq!(chord("cmd++"), chord("cmd+shift+="));
        assert_eq!(chord("cmd+_"), chord("cmd+shift+-"));
        assert_eq!(chord("cmd+{"), chord("cmd+shift+["));
        assert_eq!(chord("cmd+:"), chord("cmd+shift+;"));
        assert_eq!(chord("cmd+\""), chord("cmd+shift+'"));
        assert_eq!(chord("cmd+|"), chord("cmd+shift+\\"));
        assert_eq!(chord("cmd+<"), chord("cmd+shift+,"));
        assert_eq!(chord("cmd+~"), chord("cmd+shift+`"));
        assert_eq!(chord("cmd+!"), chord("cmd+shift+1"));
        assert_eq!(chord("+"), chord("shift+="));
    }

    #[test]
    fn from_keystroke_accepts_base_key_or_produced_character() {
        let want = chord("ctrl+shift+/");
        assert_eq!(
            Chord::from_keystroke("/", false, true, false, true),
            Some(want.clone())
        );
        assert_eq!(
            Chord::from_keystroke("?", false, true, false, false),
            Some(want)
        );
        assert_eq!(
            Chord::from_keystroke("1", true, false, false, true),
            Some(chord("cmd+shift+1"))
        );
        assert_eq!(Chord::from_keystroke("", true, false, false, false), None);
        assert_eq!(Chord::from_keystroke("ab", true, false, false, false), None);
    }

    #[test]
    fn shifted_punctuation_labels_as_the_character() {
        assert_eq!(chord("ctrl+shift+/").label(), "Ctrl+?");
        assert_eq!(chord("cmd+shift+1").label(), "Cmd+Shift+1");
        assert_eq!(chord("cmd+=").label(), "Cmd+=");
    }

    #[test]
    fn parses_named_keys() {
        assert_eq!(chord("space").key, "space");
        assert_eq!(chord("f2").key, "f2");
        assert_eq!(chord("cmd+backspace").key, "backspace");
        assert_eq!(chord("escape").key, "escape");
        assert_eq!(chord("esc").key, "escape");
        assert_eq!(chord("return").key, "enter");
        assert_eq!(chord("up").key, "up");
    }

    #[test]
    fn parses_punctuation_and_digits() {
        assert_eq!(chord("cmd+=").key, "=");
        assert_eq!(chord("cmd+-").key, "-");
        assert_eq!(chord("cmd+/").key, "/");
        assert_eq!(chord("cmd+shift+1").key, "1");
        assert_eq!(chord("cmd++").key, "=");
        assert_eq!(chord("+").key, "=");
    }

    #[test]
    fn rejects_bad_chords() {
        for bad in [
            "",
            "   ",
            "cmd+",
            "hyper+x",
            "cmd+cmd+x",
            "ab",
            "cmd+ab",
            "f13",
            "foo",
            "cmd p",
        ] {
            assert!(
                matches!(Chord::parse(bad), Err(ConfigError::InvalidChord(_))),
                "`{bad}` should be invalid"
            );
        }
    }

    #[test]
    fn parse_sequence_splits_on_spaces() {
        let s = seq("cmd+k z");
        assert_eq!(s, vec![chord("cmd+k"), chord("z")]);
        assert!(parse_sequence("   ").is_err());
        assert!(parse_sequence("cmd+k bogus!!").is_err());
    }

    #[test]
    fn labels_read_naturally() {
        assert_eq!(chord("cmd+shift+p").label(), "Cmd+Shift+P");
        assert_eq!(chord("ctrl+?").label(), "Ctrl+?");
        assert_eq!(chord("f2").label(), "F2");
        assert_eq!(chord("space").label(), "Space");
        assert_eq!(chord("cmd+backspace").label(), "Cmd+Backspace");
    }

    // ---- defaults ----

    #[test]
    fn every_default_chord_parses() {
        // Keymap::defaults panics on a bad built-in chord, so this must not panic.
        let km = Keymap::defaults();
        assert!(!km.bindings.is_empty());
        assert!(km.bindings.iter().all(|b| !b.chords.is_empty()));
    }

    #[test]
    fn defaults_match_spec_global_table() {
        let km = Keymap::defaults();
        let check = |ctx: Context, keys: &str, cmd: Command| {
            assert_eq!(
                km.lookup(ctx, &seq(keys)),
                KeyMatch::Exact(cmd),
                "{keys} in {ctx:?}"
            );
        };
        check(Context::Global, "cmd+p", Command::QuickOpen);
        check(Context::Global, "cmd+shift+p", Command::CommandPalette);
        check(Context::Global, "cmd+b", Command::ToggleTree);
        check(Context::Global, "cmd+shift+e", Command::FocusTree);
        check(Context::Global, "cmd+shift+f", Command::SearchFolder);
        check(Context::Global, "cmd+shift+o", Command::Outline);
        check(Context::Global, "cmd+/", Command::ToggleSourceMode);
        check(Context::Global, "cmd+alt+v", Command::ToggleVim);
        check(Context::Global, "cmd+=", Command::ZoomIn);
        check(Context::Global, "cmd+-", Command::ZoomOut);
        check(Context::Global, "cmd+s", Command::Save);
        check(Context::Global, "cmd+w", Command::CloseTab);
        check(Context::Global, "cmd+k z", Command::ZenMode);
        check(Context::Global, "ctrl+?", Command::Cheatsheet);
        check(Context::Global, "ctrl+-", Command::Back);
    }

    #[test]
    fn defaults_match_spec_editor_and_tree_tables() {
        let km = Keymap::defaults();
        let check = |ctx: Context, keys: &str, cmd: Command| {
            assert_eq!(
                km.lookup(ctx, &seq(keys)),
                KeyMatch::Exact(cmd),
                "{keys} in {ctx:?}"
            );
        };
        check(Context::Editor, "cmd+b", Command::Bold);
        check(Context::Editor, "cmd+i", Command::Italic);
        check(Context::Editor, "cmd+shift+x", Command::Strike);
        check(Context::Editor, "cmd+shift+1", Command::Heading1);
        check(Context::Editor, "cmd+shift+6", Command::Heading6);
        check(Context::Tree, "up", Command::TreeUp);
        check(Context::Tree, "down", Command::TreeDown);
        check(Context::Tree, "left", Command::TreeCollapse);
        check(Context::Tree, "right", Command::TreeExpand);
        check(Context::Tree, "enter", Command::TreeOpenPinned);
        check(Context::Tree, "space", Command::TreePreview);
        check(Context::Tree, "cmd+backspace", Command::TreeDelete);
        check(Context::Tree, "f2", Command::TreeRename);
    }

    #[test]
    fn editor_cmd_b_shadows_global_toggle_tree() {
        let km = Keymap::defaults();
        assert_eq!(
            km.lookup(Context::Editor, &seq("cmd+b")),
            KeyMatch::Exact(Command::Bold)
        );
        assert_eq!(
            km.lookup(Context::Tree, &seq("cmd+b")),
            KeyMatch::Exact(Command::ToggleTree)
        );
    }

    #[test]
    fn vim_tree_keys_are_not_plain_bindings() {
        // `d` and `a` must never act when vim is off.
        let km = Keymap::defaults();
        assert_eq!(km.lookup(Context::Tree, &seq("d")), KeyMatch::None);
        assert_eq!(km.lookup(Context::Tree, &seq("j")), KeyMatch::None);
        assert_eq!(vim_tree_command("j"), Some(Command::TreeDown));
        assert_eq!(vim_tree_command("k"), Some(Command::TreeUp));
        assert_eq!(vim_tree_command("h"), Some(Command::TreeCollapse));
        assert_eq!(vim_tree_command("l"), Some(Command::TreeExpand));
        assert_eq!(vim_tree_command("a"), Some(Command::TreeNewFile));
        assert_eq!(vim_tree_command("r"), Some(Command::TreeRename));
        assert_eq!(vim_tree_command("d"), Some(Command::TreeDelete));
        assert_eq!(vim_tree_command("space"), Some(Command::TreePreview));
        assert_eq!(vim_tree_command("q"), None);
    }

    #[test]
    fn default_vim_tables_match_spec() {
        let km = Keymap::defaults();
        assert_eq!(km.vim.leader.get(&'f'), Some(&Command::QuickOpen));
        assert_eq!(km.vim.leader.get(&'e'), Some(&Command::ToggleTree));
        assert_eq!(km.vim.leader.get(&'/'), Some(&Command::SearchFolder));
        assert_eq!(km.vim.leader.get(&'o'), Some(&Command::Outline));
        assert_eq!(km.vim.leader.get(&'p'), Some(&Command::CommandPalette));
        assert_eq!(km.vim.leader.get(&'z'), Some(&Command::ZenMode));
        assert_eq!(km.vim.leader.len(), 6);
        assert!(
            km.vim
                .sequences
                .contains(&("gd".to_string(), Command::FollowLink))
        );
        assert!(
            km.vim
                .sequences
                .contains(&("]]".to_string(), Command::NextHeading))
        );
        assert!(
            km.vim
                .sequences
                .contains(&("[[".to_string(), Command::PrevHeading))
        );
    }

    // ---- lookup ----

    #[test]
    fn cmd_k_is_a_prefix_of_zen_mode() {
        let km = Keymap::defaults();
        assert_eq!(km.lookup(Context::Editor, &seq("cmd+k")), KeyMatch::Prefix);
        assert_eq!(km.lookup(Context::Global, &seq("cmd+k")), KeyMatch::Prefix);
        assert_eq!(
            km.lookup(Context::Editor, &seq("cmd+k z")),
            KeyMatch::Exact(Command::ZenMode)
        );
        assert_eq!(
            km.exact(Context::Editor, &seq("cmd+k")),
            Some(Command::InsertLink)
        );
    }

    #[test]
    fn unbound_and_empty_sequences_are_none() {
        let km = Keymap::defaults();
        assert_eq!(km.lookup(Context::Editor, &seq("x")), KeyMatch::None);
        assert_eq!(km.lookup(Context::Editor, &[]), KeyMatch::None);
        assert_eq!(km.exact(Context::Editor, &[]), None);
    }

    #[test]
    fn context_wins_over_global_and_global_is_fallback() {
        let km = Keymap::defaults();
        // cmd+s only exists globally: visible from the tree too.
        assert_eq!(
            km.lookup(Context::Tree, &seq("cmd+s")),
            KeyMatch::Exact(Command::Save)
        );
        // Editor-only chord is not visible from the tree.
        assert_eq!(km.lookup(Context::Tree, &seq("cmd+i")), KeyMatch::None);
    }

    #[test]
    fn exact_wins_in_same_context_when_no_longer_binding() {
        let km = Keymap::defaults();
        assert_eq!(
            km.lookup(Context::Editor, &seq("cmd+b")),
            KeyMatch::Exact(Command::Bold)
        );
    }

    // ---- user overrides ----

    fn temp_keymap(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("keymap.toml");
        std::fs::write(&path, body).expect("write");
        (dir, path)
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let km = Keymap::load(&dir.path().join("nope.toml")).expect("missing is ok");
        assert_eq!(km, Keymap::defaults());
        let (lenient, problems) =
            Keymap::load_lenient(&dir.path().join("nope.toml")).expect("missing ok");
        assert_eq!(lenient, Keymap::defaults());
        assert!(problems.is_empty());
    }

    #[test]
    fn user_bind_is_added() {
        let (_d, path) = temp_keymap(
            r#"
[[bind]]
keys = "cmd+shift+p"
command = "Outline"
context = "global"
"#,
        );
        let km = Keymap::load(&path).expect("load");
        assert_eq!(
            km.lookup(Context::Global, &seq("cmd+shift+p")),
            KeyMatch::Exact(Command::Outline)
        );
    }

    #[test]
    fn user_bind_replaces_same_context_and_chords() {
        let (_d, path) = temp_keymap(
            r#"
[[bind]]
keys = "cmd+s"
command = "NewUntitled"
context = "global"
"#,
        );
        let km = Keymap::load(&path).expect("load");
        assert_eq!(
            km.lookup(Context::Global, &seq("cmd+s")),
            KeyMatch::Exact(Command::NewUntitled)
        );
        let count = km
            .bindings
            .iter()
            .filter(|b| b.context == Context::Global && b.chords == seq("cmd+s"))
            .count();
        assert_eq!(count, 1, "old binding must be removed, not kept alongside");
    }

    #[test]
    fn user_bind_in_other_context_keeps_default() {
        let (_d, path) = temp_keymap(
            r#"
[[bind]]
keys = "cmd+b"
command = "Italic"
context = "tree"
"#,
        );
        let km = Keymap::load(&path).expect("load");
        assert_eq!(
            km.lookup(Context::Tree, &seq("cmd+b")),
            KeyMatch::Exact(Command::Italic)
        );
        assert_eq!(
            km.lookup(Context::Editor, &seq("cmd+b")),
            KeyMatch::Exact(Command::Bold)
        );
    }

    #[test]
    fn context_defaults_to_global() {
        let (_d, path) = temp_keymap(
            r#"
[[bind]]
keys = "cmd+shift+q"
command = "Save"
"#,
        );
        let km = Keymap::load(&path).expect("load");
        assert_eq!(
            km.lookup(Context::Tree, &seq("cmd+shift+q")),
            KeyMatch::Exact(Command::Save)
        );
    }

    #[test]
    fn user_sequence_bind_works() {
        let (_d, path) = temp_keymap(
            r#"
[[bind]]
keys = "cmd+k o"
command = "Outline"
context = "editor"
"#,
        );
        let km = Keymap::load(&path).expect("load");
        assert_eq!(
            km.lookup(Context::Editor, &seq("cmd+k o")),
            KeyMatch::Exact(Command::Outline)
        );
    }

    #[test]
    fn strict_load_rejects_unknown_command() {
        let (_d, path) = temp_keymap(
            r#"
[[bind]]
keys = "cmd+x"
command = "Explode"
"#,
        );
        assert!(matches!(
            Keymap::load(&path),
            Err(ConfigError::UnknownCommand(ref c)) if c == "Explode"
        ));
    }

    #[test]
    fn strict_load_rejects_bad_chord() {
        let (_d, path) = temp_keymap(
            r#"
[[bind]]
keys = "hyper+x"
command = "Save"
"#,
        );
        assert!(matches!(
            Keymap::load(&path),
            Err(ConfigError::InvalidChord(_))
        ));
    }

    #[test]
    fn strict_load_rejects_bad_toml() {
        let (_d, path) = temp_keymap("[[bind\nkeys =");
        assert!(matches!(Keymap::load(&path), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn lenient_load_keeps_good_lines_and_reports_bad_ones() {
        let (_d, path) = temp_keymap(
            r#"
[[bind]]
keys = "cmd+shift+p"
command = "Outline"

[[bind]]
keys = "hyper+x"
command = "Save"

[[bind]]
keys = "cmd+x"
command = "Explode"
"#,
        );
        let (km, problems) = Keymap::load_lenient(&path).expect("lenient");
        assert_eq!(problems.len(), 2);
        assert!(matches!(problems[0], ConfigError::InvalidChord(_)));
        assert!(matches!(problems[1], ConfigError::UnknownCommand(_)));
        assert_eq!(
            km.lookup(Context::Global, &seq("cmd+shift+p")),
            KeyMatch::Exact(Command::Outline)
        );
    }

    #[test]
    fn lenient_load_of_bad_toml_is_an_error_not_defaults() {
        // Callers keep the previous keymap on Err. Defaults would silently drop user binds.
        let (_d, path) = temp_keymap("this is = = not toml");
        assert!(matches!(
            Keymap::load_lenient(&path),
            Err(ConfigError::Parse(_))
        ));
        assert!(matches!(Keymap::load(&path), Err(ConfigError::Parse(_))));
    }

    // ---- sequence state machine ----

    /// Fixed clock: `t0 + ms`.
    fn at(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    #[test]
    fn single_chord_runs_immediately() {
        let km = Keymap::defaults();
        let mut st = SequenceState::new();
        let t0 = Instant::now();
        let step = st.press(&km, Context::Editor, chord("cmd+b"), t0, false);
        assert_eq!(step.outcome, KeyOutcome::Run(Command::Bold));
        assert_eq!(step.flushed, None);
        assert!(st.pending().is_empty());
    }

    #[test]
    fn unbound_key_is_unhandled() {
        let km = Keymap::defaults();
        let mut st = SequenceState::new();
        let t0 = Instant::now();
        let step = st.press(&km, Context::Editor, chord("x"), t0, false);
        assert_eq!(step.outcome, KeyOutcome::Unhandled);
        assert!(st.pending().is_empty());
    }

    #[test]
    fn cmd_k_then_z_within_timeout_runs_zen_mode() {
        let km = Keymap::defaults();
        let mut st = SequenceState::new();
        let t0 = Instant::now();
        let first = st.press(&km, Context::Editor, chord("cmd+k"), t0, false);
        assert_eq!(first.outcome, KeyOutcome::Pending);
        assert_eq!(st.pending(), &[chord("cmd+k")]);

        let second = st.press(&km, Context::Editor, chord("z"), at(t0, 500), false);
        assert_eq!(second.outcome, KeyOutcome::Run(Command::ZenMode));
        assert_eq!(second.flushed, None);
        assert!(st.pending().is_empty());
    }

    #[test]
    fn cmd_k_alone_times_out_to_insert_link_via_tick() {
        let km = Keymap::defaults();
        let mut st = SequenceState::new();
        let t0 = Instant::now();
        st.press(&km, Context::Editor, chord("cmd+k"), t0, false);

        // Just before 800 ms: still waiting.
        assert_eq!(st.tick(&km, Context::Editor, at(t0, 799)), None);
        assert!(!st.pending().is_empty());

        // At 800 ms: the prefix resolves to its exact binding.
        assert_eq!(
            st.tick(&km, Context::Editor, at(t0, 800)),
            Some(Command::InsertLink)
        );
        assert!(st.pending().is_empty());
        assert_eq!(st.tick(&km, Context::Editor, at(t0, 900)), None);
    }

    #[test]
    fn cmd_k_then_late_key_flushes_insert_link_then_handles_key() {
        let km = Keymap::defaults();
        let mut st = SequenceState::new();
        let t0 = Instant::now();
        st.press(&km, Context::Editor, chord("cmd+k"), t0, false);

        // The timer never ticked. A late `b` is not part of Cmd+K.
        let step = st.press(&km, Context::Editor, chord("b"), at(t0, 900), false);
        assert_eq!(step.flushed, Some(Command::InsertLink));
        assert_eq!(step.outcome, KeyOutcome::Unhandled);
        assert!(st.pending().is_empty());
    }

    #[test]
    fn broken_sequence_flushes_then_starts_fresh_chord() {
        let km = Keymap::defaults();
        let mut st = SequenceState::new();
        let t0 = Instant::now();
        st.press(&km, Context::Editor, chord("cmd+k"), t0, false);

        // Cmd+K then Cmd+B: the sequence breaks. Insert link runs, Bold runs.
        let step = st.press(&km, Context::Editor, chord("cmd+b"), at(t0, 100), false);
        assert_eq!(step.flushed, Some(Command::InsertLink));
        assert_eq!(step.outcome, KeyOutcome::Run(Command::Bold));
    }

    #[test]
    fn late_key_after_timeout_flushes_then_handles_key() {
        let km = Keymap::defaults();
        let mut st = SequenceState::new();
        let t0 = Instant::now();
        st.press(&km, Context::Editor, chord("cmd+k"), t0, false);

        // `z` arrives after the timeout: InsertLink fires, `z` is plain text.
        let step = st.press(&km, Context::Editor, chord("z"), at(t0, 1200), false);
        assert_eq!(step.flushed, Some(Command::InsertLink));
        assert_eq!(step.outcome, KeyOutcome::Unhandled);
    }

    #[test]
    fn timeout_restarts_per_key_in_a_longer_sequence() {
        // Vim-style: each key gets its own 800 ms window.
        let mut km = Keymap::defaults();
        km.bindings.push(Binding {
            context: Context::Global,
            chords: seq("cmd+k a b"),
            command: Command::Outline,
        });
        let mut st = SequenceState::new();
        let t0 = Instant::now();
        assert_eq!(
            st.press(&km, Context::Global, chord("cmd+k"), t0, false)
                .outcome,
            KeyOutcome::Pending
        );
        // 600 ms later: `a` extends the sequence, and the clock restarts.
        assert_eq!(
            st.press(&km, Context::Global, chord("a"), at(t0, 600), false)
                .outcome,
            KeyOutcome::Pending
        );
        // 1 s after the start, but only 600 ms after `a`: still pending.
        assert_eq!(st.tick(&km, Context::Global, at(t0, 1000)), None);
        assert_eq!(
            st.press(&km, Context::Global, chord("b"), at(t0, 1100), false)
                .outcome,
            KeyOutcome::Run(Command::Outline)
        );
    }

    #[test]
    fn cancel_drops_pending_without_running() {
        let km = Keymap::defaults();
        let mut st = SequenceState::new();
        let t0 = Instant::now();
        st.press(&km, Context::Editor, chord("cmd+k"), t0, false);
        st.cancel();
        assert!(st.pending().is_empty());
        assert_eq!(st.tick(&km, Context::Editor, at(t0, 5000)), None);
    }

    #[test]
    fn custom_timeout_is_respected() {
        let km = Keymap::defaults();
        let mut st = SequenceState::with_timeout(Duration::from_millis(100));
        let t0 = Instant::now();
        st.press(&km, Context::Editor, chord("cmd+k"), t0, false);
        assert_eq!(st.tick(&km, Context::Editor, at(t0, 99)), None);
        assert_eq!(
            st.tick(&km, Context::Editor, at(t0, 100)),
            Some(Command::InsertLink)
        );
    }

    #[test]
    fn timeout_constant_is_800_ms() {
        assert_eq!(SEQUENCE_TIMEOUT, Duration::from_millis(800));
    }

    #[test]
    fn tree_context_cmd_k_has_no_exact_fallback() {
        // In the tree, Cmd+K is only the start of Zen mode. Timeout runs nothing.
        let km = Keymap::defaults();
        let mut st = SequenceState::new();
        let t0 = Instant::now();
        assert_eq!(
            st.press(&km, Context::Tree, chord("cmd+k"), t0, false)
                .outcome,
            KeyOutcome::Pending
        );
        assert_eq!(st.tick(&km, Context::Tree, at(t0, 800)), None);
        assert!(st.pending().is_empty());
    }

    // ---- regression: per-line validation, unbinding, text input, repeat ----

    #[test]
    fn context_name_is_case_insensitive() {
        assert_eq!(Context::from_name("Editor"), Some(Context::Editor));
        assert_eq!(Context::from_name("TREE"), Some(Context::Tree));
        assert_eq!(Context::from_name("text_input"), Some(Context::TextInput));
        assert_eq!(Context::from_name("text-input"), Some(Context::TextInput));
        assert_eq!(Context::from_name("nowhere"), None);
    }

    #[test]
    fn capitalized_context_in_file_is_accepted() {
        let (_d, path) = temp_keymap(
            r#"
[[bind]]
keys = "cmd+shift+q"
command = "Outline"
context = "Editor"
"#,
        );
        let km = Keymap::load(&path).expect("Editor is valid");
        assert_eq!(
            km.lookup(Context::Editor, &seq("cmd+shift+q")),
            KeyMatch::Exact(Command::Outline)
        );
    }

    #[test]
    fn one_bad_context_skips_only_that_line() {
        let (_d, path) = temp_keymap(
            r#"
[[bind]]
keys = "cmd+shift+q"
command = "Outline"
context = "Nowhere"

[[bind]]
keys = "cmd+shift+r"
command = "Save"
context = "Global"
"#,
        );
        let (km, problems) = Keymap::load_lenient(&path).expect("file is valid TOML");
        assert_eq!(problems.len(), 1);
        assert!(matches!(problems[0], ConfigError::UnknownContext(ref c) if c == "Nowhere"));
        assert_eq!(
            km.lookup(Context::Global, &seq("cmd+shift+r")),
            KeyMatch::Exact(Command::Save)
        );
        assert_eq!(
            km.lookup(Context::Editor, &seq("cmd+b")),
            KeyMatch::Exact(Command::Bold),
            "defaults survive"
        );
    }

    #[test]
    fn wrong_type_line_is_skipped_and_others_kept() {
        let (_d, path) = temp_keymap(
            r#"
[[bind]]
keys = 5
command = "Save"

[[bind]]
keys = "cmd+shift+r"
command = "Outline"
"#,
        );
        let (km, problems) = Keymap::load_lenient(&path).expect("valid TOML");
        assert_eq!(problems.len(), 1);
        assert!(matches!(problems[0], ConfigError::Parse(_)));
        assert_eq!(
            km.lookup(Context::Global, &seq("cmd+shift+r")),
            KeyMatch::Exact(Command::Outline)
        );
    }

    #[test]
    fn bad_command_does_not_remove_the_default_it_targets() {
        let (_d, path) = temp_keymap(
            r#"
[[bind]]
keys = "cmd+b"
command = "Explode"
context = "global"
"#,
        );
        let (km, problems) = Keymap::load_lenient(&path).expect("valid TOML");
        assert_eq!(problems.len(), 1);
        assert_eq!(
            km.lookup(Context::Tree, &seq("cmd+b")),
            KeyMatch::Exact(Command::ToggleTree),
            "default must survive a line that failed validation"
        );
    }

    #[test]
    fn command_none_unbinds_a_default() {
        let (_d, path) = temp_keymap(
            r#"
[[bind]]
keys = "cmd+b"
command = "none"
context = "global"
"#,
        );
        let km = Keymap::load(&path).expect("load");
        assert_eq!(km.lookup(Context::Tree, &seq("cmd+b")), KeyMatch::None);
        assert_eq!(
            km.lookup(Context::Editor, &seq("cmd+b")),
            KeyMatch::Exact(Command::Bold),
            "only the Global binding is removed"
        );
    }

    #[test]
    fn unbind_true_also_unbinds() {
        let (_d, path) = temp_keymap(
            r#"
[[bind]]
keys = "cmd+s"
unbind = true
"#,
        );
        let km = Keymap::load(&path).expect("load");
        assert_eq!(km.lookup(Context::Global, &seq("cmd+s")), KeyMatch::None);
    }

    #[test]
    fn text_input_ignores_global_and_editor_bindings() {
        let km = Keymap::defaults();
        assert_eq!(km.lookup(Context::TextInput, &seq("cmd+b")), KeyMatch::None);
        assert_eq!(km.lookup(Context::TextInput, &seq("cmd+s")), KeyMatch::None);
        assert_eq!(km.exact(Context::TextInput, &seq("cmd+s")), None);
    }

    #[test]
    fn text_input_sees_its_own_bindings() {
        let (_d, path) = temp_keymap(
            r#"
[[bind]]
keys = "cmd+enter"
command = "Save"
context = "text_input"
"#,
        );
        let km = Keymap::load(&path).expect("load");
        assert_eq!(
            km.lookup(Context::TextInput, &seq("cmd+enter")),
            KeyMatch::Exact(Command::Save)
        );
    }

    #[test]
    fn forward_has_no_default_chord() {
        let km = Keymap::defaults();
        assert_eq!(
            km.lookup(Context::Global, &seq("ctrl+shift+-")),
            KeyMatch::None
        );
        assert!(km.bindings.iter().all(|b| b.command != Command::Forward));
    }

    #[test]
    fn keeps_undo_redo_new_and_zoom_reset_defaults() {
        let km = Keymap::defaults();
        assert_eq!(
            km.lookup(Context::Editor, &seq("cmd+z")),
            KeyMatch::Exact(Command::Undo)
        );
        assert_eq!(
            km.lookup(Context::Editor, &seq("cmd+shift+z")),
            KeyMatch::Exact(Command::Redo)
        );
        assert_eq!(
            km.lookup(Context::Global, &seq("cmd+n")),
            KeyMatch::Exact(Command::NewUntitled)
        );
        assert_eq!(
            km.lookup(Context::Global, &seq("cmd+0")),
            KeyMatch::Exact(Command::ZoomReset)
        );
    }

    #[test]
    fn repeat_does_not_start_a_sequence() {
        let km = Keymap::defaults();
        let mut st = SequenceState::new();
        let t0 = Instant::now();
        let step = st.press(&km, Context::Editor, chord("cmd+k"), t0, true);
        assert_eq!(step.outcome, KeyOutcome::Unhandled);
        assert!(st.pending().is_empty());
        // A real press still starts it.
        assert_eq!(
            st.press(&km, Context::Editor, chord("cmd+k"), t0, false)
                .outcome,
            KeyOutcome::Pending
        );
    }

    #[test]
    fn repeat_while_pending_is_ignored() {
        let km = Keymap::defaults();
        let mut st = SequenceState::new();
        let t0 = Instant::now();
        st.press(&km, Context::Editor, chord("cmd+k"), t0, false);
        let step = st.press(&km, Context::Editor, chord("cmd+k"), at(t0, 100), true);
        assert_eq!(step.flushed, None, "a repeat must not flush InsertLink");
        assert_eq!(step.outcome, KeyOutcome::Unhandled);
        assert_eq!(st.pending(), &[chord("cmd+k")]);
    }

    #[test]
    fn repeat_of_single_chord_command_still_runs() {
        let km = Keymap::defaults();
        let mut st = SequenceState::new();
        let t0 = Instant::now();
        let step = st.press(&km, Context::Tree, chord("down"), t0, true);
        assert_eq!(step.outcome, KeyOutcome::Run(Command::TreeDown));
    }
}
