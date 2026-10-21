# Architecture

TeXoria is a Cargo workspace in `crates/`. Only `texoria` depends on the UI
([GPUI](https://www.gpui.rs) through [gpui-kit](https://crates.io/crates/gpui-kit)): every other
crate is plain logic with its own unit tests, usable without a window.

```
texoria ─┬─ tex-compile ── tex-log
         ├─ tex-agent (ACP, MCP) ── tex-project, tex-log
         ├─ tex-git (system `git` CLI)
         ├─ tex-complete ── tex-project ── tex-syntax
         ├─ tex-spell ── tex-syntax
         ├─ synctex
         ├─ pdf-view (PDFium)
         └─ tex-update (self-update)
```

Most crates also use `tex-sys`, a leaf crate for what they share about the system: child
processes and their trees, paths and the `PATH`, verified downloads.

| Crate | Role |
|---|---|
| [`texoria`](../../crates/texoria) | The app: window, editor tabs, panels, settings, themes |
| [`tex-compile`](../../crates/tex-compile) | Engine discovery and the background compile scheduler |
| [`tex-log`](../../crates/tex-log) | TeX log parser, producing diagnostics |
| [`tex-git`](../../crates/tex-git) | Git through the system `git` CLI, Overleaf URLs |
| [`tex-agent`](../../crates/tex-agent) | Coding agents: discovery, ACP client, MCP tools, checkpoints |
| [`tex-syntax`](../../crates/tex-syntax) | Tree-sitter LaTeX grammar, highlighting, semantic index |
| [`tex-project`](../../crates/tex-project) | Root file detection, include graph, `.bib` files, file scan |
| [`tex-complete`](../../crates/tex-complete) | Context-aware completion with fuzzy ranking |
| [`tex-spell`](../../crates/tex-spell) | Prose extraction, language detection, Hunspell dictionaries |
| [`synctex`](../../crates/synctex) | `.synctex(.gz)` reader, forward and inverse search |
| [`pdf-view`](../../crates/pdf-view) | PDF rasterization with PDFium on a dedicated thread |
| [`tex-update`](../../crates/tex-update) | Updates: latest release, signed checksums, download and install |
| [`tex-sys`](../../crates/tex-sys) | Child processes, paths and the `PATH`, verified downloads |

## Build

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
builds. Compiler events reach the UI over a channel.

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

## Coding agents

`tex-agent` drives the agents installed on the system through the Agent Client Protocol: JSON-RPC
over the agent's stdin/stdout, with the wire types of `agent-client-protocol-schema` and a
transport of its own (`rpc.rs`, threads like `tex-git` and `tex-compile`). `catalog::detect`
finds the known agents (the commands of the ACP registry) and the user's own on the login
shell's `PATH` plus the usual install folders, which is also the `PATH` agents run with (Node
scripts need it). Claude Code and Codex go through their npm adapters, pinned, with `npx`.

Agents write files on their own, whatever the client offers, so the review does not rely on
`fs/write_text_file`: `Checkpoint` records the project before each turn and `changes()`
compares it afterwards, which drives the review (a patch for the diff tab) and the undo. The
editors are read-only during a turn, so no buffer is dirty when the agent writes.

TeXoria's own tools (`build`, `problems`, `outline`) are an MCP server on a loopback port
(`mcp.rs`): agents start `texoria --mcp-bridge`, which relays its stdio to it after a random
token. The calls are answered by the workspace on the UI thread; `build` waits for the first
run that starts after it.

## Security model

Opening a project must not run code from it. Two things in a project can:

- a `latexmkrc`, which is Perl: latexmk runs with `-norc` (plus the user's and system's rc
  files) until the user allows the project's own;
- the repository's Git configuration (`core.fsmonitor`, filters, …), which can run on a plain
  `git status`: Git only runs in projects the user trusted.

Coding agents read and change a project and follow their own settings in it (which can run
hooks), so they only start in projects the user allowed, a third permission; the files they
write through TeXoria must be inside the project.

The permissions are stored in the settings for exact directories, never their subdirectories.
Updates only install a package listed in a `SHA256SUMS` signed by the release workflow's
minisign key for that very version; the public key is built into the app.

Credentials are passed to Git through the environment, never on the command line, in a URL or
in `.git/config`; Overleaf tokens are only sent over HTTPS and live in the system keychain.

## The app

- `workspace.rs` is the main window: sidebar (outline, files, source control or agent), editor
  tabs, PDF pane and status bar. Its methods are split by concern into `workspace/*.rs`.
- `dialogs.rs`, `ui.rs`, `tasks.rs`, `underlines.rs` and `text.rs` hold what the views share:
  dialogs, interface pieces, background work, wavy underlines and text helpers.
- `git_panel.rs`, `diff_view.rs` and `overleaf.rs` implement source control.
- `agent_panel.rs` is the Agent sidebar (chat, permissions, review), `agent_tools.rs` the MCP
  tools answered by the workspace.
- `file_tree.rs` is the Files view, `spell.rs` the spell-checking glue, `latex.rs` registers the
  LaTeX language and completions with the editor.
- `settings.rs` and `settings_window.rs` hold the settings (TOML in the OS config folder) and
  their window. Files are written atomically (temp file + rename).
- `updater.rs` checks for releases, downloads them and installs them on restart (`tex-update`).
- Actions, key bindings and menus are declared in `main.rs`.

[`AGENTS.md`](../../AGENTS.md) goes deeper into each crate and the invariants to preserve.
