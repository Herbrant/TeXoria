# AGENTS.md

This file provides guidance to AI coding agents when working with code in this repository.

TeXoria is a LaTeX editor in Rust (edition 2024, stable toolchain) built on GPUI via the
`gpui-kit` crate (pinned with `=`, pre-1.0 — check its API in `~/.cargo/registry` rather than
assuming upstream GPUI/Zed APIs).

## Commands

```sh
./scripts/fetch-pdfium.sh            # PDFium → vendor/pdfium (needed for the PDF preview; CI runs it)
./scripts/fetch-tectonic.sh          # Tectonic → vendor/tectonic (optional fallback engine)
cargo run -p texoria -- path/to/document.tex   # also accepts a directory (opens main.tex or the first .tex with \documentclass)

cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p tex-log                          # one crate
cargo test -p synctex inverse                  # tests matching a name filter
cargo test -p tex-compile -- --include-ignored # also runs the Tectonic e2e test (downloads TeX bundle first time)

cargo packager --release -p texoria --formats app && ./scripts/package-macos.sh  # ad-hoc signed .dmg
cargo packager --release -p texoria --formats nsis|appimage   # Windows installer / AppImage
./scripts/package-linux.sh           # Linux, after a release build: .tar.gz, .deb, .rpm (nfpm)
./scripts/gen-icons.sh               # assets/logo/*.svg → crates/texoria/packaging (macOS only)
```

CI (`.github/workflows/ci.yml`, macOS/Ubuntu/Windows) runs fmt check, clippy with `-D warnings`,
tests and a release build — keep all four green. `rustfmt.toml` sets `max_width = 100`.

## Releasing

Bump `version` in `[workspace.package]` (`Cargo.toml`), commit `chore(release): vX.Y.Z`, then
`git tag vX.Y.Z && git push origin main vX.Y.Z`. `.github/workflows/release.yml` checks that the
tag matches the version (`scripts/version.sh`), builds on native runners (macOS arm64/x64,
Ubuntu 22.04 x64/arm64 for an old enough glibc, Windows x64), publishes the GitHub release with
`SHA256SUMS`, and renders `crates/texoria/packaging/{homebrew,aur}/*.in`
(`scripts/render-manifests.sh`) into the cask pushed to `Herbrant/homebrew-tap`
(secret `HOMEBREW_TAP_TOKEN`) and the AUR `texoria-bin` package (secret `AUR_SSH_PRIVATE_KEY`).
Pre-release tags (`v1.2.0-rc.1`) skip Homebrew and the AUR; a manual run only builds artifacts.
The Homebrew/AUR step is `.github/workflows/publish.yml` (reads `SHA256SUMS` from the release):
run it by hand with the tag to retry it.
Asset names (`texoria-<v>-<os>-<arch>.<ext>`, `aarch64`/`x86_64`) are referenced by the templates
and by `scripts/install.sh`, the end-user installer for macOS/Linux (latest release, checked
against `SHA256SUMS`, no root: `/Applications` or `~/.local`).

- There is no Apple Developer ID: `package-macos.sh` signs ad hoc (no notarization), and the cask
  clears the quarantine flag in `postflight_steps`.
- The Linux layout (`usr/lib/texoria/{libpdfium.so,bin/tectonic}`) is what `pdfium_dirs()` and
  `bundled_tectonic_dirs()` look for. Package dependencies are listed by hand in
  `crates/texoria/packaging/linux/nfpm.yaml` and `crates/texoria/packaging/aur/PKGBUILD.in`;
  `package-linux.sh` fails if a binary links a shared library outside its allowlist, so a new one
  must be added to all three.
- Linux bundles Tectonic's static musl builds (the gnu one needs glibc 2.39).

Tests that depend on the environment skip by returning early (they pass trivially) rather than
failing: `tex-compile/tests/real_engine.rs` needs a TeX engine on the host, and
`pdf-view`'s `renders_sample_pdf` needs both `vendor/pdfium` and a previously compiled
`tests/fixtures/sample/.texoria/build/main.pdf`. Shared fixtures live in `tests/fixtures/` and are
referenced via `CARGO_MANIFEST_DIR/../../tests/fixtures`. The sample project must compile with
**zero diagnostics** (asserted by the e2e test; it is also the README screenshot).

## Architecture

Cargo workspace, `crates/*`. Only `texoria` depends on the UI; every other crate is plain,
window-free logic with its own unit tests. Dependency flow:

```
texoria ─┬─ tex-compile ── tex-log
         ├─ tex-agent (ACP client, MCP server) ── tex-project, tex-log
         ├─ tex-git (system `git` CLI)
         ├─ tex-complete ── tex-project ── tex-syntax
         ├─ tex-spell ── tex-syntax
         ├─ synctex
         └─ pdf-view (PDFium)
```

- **tex-compile**: `engine::detect()` returns engines best-first — system latexmk (pdfLaTeX,
  XeLaTeX, LuaLaTeX flavors; also probes well-known TeX Live/MacTeX/MiKTeX/TinyTeX dirs not on a
  GUI app's `PATH`) then Tectonic (bundled next to the executable / in the app bundle, or on
  `PATH`). `scheduler::Compiler` runs one engine process at a time in its own process tree;
  requests during a run are coalesced into one follow-up run, and a running job is never killed by
  new edits (only by explicit `cancel`, an engine change or the timeout). Build artifacts always
  go to `<root dir>/.texoria/build` (`Job::for_root`), and the log is parsed with `tex-log` into
  diagnostics. The root file is passed as `./name` (a name starting with `-` must not become an
  option), and unless `Job::allow_project_rc` is set (the user allowed it, see
  `Settings::rc_allowed`) latexmk runs with `-norc` plus `-r` for the system/user rc files,
  because a project's `latexmkrc` is arbitrary Perl. The scheduler also writes
  `.texoria/.gitignore` (`*`) so build artifacts never show up in Git.
- **tex-git**: shells out to the user's `git` (keeps credential helpers, ssh-agent, signing and
  hooks working) instead of a library. Every command runs without a terminal
  (`GIT_TERMINAL_PROMPT=0`, a new session on Unix), with `GIT_LITERAL_PATHSPECS=1` and English
  messages (errors are classified by text into `Error::{NoIdentity, Rejected, Auth, Unreachable,
  …}`); network operations have a timeout and a `Cancel` token. `Git::credential` gives network
  commands a username/password for one URL prefix through an inline `credential.<url>.helper`
  that reads them from the environment (never the command line, a URL or `.git/config`), after
  clearing the user's helpers for that URL only. `overleaf.rs` parses Overleaf/Server Pro
  project and remote URLs; `Repo::sync` is commit-all + `pull --rebase` + push. A repository's
  own config can run programs even on `git status` (`core.fsmonitor`, filters), so the app only
  uses a `Repo` for trusted projects. Trust is split in `Settings`, both exact directories (never
  their subdirectories): `trusted_projects` lets Git run, `rc_allowed` lets a `latexmkrc` run; a
  clone only gets the first, when the root file's repository is the clone itself. Git trust is
  the repository's working tree (`git_panel::repo_top` finds the nearest `.git` without running
  git) and `Git::discover_at` sets `GIT_CEILING_DIRECTORIES`, so git never climbs to an
  untrusted repository in a parent folder.
  Credentials are refused if a value has a control character (the credential protocol is line
  based), Overleaf servers must be HTTPS, and commands are logged without their environment. `Git::find` is lazy (on macOS `/usr/bin/git` is skipped without the
  Command Line Tools, whose stub would pop up an installer). Integration tests in
  `tests/repo.rs` use temp repos with `GIT_CONFIG_GLOBAL` pointing to a temp file and skip if git
  is missing.
- **tex-agent**: coding agents installed on the system (Claude Code, Codex, Gemini CLI, …)
  through the Agent Client Protocol. Only the wire types come from
  `agent-client-protocol-schema`; the JSON-RPC transport (`rpc.rs`) is ours, on threads.
  `catalog::KNOWN` follows the ACP registry's commands (Claude Code and Codex through their npm
  adapters, pinned, with `npx` when their CLI is installed); `process::search_path` is the login
  shell's `PATH` plus the usual install folders, used both to find agents and to run them (Node
  scripts need it). Agents write files on their own whatever the client offers, so changes are
  found by comparing the project with a `Checkpoint` taken before each turn (not `.git` nor
  `.texoria`), which drives the review patch and the undo. `mcp.rs` serves TeXoria's tools
  (`typeset`, `problems`, `outline`) on a loopback port; agents start `texoria --mcp-bridge`
  (handled in `main.rs` before the UI), which relays stdio there after a random token.
  `cargo run -p tex-agent --example probe -- <id> <prompt>` talks to an installed agent.
- **tex-spell**: spell checking. `extract::words` walks the tree-sitter tree and keeps only prose
  (allow-list of node kinds, deny-lists of commands/environments, no preamble but `\title`);
  `lang::detect` picks the language: `% !TeX spellcheck` (file, then root), babel/polyglossia
  (`Index::{language_options, main_language}` from tex-syntax, root first), whatlang on the prose,
  then the system language. `catalog` lists the ~90 Hunspell dictionaries of
  wooorm/dictionaries; `store` uses system ones (`DICPATH`, `/usr/share/hunspell`,
  `~/Library/Spelling`...) or downloads them at the pinned `store::COMMIT`, checking the SHA-256
  in `data/dictionaries.tsv` (regenerate both with `scripts/gen-dictionaries.sh <commit>`).
  Dictionaries are never bundled (their licenses vary, GPL included). spellbook is stricter than
  Hunspell: `repair` reads numeric flags like Hunspell and drops unreadable lines so that every
  catalog dictionary loads (keep it that way when bumping the commit).
- **texoria** (`workspace.rs` is the main window: outline or files | editor tabs | PDF pane):
  - `git_panel.rs` is the Source Control sidebar (alternates with the outline). Mutating
    operations are emitted as `GitPanelEvent::Request(GitOp)`; the workspace saves dirty buffers
    and calls `GitPanel::execute`, which runs one operation at a time on the background executor.
    Operations that rewrite files (pull, switch, discard, abort) trigger
    `Workspace::reload_from_disk`, which also runs on window activation: clean buffers whose file
    changed are reloaded, tabs of deleted files close.
  - `overleaf.rs` keeps Overleaf tokens in the system keychain through GPUI's
    `{read,write}_credentials` (keyed by server URL, cached in a global, memory-only if the
    keychain fails). A typed token is saved only once Overleaf accepted it, and a rejected one is
    never deleted (the refusal may concern one project only). The Git panel detects an Overleaf
    remote on refresh, passes the token as a `Credential`, offers `GitOp::Sync`, and asks for a
    new token on `Error::Auth`. Before a Sync that adds or deletes files it shows them
    (`SyncReview`), and `run_op` refuses the Sync if new ones appeared since. Overleaf projects
    get `Repo::exclude_local_files` (build artifacts, system/editor files, package caches in
    `.git/info/exclude`: a `.gitignore` would be pushed to the collaborators).
  - `file_tree.rs` is the Files view, which shares the sidebar with the outline (switch at the
    top; `Workspace::navigator` remembers which). It scans `root.parent()` with
    `tex_project::files::scan` on the background executor and renders it with gpui-kit's
    `tree` (whose `TreeItem` is a folder only if it has children, so `FileTree` keeps its own
    rows). Changes are emitted as `FileTreeEvent::Request(FileOp)` and run by
    `Workspace::run_file_op` after saving buffers: tabs follow a rename (a renamed root file
    goes through `set_root`) and close on delete. Git colors come from `GitPanel::file_marks`.
    Files that are not text (`files::looks_like_text`) open with the system app.
  - `diff_view.rs` is a single "preview" diff tab built on `gpui_kit::component::diff`
    (`DiffFile::parse` of git's patch, `parse_conflicts` for merge conflicts). It does not support
    combined diffs, so commits are shown against their first parent. The gpui-kit editor has no
    gutter-marker API, so there are no change markers in the editor itself.
  - Live preview: each buffer change schedules a timer (`settings.live_delay_ms`); on fire it
    re-indexes open buffers (outline/completions follow unsaved text) and, if live preview is on,
    saves and typesets. Debouncing lives in the UI, so the `Compiler` is created with
    `Duration::ZERO`.
  - Compiler events come from the scheduler's thread via a `smol` channel and are applied in a
    `cx.spawn` loop (`on_compile_event`).
  - `pdf_pane.rs` keeps old bitmaps on screen until the new PDF's pages are rasterized (no flash)
    and only materializes pages near the viewport. SyncTeX forward/inverse search goes through the
    `synctex` crate, whose coordinates are PDF points with a top-left origin, y downwards.
  - `agent_panel.rs` is the Agent sidebar (alternates with the outline). Agents only start in
    projects allowed in `Settings::agent_projects` (exact directories, like trust), and the
    agents are looked for on first use. A turn emits `AgentPanelEvent::Busy`: the workspace
    saves and makes every editor read-only until it ends, reloading from disk as tool calls
    complete. Messages and the context-menu edits (`RewriteSelection`, `EditWithAgent`) go
    through `AgentPanelEvent::Request` so the buffers are saved first; the workspace adds the
    open file and the selection to chat messages. The review is a `DiffSource::Agent` diff tab
    (no repository) with per-file undo. `agent_tools.rs` forwards the MCP calls to
    `Workspace::on_tool_call`; a `typeset` call is answered by the first run that starts after
    it (`ToolWaiter`, compile ids).
  - `spell.rs`: each `Buffer` has a `BufferSpell` (a `TextDecorationCollection` of wavy
    underlines, which follow edits, unlike editor diagnostics that are cleared on every edit).
    `Workspace::spellcheck` runs `spell::analyze` and `Speller::misspelled` on the background
    executor after `spell::DELAY`, and drops the result if the text changed meanwhile.
    Corrections come from the editor's custom context menu and a `CodeActionProvider`, both
    dispatching `ReplaceMisspelling`/`AddToDictionary` to the workspace. The language picker is in
    the status bar; per-file choices and the personal dictionary live in `Settings`.
  - `latex.rs` registers the tree-sitter LaTeX language with gpui-kit's `LanguageRegistry` and
    bridges `tex-complete` items (byte range + insert text + cursor offset) to the editor;
    the post-insert cursor move is done in `Workspace::on_buffer_changed` via "fixups".
  - Actions/keybindings/menus are declared in `main.rs` (`secondary-*` = ⌘ on macOS, Ctrl elsewhere).
  - The app icon's sources are `assets/logo/texoria.svg` (and `texoria-small.svg` for ≤ 48 px);
    the rasterized PNG/`.icns`/`.ico` in `crates/texoria/packaging` are committed. `build.rs`
    embeds the `.ico` as Windows resource ID 1, which is where GPUI loads the window icon from.
  - Settings are TOML in the OS config dir (`directories::ProjectDirs("app","TeXoria","TeXoria")`).
    `Workspace` owns them; `settings_window.rs` is the Settings window (⌘,, a second window built
    on gpui-kit's `setting` component) and writes every change through
    `Workspace::update_settings`, which saves and applies only what changed (engine, timeouts,
    soft wrap of open tabs, Git options, trust, theme). Its setters update the main window, so
    they must never run from inside it.
    Themes ("Paper" light, "Ink" dark) are in `assets/themes.json`; icons/fonts are embedded via
    `assets.rs`.
  - Files (sources and settings) are written through `atomic::write` (temp file + rename), and
    dirty buffers are saved on quit/window close.
- **pdf-view**: PDFium is not thread-safe, so a single dedicated worker thread owns the library and
  documents; the UI sends the wanted page set in priority order and gets BGRA bitmaps back. PDFs are
  loaded from memory, never from disk, because an open handle on Windows would block the engine
  from overwriting the PDF. The PDFium version in `fetch-pdfium.sh` must match pdfium-render's
  `pdfium_*` feature. PDFium is located at runtime via `pdfium_dirs()` in `workspace.rs`
  (next to the exe, bundle `Frameworks`/`Resources`, deb/AppImage `lib/texoria`, then repo `vendor/`).
  The repo `vendor/` dirs (PDFium and Tectonic) are only searched by a binary under the repo's
  `target/` (`dev_vendor_dir`): a release build must never look at build-machine paths.
  `fetch-*.sh` verify pinned SHA-256s (`scripts/sha256.sh`): update them when bumping versions
  (`release.yml` pins nfpm the same way).
- **tex-syntax**: `tree-sitter` must stay on the same major version that gpui-kit uses.

## Conventions

- All repository content (code, comments, docs, commits) is in English.
- User and developer documentation lives in `docs/` (index: `docs/README.md`); the README stays
  short and links to it. Update the relevant page when a user-visible behavior, setting or
  shortcut changes. The project is MIT-licensed.
- Commit messages follow Conventional Commits (`type(scope): subject`).
