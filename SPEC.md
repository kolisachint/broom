# broom — spec

A native macOS markdown editor. Rust. GPUI + gpui-kit.

## Platform

- macOS only.
- UI: GPUI Kit `gpui-kit = "=0.7.3"` (wraps gpui-component 0.7.3 and GPUI `gpui-pre =0.3.9`).
- Binary: `broom [folder]`. No folder means the current directory (or `$HOME` when cwd is `/`).

## Files

- The tree shows all files except `.git` and `.DS_Store` (and `*.broom-tmp` temp files from atomic saves).
- `.md` / `.markdown` open in broom.
- Everything else opens in the default macOS app (`open`).
- Folders sort first, then names (case-insensitive).
- Single click or Space = VSCode-style **preview tab**.
  - A preview tab is replaced by the next preview.
  - It becomes **pinned** on first edit or on double-click / Enter.
- Autosave only for named files. Untitled buffers are never autosaved.
- External change while a file is open: warn with Reload / Keep mine / Save as.
- Crash recovery: drafts live in `~/.local/state/broom/recovery`.
- Files over 1 MB (`large_file_bytes`) open in source mode with a warning bar:
  `Large file — opened as plain text. [Open formatted anyway]`

## Editing model

- True WYSIWYG. Markdown symbols are never shown in formatted mode.
  - `# ` becomes a heading.
  - `**x**` becomes bold `x`.
  - `[[x]]` becomes a wikilink chip.
- `Cmd+/` toggles plain-text source mode (raw markdown, same file).
- Untouched blocks are byte-identical on save.
  Only edited blocks are re-serialized.
- Block kinds: heading, paragraph, list, task list, blockquote, code block,
  GFM table, thematic break, front matter, math block, opaque.
- Opaque blocks (raw HTML, footnote definitions, unknown) are LOCKED.
  - Enter opens a mini source editor for that block.
- Inline: bold, italic, strike, code, link, wikilink, inline math.
- Math is KaTeX-style. Renderer backend is not chosen yet (`broom-math`).
- Diagrams are NOT rendered. A ` ```mermaid ` block shows as a code block
  labelled `diagram (not rendered)`.
- Input rules, typed at the start of a block or after a delimiter:
  - `# ` through `###### ` headings
  - `- ` / `* ` bullet list
  - `1. ` ordered list
  - `- [ ] ` task list
  - `> ` blockquote
  - `**x**` bold, `*x*` italic, `` `x` `` code
  - `[[` opens wikilink autocomplete

## Vim (off by default)

- Toggle: `Cmd+Alt+V` or setting `vim_enabled`.
- Modes: NORMAL, INSERT, VISUAL, V-LINE. Shown as a word in the status bar.
- Motions run over VISIBLE text. Formatting is a property of text, not a
  mode.
- Leader = `<Space>`:
  - `f` quick open
  - `e` tree
  - `/` search folder
  - `o` outline
  - `p` palette
  - `z` zen
- Normal sequences: `gd` follow link, `]]` next heading, `[[` previous heading.
- Tree (vim keys): `j` `k` `h` `l`, `a` new, `r` rename, `d` delete,
  `Space` preview tab.

## Layout

- Three panes: tree | editor | right panel.
- Right panel = outline + backlinks. Hide with `show_right_panel` or `Cmd+Alt+B`
  (not yet bound).
- Status bar: mode word | file name | word count | save state
  (saved / unsaved / saving / conflict).
- Save state is never color-only. Always use text.
- Zen mode hides tree, right panel and status bar.

## Links

- Wikilinks: `[[target]]` and `[[target|alias]]`.
- `[[` opens autocomplete from the folder index.
- Backlinks listed in the right panel.
- Rename a note: all links to it are rewritten (`LinkIndex::rename_rewrite`).
  A link with no alias becomes `[[new]]`.

## Look

- Light theme: Solarized Light family.
- Body font: `.SystemUIFont` (SF Pro).
- Mono font: `JetBrainsMono Nerd Font Mono`.
- Default size: 16 pt. Zoom with `Cmd+=` / `Cmd+-`.
- Cursor: thick, high contrast.
- Never rely on color alone for meaning.

## Config (XDG, nothing loose in `$HOME`)

- `~/.config/broom/settings.toml`
- `~/.config/broom/keymap.toml`
- Honors `$XDG_CONFIG_HOME` and `$XDG_STATE_HOME` when set.
- Both files hot-reload (notify).

### settings.toml (defaults)

```toml
theme = "solarized-light"
body_font = ".SystemUIFont"
mono_font = "JetBrainsMono Nerd Font Mono"
font_size = 16.0
vim_enabled = false
autosave_delay_ms = 1000
large_file_bytes = 1048576
show_right_panel = true
```

### keymap.toml (user overrides, merged over defaults)

```toml
[[bind]]
keys = "cmd+shift+p"      # chords separated by spaces: "cmd+k z"
command = "CommandPalette"
context = "global"         # global | editor | tree
```

## Keymap defaults

Global:

| Chord | Command |
|---|---|
| Cmd+P | QuickOpen |
| Cmd+Shift+P | CommandPalette |
| Cmd+B | ToggleTree (in editor: Bold) |
| Cmd+Shift+E | FocusTree |
| Cmd+Shift+F | SearchFolder |
| Cmd+Shift+O | Outline |
| Cmd+/ | ToggleSourceMode |
| Cmd+Alt+V | ToggleVim |
| Cmd+= | ZoomIn |
| Cmd+- | ZoomOut |
| Cmd+S | Save |
| Cmd+W | CloseTab |
| Cmd+K Z | ZenMode |
| Ctrl+? | Cheatsheet |
| Ctrl+- | Back |

Editor context:

| Chord | Command |
|---|---|
| Cmd+B | Bold |
| Cmd+I | Italic |
| Cmd+Shift+X | Strike |
| Cmd+K | InsertLink |
| Cmd+Shift+1..6 | Heading1..Heading6 |

Tree context:

| Chord | Command |
|---|---|
| Up / Down | TreeUp / TreeDown |
| Left / Right | TreeCollapse / TreeExpand |
| Enter | TreeOpenPinned |
| Space | TreePreview |
| Cmd+Backspace | TreeDelete |
| F2 | TreeRename |

Vim tree: `j` `k` `h` `l` `a` `r` `d` `Space` (see Vim).

Open decision: `Cmd+K` is both InsertLink and the first key of `Cmd+K Z`.
Resolution: after Cmd+K, wait up to 800 ms. `z` = ZenMode. Timeout = InsertLink.

## Crates

| Crate | Role |
|---|---|
| broom-core | Document model: blocks, inline runs, edits, undo, input rules |
| broom-vim | Modal state machine over a `VimBuffer` trait (no UI deps) |
| broom-links | Wikilink parse, `LinkIndex`, backlinks, rename rewrite |
| broom-fs | File tree, watcher, autosave, conflicts, recovery, large-file check |
| broom-config | XDG paths, `Settings`, `Keymap`, `Command`, config hot reload |
| broom-math | `render_math` (stub) and `MathCache` |
| broom-app | The `broom` binary: GPUI window and layout |

Dependencies:

```
broom-app -> all
broom-core -> broom-links
broom-links -> broom-fs
broom-vim -> broom-config
broom-config, broom-fs, broom-math: leaves
```

## Status

- Scaffold only. Every crate compiles. Bodies are `todo!()` except
  `broom-math::render_math`, which returns `Err(Unsupported)`.
- Next: implement crates in parallel against these APIs.
