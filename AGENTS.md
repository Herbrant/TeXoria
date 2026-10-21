# AGENTS.md

This file provides guidance to AI coding agents when working with code in this repository.

TeXoria is a LaTeX editor in Rust (edition 2024, stable toolchain) built on GPUI via the
`gpui-kit` crate (pinned with `=`, pre-1.0 — check its API in `~/.cargo/registry` rather than
assuming upstream GPUI/Zed APIs).

## Commands

```sh
./scripts/fetch-pdfium.sh            # PDFium → vendor/pdfium (needed for the PDF preview; CI runs it)
./scripts/fetch-tectonic.sh          # Tectonic → vendor/tectonic (optional fallback engine)
cargo run -p texoria -- path/to/document.tex   # also accepts a directory (tex_project::main_file picks its document)

cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p tex-log                          # one crate
cargo test -p synctex inverse                  # tests matching a name filter
cargo test -p tex-compile -- --include-ignored # also runs the Tectonic e2e test (downloads TeX bundle first time)
cargo llvm-cov --workspace --html              # coverage; CI checks scripts/coverage-thresholds.tsv
cd fuzz && cargo +nightly fuzz run tex_log     # fuzz targets (fuzz/, not a workspace member)

cargo packager --release -p texoria --formats app && ./scripts/package-macos.sh  # ad-hoc signed .dmg
cargo packager --release -p texoria --formats nsis|appimage   # Windows installer / AppImage
./scripts/package-linux.sh           # Linux, after a release build: .tar.gz, .deb, .rpm (nfpm)
./scripts/gen-icons.sh               # assets/logo/*.svg → crates/texoria/packaging (macOS only)
```

CI (`.github/workflows/ci.yml`, on every release platform: macOS arm64/x64, Linux x64/arm64,
Windows) runs fmt check, clippy with `-D warnings`, the tests under cargo-llvm-cov with a minimum
line coverage per crate (`scripts/coverage-check.sh`, `scripts/coverage-thresholds.tsv`: raise
a minimum when tests improve, never lower it), the Tectonic end-to-end test included — keep
them green. The release build and the packaging are checked by `release.yml`, which runs every
night on main (if it changed since the last nightly run) and then only builds artifacts. Code compiled only on Windows or only on Unix (cfg'd tests and their
helpers) is linted on that system only, so check it with care. `.github/workflows/fuzz.yml` runs
the cargo-fuzz targets of `fuzz/` (nightly). `rustfmt.toml` sets `max_width = 100`.

## Releasing

Bump `version` in `[workspace.package]` (`Cargo.toml`), add the version's section to
`CHANGELOG.md`, commit both as `chore(release): vX.Y.Z`, then
`git tag vX.Y.Z && git push origin main vX.Y.Z`. The changelog section is the release notes
(`scripts/release-notes.sh`; the workflow fails before building without one): group the changes
since the previous tag (`git log vPREV..HEAD`) under Added / Changed / Security / Fixed / Build and
documentation, written for users, each entry linking its commits
(`[abc1234](https://github.com/Herbrant/TeXoria/commit/abc1234)`), and end with the
`compare/vPREV...vX.Y.Z` link. `.github/workflows/release.yml` checks that the
tag matches the version (`scripts/version.sh`), builds on native runners (macOS arm64/x64,
Ubuntu 22.04 x64/arm64 for an old enough glibc, Windows x64), publishes the GitHub release with
`SHA256SUMS` and `SHA256SUMS.minisig` (signed with the secret `MINISIGN_SECRET_KEY`, trusted
comment `texoria X.Y.Z`, checked against `crates/tex-update/src/texoria.pub`), and renders `crates/texoria/packaging/{homebrew,aur}/*.in`
(`scripts/render-manifests.sh`) into the cask pushed to `Herbrant/homebrew-tap`
(secret `HOMEBREW_TAP_TOKEN`) and the AUR `texoria-bin` package (secret `AUR_SSH_PRIVATE_KEY`).
Pre-release tags (`v1.2.0-rc.1`) skip Homebrew and the AUR; a manual or nightly run only builds
artifacts, and the nightly run saves the cache that a tag's build starts from.
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

Tests that depend on the environment call `test_support::skip("<TOOL>", why)` and return early
(they pass trivially) when the tool is missing, unless `TEXORIA_REQUIRE_<TOOL>=1` (set in CI for
`TEX`, `TECTONIC`, `PDFIUM`, `GIT`), where it is a failure: `tex-compile/tests/real_engine.rs`
needs a TeX engine, pdf-view's tests need `vendor/pdfium`. Shared fixtures live in
`tests/fixtures/` (`test_support::fixtures()`); tests copy a fixture to a temporary folder before
building or editing it. The sample project must compile with **zero diagnostics** (asserted by
the e2e test; it is also the README screenshot). `crates/test-support` (dev-dependency only, it
depends on no workspace crate) also has the `|` cursor-marker helper, a loopback HTTP server for
the network clients (`http`, with form decoding and `closed_url`), an isolated Git
(`git::Isolated`), file helpers (`write`, `write_files`, `copy_dir`, `executable`), `vendor`/
`pdfium` and polling (`within`, `eventually`): use them rather than writing another copy. Processes are faked by binaries built with
the tests (`tex-compile`'s `fake-engine`, `tex-agent`'s `fake-acp-agent`, through
`CARGO_BIN_EXE_*`), and invariants are property-tested with proptest (commit
`proptest-regressions/`). Never use `std::env::set_var` in tests: inject the value instead. A
target (test, bin, example) must not share a dependency's library name: cargo-llvm-cov's
`cargo clean -p` would delete that library and rebuild all that depends on it on every run
(`scripts/check-target-names.sh`, run by CI). See `docs/development/testing.md`.

## Architecture

Cargo workspace, `crates/*`. Only `texoria` depends on the UI; every other crate is plain,
window-free logic with its own unit tests. Dependency flow:

```
texoria ─┬─ tex-compile ── tex-log
         ├─ tex-agent (ACP client, MCP server) ── tex-project, tex-log
         ├─ tex-git (system `git` CLI)
         ├─ tex-complete ── tex-project ── tex-syntax
         ├─ tex-spell ── tex-syntax
         ├─ tex-cite (DOI, arXiv, Crossref) ── tex-project
         ├─ tex-vim (Vim emulation on plain text)
         ├─ synctex
         ├─ pdf-view (PDFium)
         └─ tex-update (self-update)
```

`tex-sys` is a leaf crate that most of them use (not drawn): what they share about the system.

- **tex-sys** (no workspace dependency): `process` starts a child in a group of its own
  (`new_session` for git and agents, `own_group` for engines, texdoc and LanguageTool) without a
  console window, and `kill_tree` stops all it started; `paths` has `plain`/`canonical` (Windows'
  `\\?\` paths), `lexical` (`.`/`..`), `home`, `absolute_path` (the `PATH` programs run with:
  absolute dirs only, never empty on Unix) and `find_program`; `download` has `copy_verified`
  (SHA-256) and the `USER_AGENT` every HTTP client sends. Code compiled only on one system lives
  here once: change it with care.
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
  `.texoria`), which drives the review patch and the undo. It walks the project on its own
  (never the Files view's capped scan) and knows when it could not see everything: undo only
  deletes an added file when the walk was complete and the file is newer than the checkpoint.
  It also watches `.git/config` and the hooks (`git_changes`), which the review does not show.
  Through `fs/*` an agent reaches only the project, outside `.git`, and cannot write
  `latexmkrc` (`agent_panel::writable`). `mcp.rs` serves TeXoria's tools (`build`,
  `problems`, `outline`, `pdf_page` with a PNG, `pdf_text`, `bib_search`, `word_count`) on a
  loopback port; agents start `texoria --mcp-bridge` (handled in
  `main.rs` before the UI), which relays stdio there after a random token (`getrandom`; before
  it, a connection gets little time, memory and company).
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
  catalog dictionary loads (keep it that way when bumping the commit). `extract::count` counts
  words like texcount (text, headings, captions, formulas). `grammar` talks to a LanguageTool
  server: the source becomes LanguageTool's annotated text (`annotate`: prose runs from
  `extract::prose_runs`, markup read as what it stands for), split at paragraphs (`chunks`) so
  that the app caches findings per chunk; offsets come back in UTF-16 units.
- **tex-project** also holds the checks run as you type (`check::run`: undefined references and
  citations, duplicate labels, missing includes/pictures, `.bib` flaws; the sample project must
  have none) and the project-wide search (`search`, regex-based, over `files::scan`'s text files
  with the open buffers' text). `tex-syntax`'s index records byte ranges and rows of labels,
  references, citations, includes, commands and environments, for these and for navigation.
- **tex-complete** also answers "what is here" (`nav::symbol_at`), where it is defined
  (`nav::definition`) and what to show on hover (`nav::hover`), and completes paths.
- **tex-cite**: BibTeX from doi.org (content negotiation), arXiv's Atom API, or a Crossref search;
  `make_key`/`normalize` name and format the entry. Network only on an explicit action; tests use
  fixtures (`TEXORIA_CITE_ONLINE=1` runs the online one).
- **tex-vim**: Vim on plain text: keys in, `Action`s (edits in byte offsets, cursor, selection,
  undo, yank…) out; `.` replays the recorded keys on a copy of the text and returns one edit.
- **texoria** (`workspace.rs` is the main window: outline or files | editor tabs | PDF pane):
  - `workspace.rs` holds the `Workspace` type, its construction and shared helpers; its methods
    are split by concern into `workspace/*.rs` (`impl Workspace` blocks, `pub(super)`: building,
    indexing, buffers, agent, mcp_tools, spelling, sidebar, tabs, chrome…), its `Render` impl
    and actions in `workspace/view.rs`.
  - Shared pieces: `dialogs.rs` (`open_form`, `prompt_text` for a one-line question,
    `confirm_danger`), `ui.rs` (`icon_button`, `banner`, `spinning`, `sidebar`), `tasks.rs`
    (`Background::{background_then, background_then_in, forward}` on an entity's `Context`),
    `underlines.rs` (wavy underlines with notes and their correction code actions),
    `text.rs` (`plural`, `thousands`) and `paths.rs`.
  - Tests run headless on GPUI's test platform (`#[gpui::test]`, dev-dependency `gpui-pre` with
    `test-support`; gpui-kit's own feature would resolve its `gpui-fast` engine, whose pins
    conflict). `testing.rs` opens real workspaces from a `Startup` (settings, engines, PDFium
    folders) instead of `Workspace::new`'s `Startup::detect`. Under `cfg(test)`
    `Settings::load` never reads the user's file; `Settings::load_from` saves where it read.
    Test workspaces start no MCP tool server, find no machine dictionaries and no installed
    agents (`AgentPanel`'s detector is replaced), and the PDF pane's events are pumped by hand:
    real threads waking GPUI's deterministic scheduler make it panic, so a test that runs real
    processes calls `testing::allow_threads` and waits with `testing::wait_until`.
    `testing::fake_program` finds tex-compile's `fake-engine` and tex-agent's `fake-acp-agent`
    next to the test executable (built by `cargo test --workspace`; with cargo-llvm-cov, in
    `<target>/llvm-cov-target/debug`), and skips the test unless `TEXORIA_REQUIRE_FAKES=1`.
    Paths go through `paths::canonical` (resolved, without Windows' `\\?\` prefix).
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
    Files that are not text (`files::looks_like_text`) open with the system app only if they
    are documents or pictures (`files::safe_to_open`), since a clone's files are not
    quarantined; anything else is revealed in its folder.
  - `diff_view.rs` is a single "preview" diff tab built on `gpui_kit::component::diff`
    (`DiffFile::parse` of git's patch, `parse_conflicts` for merge conflicts). It does not support
    combined diffs, so commits are shown against their first parent. The gpui-kit editor has no
    gutter-marker API, so there are no change markers in the editor itself.
  - Live preview: each buffer change schedules a timer (`settings.live_delay_ms`); on fire it
    re-indexes open buffers (outline/completions follow unsaved text) and, if live preview is on,
    saves and builds. Debouncing lives in the UI, so the `Compiler` is created with
    `Duration::ZERO`. `Workspace::reindex` parses only the buffers whose text changed since
    (`Buffer::indexed`, a hash), on the background executor, one re-index at a time (those asked
    for meanwhile make one more); the same tree gives the word count. Whatever changes the project
    from outside a buffer's text must clear `indexed` (`reindex_all`, `reload_bibs`), or an
    unchanged buffer keeps a stale index.
  - Compiler events come from the scheduler's thread via a `smol` channel and are applied in a
    `cx.spawn` loop (`on_compile_event`).
  - A PDF in the build folder is shown only for projects opened before (`recent`) or once a
    run wrote it (`Output::pdf_written`); until then a full build is forced (latexmk `-g`), so
    build files shipped with a project are never handed to PDFium.
  - `pdf_pane.rs` keeps old bitmaps on screen until the new PDF's pages are rasterized (no flash)
    and only materializes pages near the viewport. Pages are laid out in whole device pixels and
    rasterized at the display's exact scale (`scale_milli`, once it stops changing), so a bitmap
    is drawn 1:1: resampled, it looks grainy on a low-resolution display. SyncTeX forward/inverse
    search goes through the `synctex` crate, whose coordinates are PDF points with a top-left
    origin, y downwards.
    The pane takes the focus on click: in its key context (`PdfPane`) ⌘= / ⌘- / ⌘0 zoom the PDF,
    elsewhere they resize the editor's text (`Settings::editor_font_size`). Links, page text
    (selection) and search hits are `pdf_view::Query`s answered on the worker, per document
    generation. `preview_window.rs` is the PDF's own window (`TogglePreviewWindow`): the same
    `PdfPane` entity moves there (`PdfPane::leave_window` hands back the bitmaps for the old
    window to free from its atlas, and keeps the reader's place for the new layout), and it
    passes the main window's actions on through the workspace's focus handle. The workspace's
    PDF actions that open a field (find, go to page) run in the window showing the pane
    (`Workspace::in_pdf_window`). Its placement (display UUID, bounds, full screen) is kept in
    `Settings::preview_window`. Never read the `Workspace` while another window draws: GPUI
    then binds the entity to that window, and its `subscribe_in` callbacks run there.
  - `agent_panel.rs` is the Agent sidebar (alternates with the outline). Agents only start in
    projects allowed in `Settings::agent_projects` (exact directories, like trust), and the
    agents are looked for on first use. A turn emits `AgentPanelEvent::Busy`: the workspace
    saves and makes every editor read-only until it ends, reloading from disk as tool calls
    complete. Messages and the context-menu edits (`RewriteSelection`, `EditWithAgent`) go
    through `AgentPanelEvent::Request` so the buffers are saved first; the workspace adds the
    open file and the selection to chat messages. The review is a `DiffSource::Agent` diff tab
    (no repository) with per-file undo. `agent_tools.rs` forwards the MCP calls to
    `Workspace::on_tool_call`; a `build` call is answered by the first run that starts after
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
    `LatexNavigation` is the definition and hover provider: a target in another file is a
    `texoria:definition/<n>` link that its `ShowDocumentHandler` turns into `OpenLocation`;
    `HoverNote`s add the checks' and the grammar's messages to the hover.
  - `checks.rs` and `grammar.rs` underline, like spelling, the live checks (run after each
    re-index) and LanguageTool's findings (`Grammar` starts `languagetool-server` when allowed and
    stops its process group on quit). Their hover messages are matched to underlines by text and
    position (`underlines::Notes`), since deleted ranges disappear from a decoration collection.
  - `search_panel.rs` is the Search view of the sidebar; Replace All edits open buffers through
    the editor and writes the others. `edit.rs` holds the comment toggling and tex-fmt formatting.
  - `vim.rs`: with `Settings::vim_mode`, `cx.intercept_keystrokes` gives the active editor's keys
    to `tex_vim` and stops them when Vim takes them; the block cursor is a fill
    `RangeDecoration` (the editor has no cursor shapes).
  - Actions/keybindings/menus are declared in `main.rs` (`secondary-*` = ⌘ on macOS, Ctrl elsewhere).
    On macOS the app outlives its window: `reopen` (the Dock icon) opens the last project in a
    new one. Closing the main window also closes the Settings and PDF windows. The Finder sends
    the documents it opens as URLs (`on_open_urls`, `open_document`), not as arguments. The
    `.tex` type is declared per system: `packaging/macos/Info.plist` (merged by cargo-packager;
    `org.tug.tex` is imported for Macs without a TeX distribution), the `.desktop` file's
    `MimeType`, and the NSIS `preinstall-section` in `Cargo.toml` (cargo-packager's own
    `file-associations` are not used: its uninstaller leaves them).
  - The app icon's sources are `assets/logo/texoria.svg` (and `texoria-small.svg` for ≤ 48 px);
    the rasterized PNG/`.icns`/`.ico` in `crates/texoria/packaging` are committed. `build.rs`
    embeds the `.ico` as Windows resource ID 1, which is where GPUI loads the window icon from.
  - Settings are TOML in the OS config dir (`directories::ProjectDirs("app","TeXoria","TeXoria")`).
    `Workspace` owns them; `settings_window.rs` is the Settings window (⌘,, a second window built
    on gpui-kit's `setting` component) and writes every change through
    `Workspace::update_settings`, which saves and applies only what changed (engine, timeouts,
    soft wrap of open tabs, Git options, trust, theme). Its setters update the main window, so
    they must never run from inside it. It shows a copy of what it reads (`Shown`), refreshed by
    observers: it must not read the workspace while it draws (see `preview_window.rs`).
    Themes ("Paper" light, "Ink" dark) are in `assets/themes.json`; icons/fonts are embedded via
    `assets.rs`.
  - Files (sources and settings) are written through `atomic::write` (temp file + rename), and
    dirty buffers are saved on quit/window close.
- **pdf-view**: PDFium is not thread-safe and pdfium-render binds it once per process: the first
  `Renderer` binds it in a process-wide static that later ones reuse, each renderer's worker
  thread owns its documents, and calls into PDFium run one at a time; the UI sends the wanted
  page set in priority order and gets BGRA bitmaps back. PDFs are loaded from memory, never from
  disk, because an open handle on Windows would block the engine from overwriting the PDF. The
  PDFium version in `fetch-pdfium.sh` must match pdfium-render's `pdfium_*` feature. PDFium is
  located at runtime via `pdfium_dirs()` in `workspace.rs` (next to the exe, bundle
  `Frameworks`/`Resources`, deb/AppImage `lib/texoria`, then repo `vendor/`).
  The repo `vendor/` dirs (PDFium and Tectonic) are only searched by a binary under the repo's
  `target/` (`dev_vendor_dir`): a release build must never look at build-machine paths.
  `fetch-*.sh` verify pinned SHA-256s (`scripts/sha256.sh`): update them when bumping versions
  (`release.yml` pins nfpm the same way).
- **tex-update**: updates of copies no package manager looks after. `InstallKind::classify`
  (pure, over paths) tells them apart: a writable `TeXoria.app` (`.dmg`/install.sh), the NSIS
  per-user install (`uninstall.exe` next to the exe), `$APPIMAGE`, install.sh's Linux prefix
  (`bin/texoria` + `lib/texoria`) update themselves; Homebrew (a `Caskroom/texoria` folder),
  `/usr` and other `/opt` installs (deb/rpm/AUR), translocated or read-only apps only get
  instructions; a binary in `target/{debug,release}` is `Dev` and never checks. Under `/usr`
  everything is a distro package; under `/usr/local` and `/opt` only a writable `bin/texoria` +
  `lib/texoria` layout (install.sh's) updates itself. The latest version comes from the
  `releases/latest` redirect (no API). Nothing is installed unless `SHA256SUMS` verifies against
  the embedded minisign key with the trusted comment `texoria <version>`, and the download
  matches it. The download goes to a `.texoria-update-*` folder next to its target (same file
  system), removed if anything fails; the install renames the new copy over the old one
  (macOS: `hdiutil` + `ditto` + `codesign --verify` first), and `Relaunch` waits for the app's
  PID to exit before starting it again. Windows runs `setup.exe /P /R /NS /D=<dir>` instead,
  from a PowerShell script (`apply.rs`) that waits for the app to exit and asks to close the other
  `texoria.exe` windows first, since the NSIS template's passive mode kills them without saving.
  `Lock` (an OS file lock in the temp folder, released when its process dies) lets one running
  copy at a time download and install; the others show `UpdateState::Elsewhere` and poll it.
  Asset names are those of `release.yml`. `TEXORIA_UPDATE_URL` replaces the releases URL (and,
  in debug builds only, `TEXORIA_UPDATE_KEY` the key) to test against a local server.
  In the app, `updater.rs` holds the state shown in the status bar; the workspace saves
  everything before `Updater::install` and quits on `UpdaterEvent::Installed`.
- **tex-syntax**: `tree-sitter` must stay on the same major version that gpui-kit uses.

## Conventions

- All repository content (code, comments, docs, commits) is in English.
- User and developer documentation lives in `docs/` (index: `docs/README.md`); the README stays
  short and links to it. Update the relevant page when a user-visible behavior, setting or
  shortcut changes. The project is MIT-licensed.
- Commit messages follow Conventional Commits (`type(scope): subject`).
