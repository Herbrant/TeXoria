# Architecture

TeXoria is a Cargo workspace in `crates/`. Only `texoria` depends on the UI
([GPUI](https://www.gpui.rs) through [gpui-kit](https://crates.io/crates/gpui-kit)): every other
crate is plain logic with its own unit tests, usable without a window.

```
texoria ─┬─ tex-compile ── tex-log
         ├─ tex-git (system `git` CLI)
         ├─ tex-complete ── tex-project ── tex-syntax
         ├─ tex-spell ── tex-syntax
         ├─ synctex
         └─ pdf-view (PDFium)
```

| Crate | Role |
|---|---|
| [`texoria`](../../crates/texoria) | The app: window, editor tabs, panels, settings, themes |
| [`tex-compile`](../../crates/tex-compile) | Engine discovery and the background compile scheduler |
| [`tex-log`](../../crates/tex-log) | TeX log parser, producing diagnostics |
| [`tex-git`](../../crates/tex-git) | Git through the system `git` CLI, Overleaf URLs |
| [`tex-syntax`](../../crates/tex-syntax) | Tree-sitter LaTeX grammar, highlighting, semantic index |
| [`tex-project`](../../crates/tex-project) | Root file detection, include graph, `.bib` files, file scan |
| [`tex-complete`](../../crates/tex-complete) | Context-aware completion with fuzzy ranking |
| [`tex-spell`](../../crates/tex-spell) | Prose extraction, language detection, Hunspell dictionaries |
| [`synctex`](../../crates/synctex) | `.synctex(.gz)` reader, forward and inverse search |
| [`pdf-view`](../../crates/pdf-view) | PDF rasterization with PDFium on a dedicated thread |

## Typesetting

`tex_compile::engine::detect()` returns the engines best first: the system latexmk (pdfLaTeX,
XeLaTeX and LuaLaTeX flavors), also looked up in the well-known TeX Live, MacTeX, MiKTeX and
TinyTeX folders that a GUI app may not have on its `PATH`, then Tectonic (bundled with the app,
or on `PATH`).

`scheduler::Compiler` runs one engine process at a time, in its own process tree. Requests made
during a run are coalesced into a single follow-up run, and a running job is only killed by an
explicit cancel, an engine change or the timeout, never by new edits. Artifacts always go to
`<root dir>/.texoria/build`, and the log is parsed by `tex-log` into diagnostics.

Debouncing lives in the UI: each edit restarts a timer, and when it fires the workspace
re-indexes the open buffers (so the outline and completions follow unsaved text), saves and
typesets. Compiler events reach the UI over a channel.

## Preview

PDFium is not thread-safe, so `pdf-view` gives a single worker thread ownership of the library
and the documents. The UI sends the pages it wants, in priority order, and gets BGRA bitmaps
back. PDFs are loaded from memory, never from disk, because an open file handle on Windows would
stop the engine from overwriting the PDF. `pdf_pane.rs` keeps the old bitmaps on screen until the
new PDF's pages are ready and only renders pages near the viewport.

`synctex` coordinates are PDF points with a top-left origin, y growing downwards.

## Git

`tex-git` shells out to the user's `git` instead of linking a library, which keeps credential
helpers, ssh-agent, signing and hooks working. Every command runs without a terminal
(`GIT_TERMINAL_PROMPT=0`), with literal pathspecs and English messages, so errors can be
classified by their text. Network commands have a timeout and can be cancelled. The Git panel
runs one operation at a time on the background executor; operations that rewrite files reload
the clean buffers whose file changed.

## Security model

Opening a project must not run code from it. Two things in a project can:

- a `latexmkrc`, which is Perl: latexmk runs with `-norc` (plus the user's and system's rc
  files) until the user allows the project's own;
- the repository's Git configuration (`core.fsmonitor`, filters, …), which can run on a plain
  `git status`: Git only runs in projects the user trusted.

Both permissions are stored in the settings for exact directories, never their subdirectories.
Credentials are passed to Git through the environment, never on the command line, in a URL or
in `.git/config`; Overleaf tokens are only sent over HTTPS and live in the system keychain.

## The app

- `workspace.rs` is the main window: sidebar (outline, files or source control), editor tabs,
  PDF pane and status bar.
- `git_panel.rs`, `diff_view.rs` and `overleaf.rs` implement source control.
- `file_tree.rs` is the Files view, `spell.rs` the spell-checking glue, `latex.rs` registers the
  LaTeX language and completions with the editor.
- `settings.rs` and `settings_window.rs` hold the settings (TOML in the OS config folder) and
  their window. Files are written atomically (temp file + rename).
- Actions, key bindings and menus are declared in `main.rs`.

[`AGENTS.md`](../../AGENTS.md) goes deeper into each crate and the invariants to preserve.
