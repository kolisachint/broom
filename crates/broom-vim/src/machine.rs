//! The modal state machine.
//!
//! Motions: h j k l w b e 0 $ ^ gg G, counts (`3j`), and `]]` / `[[` for headings.
//! Operators: d c y, plus x, p, P, u, Ctrl+r.
//! Visual and V-LINE modes extend the selection with the same motions.
//! Leader (`<Space>`) and sequences come from [`broom_config::VimBindings`]
//! and return [`VimOutcome::Command`].
//!
//! Formatting (bold, headings, ...) is not a vim mode. Those keys go through
//! [`VimOutcome::Command`] and the editor applies them to the document.

use crate::buffer::{Cursor, Selection, TextRange, VimBuffer};
use crate::key::{Key, KeyInput};
use crate::motion::{self, Find, Motion, MotionKind};
use crate::register::{Register, RegisterKind};
use crate::textobj::{self, TextObject};
use broom_config::{Command, VimBindings};

/// Current vim mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
    VisualLine,
}

impl Mode {
    /// Status-bar word. Exactly one of `NORMAL`, `INSERT`, `VISUAL`, `V-LINE`.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "V-LINE",
        }
    }
}

/// What the editor must do after a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VimOutcome {
    /// Vim consumed the key. The buffer, cursor, selection, or mode may have changed.
    Handled,
    /// Vim does not handle this key. In Insert mode the editor types it
    /// (`Document::insert_typed`). In other modes the editor ignores it.
    PassThrough,
    /// An app command (leader keys, `gd`, `]]` / `[[`, `u`, `Ctrl+r`).
    Command(Command),
}

/// Vim state. One per editor pane.
#[derive(Debug)]
pub struct Vim {
    mode: Mode,
    bindings: VimBindings,
    // Pending count, operator, and partial sequence. Private.
    pending: Pending,
    // Unnamed register: last yanked or deleted text.
    register: Register,
    // Last `f` / `F` / `t` / `T` search, for `;` and `,`.
    last_find: Option<Find>,
    // Keys of the current command, for `.`.
    cmd_keys: Vec<KeyInput>,
    // The last change, for `.`.
    last_change: Option<Change>,
    // Keys that entered the current insert session, and the text typed in it.
    insert_entry: Vec<KeyInput>,
    typed: String,
    // True while `.` replays keys. Replayed keys do not overwrite the last change.
    replaying: bool,
}

// Keys typed so far that are not yet a complete command.
#[derive(Debug, Default)]
struct Pending {
    // Count typed before an operator (`2` in `2d3w`).
    op_count: Option<usize>,
    count: Option<usize>,
    operator: Option<char>,
    keys: String,
    awaiting_leader: bool,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.op_count.is_none()
            && self.count.is_none()
            && self.operator.is_none()
            && self.keys.is_empty()
            && !self.awaiting_leader
    }
}

// A change that `.` can repeat.
#[derive(Clone, Debug)]
enum Change {
    Keys(Vec<KeyInput>),
    Insert { entry: Vec<KeyInput>, text: String },
}

// What an operator acts on.
#[derive(Clone, Copy, Debug)]
enum Target {
    Chars(TextRange),
    Lines(usize, usize),
}

impl Vim {
    /// New machine in Normal mode with the given leader and sequence tables.
    pub fn new(bindings: VimBindings) -> Vim {
        Vim {
            mode: Mode::Normal,
            bindings,
            pending: Pending::default(),
            register: Register::new(),
            last_find: None,
            cmd_keys: Vec::new(),
            last_change: None,
            insert_entry: Vec::new(),
            typed: String::new(),
            replaying: false,
        }
    }

    /// Current mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Partial input shown in the status bar, e.g. `"2d"` or `"<Space>"`.
    /// `None` when nothing is pending.
    pub fn pending_display(&self) -> Option<String> {
        if self.pending.awaiting_leader {
            return Some("<Space>".to_string());
        }
        let mut s = String::new();
        if let Some(n) = self.pending.op_count {
            s.push_str(&n.to_string());
        }
        if let Some(op) = self.pending.operator {
            s.push(op);
        }
        if let Some(n) = self.pending.count {
            s.push_str(&n.to_string());
        }
        s.push_str(&self.pending.keys);
        (!s.is_empty()).then_some(s)
    }

    /// Drops any pending count, operator, or sequence and returns to Normal.
    /// Call when vim is toggled off or the editor loses focus.
    pub fn reset(&mut self) {
        self.mode = Mode::Normal;
        self.pending = Pending::default();
    }

    /// Handles one key. Returns what the editor must do next.
    /// Cmd-modified keys always return `PassThrough` (so ⌘S, ⌘B still work).
    pub fn handle_key(&mut self, input: KeyInput, buf: &mut dyn VimBuffer) -> VimOutcome {
        if input.mods.cmd {
            return VimOutcome::PassThrough;
        }
        if self.mode == Mode::Insert {
            return self.insert_key(input, buf);
        }
        if self.mode == Mode::Normal && buf.selection().is_some() {
            buf.set_selection(None);
        }
        if self.mode == Mode::Normal && self.pending.is_empty() {
            self.cmd_keys.clear();
        }
        self.cmd_keys.push(input);
        if input.key == Key::Escape {
            self.pending = Pending::default();
            if self.mode != Mode::Normal {
                self.mode = Mode::Normal;
                buf.set_selection(None);
                buf.set_cursor(clamp_normal(buf, buf.cursor()));
            }
            return VimOutcome::Handled;
        }
        if input.mods.ctrl {
            self.pending = Pending::default();
            return match input.key {
                Key::Char('r') => VimOutcome::Command(Command::Redo),
                _ => VimOutcome::PassThrough,
            };
        }
        match key_char(input.key) {
            Some(c) => self.normal_char(c, buf),
            None => VimOutcome::Handled,
        }
    }
}

// Maps a key to the char vim treats it as in Normal and Visual modes.
fn key_char(key: Key) -> Option<char> {
    Some(match key {
        Key::Char(c) => c,
        Key::Left | Key::Backspace => 'h',
        Key::Right => 'l',
        Key::Up => 'k',
        Key::Down => 'j',
        Key::Home => '0',
        Key::End => '$',
        Key::Delete => 'x',
        Key::Escape | Key::Enter | Key::Tab => return None,
    })
}

// Clamps to a valid Normal-mode cursor: col at most the last char.
fn clamp_normal(buf: &dyn VimBuffer, p: Cursor) -> Cursor {
    let line = p.line.min(buf.line_count().saturating_sub(1));
    let max = buf.line_len(line).saturating_sub(1);
    Cursor {
        line,
        col: p.col.min(max),
    }
}

// Exact match in the sequence table.
fn sequence_command(bindings: &VimBindings, seq: &str) -> Option<Command> {
    bindings
        .sequences
        .iter()
        .find(|(s, _)| s == seq)
        .map(|(_, c)| *c)
}

fn sequence_prefix(bindings: &VimBindings, seq: &str) -> bool {
    bindings
        .sequences
        .iter()
        .any(|(s, _)| s.len() > seq.len() && s.starts_with(seq))
}

impl Vim {
    // Handles one Normal or Visual key, after Escape and Ctrl are dealt with.
    fn normal_char(&mut self, c: char, buf: &mut dyn VimBuffer) -> VimOutcome {
        if self.pending.awaiting_leader {
            self.pending = Pending::default();
            return match self.bindings.leader.get(&c) {
                Some(cmd) => VimOutcome::Command(*cmd),
                None => VimOutcome::Handled,
            };
        }
        if !self.pending.keys.is_empty() {
            let prefix = std::mem::take(&mut self.pending.keys);
            return self.continue_keys(&prefix, c, buf);
        }
        if c == ' ' {
            self.pending.awaiting_leader = true;
            return VimOutcome::Handled;
        }
        let seq = c.to_string();
        if let Some(cmd) = sequence_command(&self.bindings, &seq) {
            self.pending = Pending::default();
            return VimOutcome::Command(cmd);
        }
        if sequence_prefix(&self.bindings, &seq) {
            self.pending.keys = seq;
            return VimOutcome::Handled;
        }
        self.dispatch(c, buf)
    }

    // Second key of a two-key sequence, `g`/`f`/`i`/... prefix.
    fn continue_keys(&mut self, prefix: &str, c: char, buf: &mut dyn VimBuffer) -> VimOutcome {
        let seq = format!("{prefix}{c}");
        if let Some(cmd) = sequence_command(&self.bindings, &seq) {
            self.pending = Pending::default();
            return VimOutcome::Command(cmd);
        }
        if sequence_prefix(&self.bindings, &seq) {
            self.pending.keys = seq;
            return VimOutcome::Handled;
        }
        match prefix {
            "g" if c == 'g' => self.motion(Motion::FileStart, buf),
            "f" | "F" | "t" | "T" => {
                let find = Find {
                    ch: c,
                    forward: prefix == "f" || prefix == "t",
                    till: prefix == "t" || prefix == "T",
                };
                self.last_find = Some(find);
                self.motion(Motion::Find(find), buf)
            }
            "i" | "a" => self.text_object(prefix == "a", c, buf),
            _ => self.cancel(),
        }
    }

    fn dispatch(&mut self, c: char, buf: &mut dyn VimBuffer) -> VimOutcome {
        let visual = self.is_visual();
        let op_pending = self.pending.operator.is_some();
        if c.is_ascii_digit() && (c != '0' || self.pending.count.is_some()) {
            let d = c.to_digit(10).unwrap_or(0) as usize;
            self.pending.count = Some(self.pending.count.unwrap_or(0) * 10 + d);
            return VimOutcome::Handled;
        }
        if visual {
            if let Some(out) = self.visual_op(c, buf) {
                return out;
            }
        }
        if let Some(m) = motion_for(c) {
            return self.motion(m, buf);
        }
        match c {
            'd' | 'c' | 'y' => self.start_operator(c, buf),
            ';' | ',' => match self.last_find {
                Some(find) => self.motion(
                    Motion::RepeatFind {
                        find,
                        reverse: c == ',',
                    },
                    buf,
                ),
                None => self.cancel(),
            },
            'g' | 'f' | 'F' | 't' | 'T' => {
                self.pending.keys = c.to_string();
                VimOutcome::Handled
            }
            'i' | 'a' if op_pending || visual => {
                self.pending.keys = c.to_string();
                VimOutcome::Handled
            }
            _ => self.dispatch_command(c, buf),
        }
    }

    // Commands that are not motions or operators.
    fn dispatch_command(&mut self, c: char, buf: &mut dyn VimBuffer) -> VimOutcome {
        match c {
            'x' => self.op_motion('d', Motion::Right, buf),
            'X' => self.op_motion('d', Motion::Left, buf),
            'D' => self.op_motion('d', Motion::LineEnd, buf),
            'C' => self.op_motion('c', Motion::LineEnd, buf),
            'Y' => self.line_op('y', buf),
            'p' => self.paste(true, buf),
            'P' => self.paste(false, buf),
            'i' => {
                self.enter_insert();
                VimOutcome::Handled
            }
            'a' => {
                let cur = buf.cursor();
                let len = buf.line_len(cur.line);
                buf.set_cursor(Cursor {
                    line: cur.line,
                    col: (cur.col + 1).min(len),
                });
                self.enter_insert();
                VimOutcome::Handled
            }
            'I' => {
                let line = buf.cursor().line;
                buf.set_cursor(Cursor {
                    line,
                    col: motion::first_nonblank(buf, line),
                });
                self.enter_insert();
                VimOutcome::Handled
            }
            'A' => {
                let cur = buf.cursor();
                let len = buf.line_len(cur.line);
                buf.set_cursor(Cursor {
                    line: cur.line,
                    col: len,
                });
                self.enter_insert();
                VimOutcome::Handled
            }
            'o' | 'O' => {
                self.open_line(c == 'O', buf);
                VimOutcome::Handled
            }
            'v' => self.toggle_visual(Mode::Visual, buf),
            'V' => self.toggle_visual(Mode::VisualLine, buf),
            'u' => {
                self.pending = Pending::default();
                VimOutcome::Command(Command::Undo)
            }
            '.' => self.repeat_last(buf),
            _ => self.cancel(),
        }
    }
}

impl Vim {
    fn is_visual(&self) -> bool {
        matches!(self.mode, Mode::Visual | Mode::VisualLine)
    }

    fn cancel(&mut self) -> VimOutcome {
        self.pending = Pending::default();
        VimOutcome::Handled
    }

    // Records the keys of the current command as the last change, for `.`.
    fn record_change(&mut self) {
        if !self.replaying {
            self.last_change = Some(Change::Keys(self.cmd_keys.clone()));
        }
    }

    // Count typed before and after an operator, multiplied. `None` if neither.
    fn take_count(&mut self) -> Option<usize> {
        let before = self.pending.op_count.take();
        let after = self.pending.count.take();
        match (before, after) {
            (Some(a), Some(b)) => Some(a * b),
            (a, b) => a.or(b),
        }
    }

    fn start_operator(&mut self, c: char, buf: &mut dyn VimBuffer) -> VimOutcome {
        if self.pending.operator == Some(c) {
            self.pending.operator = None;
            return self.line_op(c, buf);
        }
        if self.pending.operator.is_some() {
            return self.cancel();
        }
        self.pending.operator = Some(c);
        self.pending.op_count = self.pending.count.take();
        VimOutcome::Handled
    }

    // An operator with a fixed motion (`x` is `dl`).
    fn op_motion(&mut self, op: char, m: Motion, buf: &mut dyn VimBuffer) -> VimOutcome {
        self.pending.operator = Some(op);
        self.motion(m, buf)
    }

    // `dd`, `yy`, `cc`: the operator on `count` lines from the cursor.
    fn line_op(&mut self, op: char, buf: &mut dyn VimBuffer) -> VimOutcome {
        let n = self.take_count().unwrap_or(1);
        let cur = buf.cursor().line;
        let end = (cur + n - 1).min(buf.line_count() - 1);
        self.run_op(op, Target::Lines(cur, end), buf);
        VimOutcome::Handled
    }

    // Runs a motion: moves the cursor, or applies the pending operator.
    fn motion(&mut self, m: Motion, buf: &mut dyn VimBuffer) -> VimOutcome {
        let n = self.take_count();
        let count = match (m, n) {
            (Motion::FileEnd, None) => buf.line_count(),
            _ => n.unwrap_or(1),
        };
        let target = motion::apply(m, count, buf);
        if let Some(op) = self.pending.operator.take() {
            self.apply_motion_operator(op, m, target, buf);
            return VimOutcome::Handled;
        }
        self.pending = Pending::default();
        let p = clamp_normal(buf, target);
        let anchor = buf.selection().map_or(p, |s| s.anchor);
        buf.set_cursor(p);
        if self.is_visual() {
            buf.set_selection(Some(Selection { anchor, head: p }));
        }
        VimOutcome::Handled
    }

    fn apply_motion_operator(
        &mut self,
        op: char,
        m: Motion,
        target: Cursor,
        buf: &mut dyn VimBuffer,
    ) {
        let cur = buf.cursor();
        if m.kind() == MotionKind::Linewise {
            let (a, b) = (cur.line.min(target.line), cur.line.max(target.line));
            self.run_op(op, Target::Lines(a, b), buf);
            return;
        }
        let (start, mut end) = if target < cur {
            (target, cur)
        } else {
            (cur, target)
        };
        if m.kind() == MotionKind::Inclusive {
            end.col += 1;
        } else if end == target && end.col == 0 && end.line > start.line {
            // An exclusive motion ending at column 0 stops at the previous line's end.
            let line = end.line - 1;
            end = Cursor {
                line,
                col: buf.line_len(line),
            };
        }
        end.col = end.col.min(buf.line_len(end.line));
        self.run_op(op, Target::Chars(TextRange { start, end }), buf);
    }

    // Applies `op` (d, c, y) to `target`. Leaves Visual mode.
    fn run_op(&mut self, op: char, target: Target, buf: &mut dyn VimBuffer) {
        self.pending = Pending::default();
        if self.is_visual() {
            self.mode = Mode::Normal;
            buf.set_selection(None);
        }
        match target {
            Target::Lines(a, b) => {
                let text = (a..=b)
                    .map(|l| buf.line(l).unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join("\n");
                self.register.set(text, RegisterKind::Linewise);
                if op == 'y' {
                    let col = buf.cursor().col;
                    let p = clamp_normal(buf, Cursor { line: a, col });
                    buf.set_cursor(p);
                    return;
                }
                if op == 'c' {
                    buf.delete_range(lines_inner(a, b, buf));
                    buf.set_cursor(Cursor { line: a, col: 0 });
                    self.enter_insert();
                    return;
                }
                buf.delete_range(lines_range(a, b, buf));
                let line = a.min(buf.line_count() - 1);
                let col = motion::first_nonblank(buf, line);
                let p = clamp_normal(buf, Cursor { line, col });
                buf.set_cursor(p);
                self.record_change();
            }
            Target::Chars(r) => {
                let text = buf.text_in_range(r);
                self.register.set(text, RegisterKind::Charwise);
                if op == 'y' {
                    let p = clamp_normal(buf, r.start);
                    buf.set_cursor(p);
                    return;
                }
                buf.delete_range(r);
                if op == 'c' {
                    buf.set_cursor(r.start);
                    self.enter_insert();
                } else {
                    let p = clamp_normal(buf, r.start);
                    buf.set_cursor(p);
                    self.record_change();
                }
            }
        }
    }

    // Visual-mode operator keys. `None` for other keys.
    fn visual_op(&mut self, c: char, buf: &mut dyn VimBuffer) -> Option<VimOutcome> {
        let (op, linewise) = match c {
            'd' | 'x' => ('d', false),
            'c' | 's' => ('c', false),
            'y' => ('y', false),
            'D' | 'X' => ('d', true),
            'C' | 'S' => ('c', true),
            'Y' => ('y', true),
            'o' => {
                if let Some(s) = buf.selection() {
                    buf.set_selection(Some(Selection {
                        anchor: s.head,
                        head: s.anchor,
                    }));
                    buf.set_cursor(s.anchor);
                }
                return Some(VimOutcome::Handled);
            }
            _ => return None,
        };
        let target = self.selection_target(linewise, buf);
        self.run_op(op, target, buf);
        Some(VimOutcome::Handled)
    }

    // The visual selection as an operator target.
    fn selection_target(&self, linewise: bool, buf: &dyn VimBuffer) -> Target {
        let cur = buf.cursor();
        let sel = buf.selection().unwrap_or(Selection {
            anchor: cur,
            head: cur,
        });
        let (s, mut e) = if sel.anchor <= sel.head {
            (sel.anchor, sel.head)
        } else {
            (sel.head, sel.anchor)
        };
        if linewise || self.mode == Mode::VisualLine {
            return Target::Lines(s.line, e.line);
        }
        // Visual selections include the char under the head.
        e.col = (e.col + 1).min(buf.line_len(e.line));
        Target::Chars(TextRange { start: s, end: e })
    }

    // `v` / `V`: enter the mode, or leave it when already in it.
    fn toggle_visual(&mut self, to: Mode, buf: &mut dyn VimBuffer) -> VimOutcome {
        self.pending = Pending::default();
        if self.mode == to {
            self.mode = Mode::Normal;
            buf.set_selection(None);
            let p = clamp_normal(buf, buf.cursor());
            buf.set_cursor(p);
            return VimOutcome::Handled;
        }
        let cur = buf.cursor();
        let anchor = if self.is_visual() {
            buf.selection().map_or(cur, |s| s.anchor)
        } else {
            cur
        };
        self.mode = to;
        buf.set_selection(Some(Selection { anchor, head: cur }));
        VimOutcome::Handled
    }

    // `iw`, `a"`, `i(`, ...: with an operator, applies it; in Visual, selects.
    fn text_object(&mut self, around: bool, c: char, buf: &mut dyn VimBuffer) -> VimOutcome {
        let obj = match c {
            'w' => TextObject::Word,
            'p' => TextObject::Paragraph,
            '"' | '\'' => TextObject::Quote(c),
            '(' | ')' | 'b' => TextObject::Pair('('),
            '[' | ']' => TextObject::Pair('['),
            '{' | '}' | 'B' => TextObject::Pair('{'),
            '<' | '>' => TextObject::Pair('<'),
            _ => return self.cancel(),
        };
        let op = self.pending.operator.take();
        self.take_count();
        let Some(r) = textobj::range(obj, around, buf) else {
            return VimOutcome::Handled;
        };
        match op {
            Some(op) => self.run_op(op, Target::Chars(r), buf),
            None if self.is_visual() => {
                let head = Cursor {
                    line: r.end.line,
                    col: r.end.col.saturating_sub(1),
                };
                buf.set_selection(Some(Selection { anchor: r.start, head }));
                buf.set_cursor(head);
            }
            None => {}
        }
        VimOutcome::Handled
    }

    fn enter_insert(&mut self) {
        self.mode = Mode::Insert;
        self.pending = Pending::default();
        self.typed.clear();
        self.insert_entry = self.cmd_keys.clone();
    }

    // `o` / `O`: opens a line and enters Insert on it.
    fn open_line(&mut self, above: bool, buf: &mut dyn VimBuffer) {
        let cur = buf.cursor();
        let at = if above {
            Cursor {
                line: cur.line,
                col: 0,
            }
        } else {
            Cursor {
                line: cur.line,
                col: buf.line_len(cur.line),
            }
        };
        let after = buf.insert_text(at, "\n");
        let line = if above { cur.line } else { after.line };
        buf.set_cursor(Cursor { line, col: 0 });
        self.enter_insert();
    }

    // `p` / `P`. Linewise text opens lines; charwise text goes in the line.
    fn paste(&mut self, after: bool, buf: &mut dyn VimBuffer) -> VimOutcome {
        let n = self.take_count().unwrap_or(1);
        if self.register.is_empty() {
            return VimOutcome::Handled;
        }
        let text = self.register.text().to_string();
        let cur = buf.cursor();
        match self.register.kind() {
            RegisterKind::Linewise => {
                let body = vec![text.as_str(); n].join("\n");
                if after {
                    let at = Cursor {
                        line: cur.line,
                        col: buf.line_len(cur.line),
                    };
                    buf.insert_text(at, &format!("\n{body}"));
                    let line = cur.line + 1;
                    let col = motion::first_nonblank(buf, line);
                    buf.set_cursor(Cursor { line, col });
                } else {
                    let at = Cursor {
                        line: cur.line,
                        col: 0,
                    };
                    buf.insert_text(at, &format!("{body}\n"));
                    let col = motion::first_nonblank(buf, cur.line);
                    buf.set_cursor(Cursor { line: cur.line, col });
                }
            }
            RegisterKind::Charwise => {
                let len = buf.line_len(cur.line);
                let col = if after && len > 0 { cur.col + 1 } else { cur.col };
                let end = buf.insert_text(Cursor { line: cur.line, col }, &text.repeat(n));
                let p = Cursor {
                    line: end.line,
                    col: end.col.saturating_sub(1),
                };
                let p = clamp_normal(buf, p);
                buf.set_cursor(p);
            }
        }
        self.record_change();
        VimOutcome::Handled
    }

    // `.`: repeats the last change.
    fn repeat_last(&mut self, buf: &mut dyn VimBuffer) -> VimOutcome {
        match self.last_change.clone() {
            Some(change) => {
                self.pending = Pending::default();
                self.replay(change, buf);
                VimOutcome::Handled
            }
            None => self.cancel(),
        }
    }

    fn replay(&mut self, change: Change, buf: &mut dyn VimBuffer) {
        self.replaying = true;
        match change {
            Change::Keys(keys) => {
                for k in keys {
                    self.handle_key(k, buf);
                }
            }
            Change::Insert { entry, text } => {
                for k in entry {
                    self.handle_key(k, buf);
                }
                if self.mode == Mode::Insert {
                    let at = buf.cursor();
                    let end = buf.insert_text(at, &text);
                    buf.set_cursor(end);
                    self.insert_escape(buf);
                }
            }
        }
        self.replaying = false;
    }

    // Keys in Insert mode. Printable keys, Enter, Tab, Backspace go to the editor.
    fn insert_key(&mut self, input: KeyInput, buf: &mut dyn VimBuffer) -> VimOutcome {
        match input.key {
            Key::Escape => {
                self.insert_escape(buf);
                VimOutcome::Handled
            }
            Key::Char(c) if !input.mods.ctrl && !input.mods.alt => {
                self.typed.push(c);
                VimOutcome::PassThrough
            }
            Key::Enter => {
                self.typed.push('\n');
                VimOutcome::PassThrough
            }
            Key::Tab => {
                self.typed.push('\t');
                VimOutcome::PassThrough
            }
            Key::Backspace => {
                self.typed.pop();
                VimOutcome::PassThrough
            }
            _ => VimOutcome::PassThrough,
        }
    }

    fn insert_escape(&mut self, buf: &mut dyn VimBuffer) {
        self.mode = Mode::Normal;
        if !self.replaying {
            self.last_change = Some(Change::Insert {
                entry: self.insert_entry.clone(),
                text: self.typed.clone(),
            });
        }
        let c = buf.cursor();
        let p = clamp_normal(
            buf,
            Cursor {
                line: c.line,
                col: c.col.saturating_sub(1),
            },
        );
        buf.set_cursor(p);
    }
}

// The char range that deletes lines `a..=b` with their newline.
fn lines_range(a: usize, b: usize, buf: &dyn VimBuffer) -> TextRange {
    let last = buf.line_count() - 1;
    if b < last {
        TextRange {
            start: Cursor { line: a, col: 0 },
            end: Cursor { line: b + 1, col: 0 },
        }
    } else if a > 0 {
        TextRange {
            start: Cursor {
                line: a - 1,
                col: buf.line_len(a - 1),
            },
            end: Cursor {
                line: b,
                col: buf.line_len(b),
            },
        }
    } else {
        TextRange {
            start: Cursor { line: 0, col: 0 },
            end: Cursor {
                line: b,
                col: buf.line_len(b),
            },
        }
    }
}

// The text of lines `a..=b` without their newlines.
fn lines_inner(a: usize, b: usize, buf: &dyn VimBuffer) -> TextRange {
    TextRange {
        start: Cursor { line: a, col: 0 },
        end: Cursor {
            line: b,
            col: buf.line_len(b),
        },
    }
}

// The motion for a plain key. `None` for keys that are not motions.
fn motion_for(c: char) -> Option<Motion> {
    Some(match c {
        'h' => Motion::Left,
        'l' => Motion::Right,
        'j' => Motion::Down,
        'k' => Motion::Up,
        'w' => Motion::WordForward { big: false },
        'W' => Motion::WordForward { big: true },
        'b' => Motion::WordBack { big: false },
        'B' => Motion::WordBack { big: true },
        'e' => Motion::WordEnd { big: false },
        'E' => Motion::WordEnd { big: true },
        '0' => Motion::LineStart,
        '^' => Motion::FirstNonBlank,
        '$' => Motion::LineEnd,
        'G' => Motion::FileEnd,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockBuffer;
    use std::collections::BTreeMap;

    fn bindings() -> VimBindings {
        let leader = [
            ('f', Command::QuickOpen),
            ('e', Command::ToggleTree),
            ('/', Command::SearchFolder),
            ('o', Command::Outline),
            ('p', Command::CommandPalette),
            ('z', Command::ZenMode),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        VimBindings {
            leader,
            sequences: vec![
                ("gd".to_string(), Command::FollowLink),
                ("]]".to_string(), Command::NextHeading),
                ("[[".to_string(), Command::PrevHeading),
            ],
        }
    }

    // Feeds keys to `v` like the editor would. `<Esc>` and `<Enter>` are
    // special. In Insert mode a PassThrough char or Enter is typed into `b`.
    fn run(v: &mut Vim, b: &mut MockBuffer, keys: &str) -> Vec<VimOutcome> {
        let mut keys_in = Vec::new();
        let mut s = keys;
        while !s.is_empty() {
            if let Some(r) = s.strip_prefix("<Esc>") {
                keys_in.push(KeyInput::plain(Key::Escape));
                s = r;
            } else if let Some(r) = s.strip_prefix("<Enter>") {
                keys_in.push(KeyInput::plain(Key::Enter));
                s = r;
            } else {
                let c = s.chars().next().unwrap_or_default();
                keys_in.push(KeyInput::char(c));
                s = &s[c.len_utf8()..];
            }
        }
        let mut out = Vec::new();
        for key in keys_in {
            let inserting = v.mode() == Mode::Insert;
            let typed = match key.key {
                Key::Char(c) => c.to_string(),
                Key::Enter => "\n".to_string(),
                _ => String::new(),
            };
            let outcome = v.handle_key(key, b);
            if inserting && outcome == VimOutcome::PassThrough && !typed.is_empty() {
                let at = b.cursor();
                let end = b.insert_text(at, &typed);
                b.set_cursor(end);
            }
            out.push(outcome);
        }
        out
    }

    fn vim() -> Vim {
        Vim::new(bindings())
    }

    // Text after `keys`, run from the top left.
    fn edit(text: &str, keys: &str) -> String {
        let mut b = MockBuffer::new(text);
        run(&mut vim(), &mut b, keys);
        b.text()
    }

    fn cursor_after(text: &str, keys: &str) -> Cursor {
        let mut b = MockBuffer::new(text);
        run(&mut vim(), &mut b, keys);
        b.cursor()
    }

    fn outcomes(text: &str, keys: &str) -> Vec<VimOutcome> {
        let mut b = MockBuffer::new(text);
        run(&mut vim(), &mut b, keys)
    }

    #[test]
    fn mode_labels() {
        let mut v = vim();
        let mut b = MockBuffer::new("abc");
        assert_eq!(v.mode().label(), "NORMAL");
        run(&mut v, &mut b, "i");
        assert_eq!(v.mode().label(), "INSERT");
        run(&mut v, &mut b, "<Esc>v");
        assert_eq!(v.mode().label(), "VISUAL");
        run(&mut v, &mut b, "V");
        assert_eq!(v.mode().label(), "V-LINE");
    }

    #[test]
    fn motions_move_cursor() {
        assert_eq!(cursor_after("abc\ndef", "lj"), Cursor { line: 1, col: 1 });
        assert_eq!(cursor_after("abcdef", "3l"), Cursor { line: 0, col: 3 });
        assert_eq!(cursor_after("foo bar", "$b"), Cursor { line: 0, col: 4 });
        assert_eq!(cursor_after("foo bar", "e"), Cursor { line: 0, col: 2 });
        assert_eq!(cursor_after("  ab", "$0"), Cursor { line: 0, col: 0 });
        assert_eq!(cursor_after("  ab", "$^"), Cursor { line: 0, col: 2 });
        assert_eq!(cursor_after("a\n b\nc", "G"), Cursor { line: 2, col: 0 });
        assert_eq!(cursor_after("a\n b\nc", "Ggg"), Cursor { line: 0, col: 0 });
    }

    #[test]
    fn find_and_repeat_find() {
        assert_eq!(edit("a,b,c", "f,;x"), "a,bc");
        assert_eq!(edit("abc,d", "dt,"), ",d");
        assert_eq!(edit("a,b,c", "$F,,x"), "a,bc");
    }

    #[test]
    fn delete_word_and_line_end() {
        assert_eq!(edit("foo bar", "dw"), "bar");
        assert_eq!(edit("abc def", "wD"), "abc ");
        assert_eq!(edit("foo bar", "wdiw"), "foo ");
    }

    #[test]
    fn delete_lines() {
        assert_eq!(edit("a\nb\nc", "jdd"), "a\nc");
        assert_eq!(edit("a\nb\nc\nd", "2dd"), "c\nd");
        assert_eq!(edit("a\nb", "jdd"), "a");
        assert_eq!(edit("a\nb\nc", "jdG"), "a");
        assert_eq!(edit("a\nb\n\nc", "dip"), "\nc");
    }

    #[test]
    fn yank_and_paste() {
        assert_eq!(edit("a\nb", "yyjp"), "a\nb\na");
        assert_eq!(edit("x", "yy3p"), "x\nx\nx\nx");
        assert_eq!(edit("ab", "ylP"), "aab");
        assert_eq!(edit("ab", "yl$p"), "aba");
        let mut v = vim();
        let mut b = MockBuffer::new("ab");
        run(&mut v, &mut b, "yy");
        assert_eq!(v.register.kind(), RegisterKind::Linewise);
        assert_eq!(v.register.text(), "ab");
    }

    #[test]
    fn x_and_capital_x() {
        assert_eq!(edit("abc", "x"), "bc");
        assert_eq!(edit("abcd", "3x"), "d");
        assert_eq!(edit("abc", "$X"), "ac");
    }

    #[test]
    fn change_commands_enter_insert() {
        assert_eq!(edit("foo bar", "cwxy<Esc>"), "xybar");
        assert_eq!(edit("abc def", "wCx<Esc>"), "abc x");
        assert_eq!(edit("foo bar", "ciwX<Esc>"), "X bar");
        assert_eq!(edit("f(a, b)", "f(di("), "f()");
        assert_eq!(edit("a\nb", "cc<Enter>x<Esc>"), "\nx\nb");
    }

    #[test]
    fn operator_counts_multiply() {
        assert_eq!(edit("a b c d e f g h", "2d3w"), "g h");
    }

    #[test]
    fn escape_clears_pending_and_leaves_visual() {
        assert_eq!(cursor_after("abc", "2<Esc>l"), Cursor { line: 0, col: 1 });
        let mut v = vim();
        let mut b = MockBuffer::new("abc");
        run(&mut v, &mut b, "vl<Esc>");
        assert_eq!(v.mode(), Mode::Normal);
        assert_eq!(b.selection(), None);
    }

    #[test]
    fn pending_display_shows_partial_input() {
        let mut v = vim();
        let mut b = MockBuffer::new("abc");
        run(&mut v, &mut b, "2d");
        assert_eq!(v.pending_display().as_deref(), Some("2d"));
        run(&mut v, &mut b, "<Esc>");
        assert_eq!(v.pending_display(), None);
        run(&mut v, &mut b, " ");
        assert_eq!(v.pending_display().as_deref(), Some("<Space>"));
    }

    #[test]
    fn reset_returns_to_normal() {
        let mut v = vim();
        let mut b = MockBuffer::new("abc");
        run(&mut v, &mut b, "v2d");
        v.reset();
        assert_eq!(v.mode(), Mode::Normal);
        assert_eq!(v.pending_display(), None);
    }

    #[test]
    fn gd_and_heading_sequences_return_commands() {
        assert_eq!(outcomes("a", "gd").last(), Some(&VimOutcome::Command(Command::FollowLink)));
        assert_eq!(outcomes("a", "]]").last(), Some(&VimOutcome::Command(Command::NextHeading)));
        assert_eq!(outcomes("a", "[[").last(), Some(&VimOutcome::Command(Command::PrevHeading)));
        assert_eq!(outcomes("a", "g").last(), Some(&VimOutcome::Handled));
    }

    #[test]
    fn leader_keys_return_commands() {
        assert_eq!(outcomes("a", " f").last(), Some(&VimOutcome::Command(Command::QuickOpen)));
        assert_eq!(outcomes("a", " z").last(), Some(&VimOutcome::Command(Command::ZenMode)));
        assert_eq!(outcomes("a", " e").last(), Some(&VimOutcome::Command(Command::ToggleTree)));
        assert_eq!(outcomes("a", " o").last(), Some(&VimOutcome::Command(Command::Outline)));
        assert_eq!(outcomes("a", " p").last(), Some(&VimOutcome::Command(Command::CommandPalette)));
        assert_eq!(outcomes("a", " /").last(), Some(&VimOutcome::Command(Command::SearchFolder)));
        assert_eq!(outcomes("a", " q").last(), Some(&VimOutcome::Handled));
    }

    #[test]
    fn undo_and_redo_are_commands() {
        assert_eq!(outcomes("a", "u").last(), Some(&VimOutcome::Command(Command::Undo)));
        let mut v = vim();
        let mut b = MockBuffer::new("a");
        let redo = KeyInput {
            key: Key::Char('r'),
            mods: crate::key::Modifiers { ctrl: true, ..Default::default() },
        };
        assert_eq!(v.handle_key(redo, &mut b), VimOutcome::Command(Command::Redo));
    }

    #[test]
    fn cmd_keys_pass_through() {
        let mut v = vim();
        let mut b = MockBuffer::new("a");
        let save = KeyInput {
            key: Key::Char('s'),
            mods: crate::key::Modifiers { cmd: true, ..Default::default() },
        };
        assert_eq!(v.handle_key(save, &mut b), VimOutcome::PassThrough);
    }

    #[test]
    fn insert_keys_pass_through_and_escape_moves_left() {
        assert_eq!(
            outcomes("abc", "ix<Enter><Esc>"),
            vec![
                VimOutcome::Handled,
                VimOutcome::PassThrough,
                VimOutcome::PassThrough,
                VimOutcome::Handled,
            ]
        );
        assert_eq!(edit("ab", "i<Enter><Esc>"), "\nab");
        assert_eq!(cursor_after("abc", "A<Esc>"), Cursor { line: 0, col: 2 });
    }

    #[test]
    fn visual_delete_and_change() {
        assert_eq!(edit("abcdef", "vlld"), "def");
        assert_eq!(edit("abc", "vlcX<Esc>"), "Xc");
        assert_eq!(edit("a\nb\nc", "Vjd"), "c");
    }

    #[test]
    fn visual_yank_then_paste() {
        assert_eq!(edit("abc", "vly$p"), "abcab");
    }

    #[test]
    fn visual_mode_selects_text() {
        let mut v = vim();
        let mut b = MockBuffer::new("abc");
        run(&mut v, &mut b, "vl");
        assert_eq!(b.selection(), Some(Selection { anchor: Cursor { line: 0, col: 0 }, head: Cursor { line: 0, col: 1 } }));
    }

    #[test]
    fn dot_repeats_delete_and_word_change() {
        assert_eq!(edit("abcd", "x."), "cd");
        assert_eq!(edit("a b c d", "dw."), "c d");
    }

    #[test]
    fn dot_repeats_insert() {
        assert_eq!(edit("x\nx", "ihi<Esc>j."), "hix\nhix");
    }

    #[test]
    fn open_lines_and_append() {
        assert_eq!(edit("a", "ob<Esc>"), "a\nb");
        assert_eq!(edit("a", "Ob<Esc>"), "b\na");
        assert_eq!(edit("a", "Ab<Esc>"), "ab");
        assert_eq!(edit("ac", "ab<Esc>"), "abc");
        assert_eq!(edit("  a", "Ib<Esc>"), "  ba");
    }
}
