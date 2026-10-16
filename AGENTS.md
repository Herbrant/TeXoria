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

cargo packager --release -p texoria  # .app/.dmg, NSIS, .deb, AppImage into target/dist
./scripts/gen-icons.sh               # assets/logo/*.svg → crates/texoria/packaging (macOS only)
```

CI (`.github/workflows/ci.yml`, macOS/Ubuntu/Windows) runs fmt check, clippy with `-D warnings`,
tests and a release build — keep all four green. `rustfmt.toml` sets `max_width = 100`.

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
         ├─ tex-complete ── tex-project ── tex-syntax
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
  option), and unless `Job::allow_project_rc` is set (the user trusted the project, see
  `Settings::trusted_projects`) latexmk runs with `-norc` plus `-r` for the system/user rc files,
  because a project's `latexmkrc` is arbitrary Perl.
- **texoria** (`workspace.rs` is the main window: outline | editor tabs | PDF pane):
  - Live preview: each buffer change schedules a timer (`settings.live_delay_ms`); on fire it
    re-indexes open buffers (outline/completions follow unsaved text) and, if live preview is on,
    saves and typesets. Debouncing lives in the UI, so the `Compiler` is created with
    `Duration::ZERO`.
  - Compiler events come from the scheduler's thread via a `smol` channel and are applied in a
    `cx.spawn` loop (`on_compile_event`).
  - `pdf_pane.rs` keeps old bitmaps on screen until the new PDF's pages are rasterized (no flash)
    and only materializes pages near the viewport. SyncTeX forward/inverse search goes through the
    `synctex` crate, whose coordinates are PDF points with a top-left origin, y downwards.
  - `latex.rs` registers the tree-sitter LaTeX language with gpui-kit's `LanguageRegistry` and
    bridges `tex-complete` items (byte range + insert text + cursor offset) to the editor;
    the post-insert cursor move is done in `Workspace::on_buffer_changed` via "fixups".
  - Actions/keybindings/menus are declared in `main.rs` (`secondary-*` = ⌘ on macOS, Ctrl elsewhere).
  - The app icon's sources are `assets/logo/texoria.svg` (and `texoria-small.svg` for ≤ 48 px);
    the rasterized PNG/`.icns`/`.ico` in `crates/texoria/packaging` are committed. `build.rs`
    embeds the `.ico` as Windows resource ID 1, which is where GPUI loads the window icon from.
  - Settings are TOML in the OS config dir (`directories::ProjectDirs("app","TeXoria","TeXoria")`).
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
  `fetch-*.sh` verify pinned SHA-256s (`scripts/sha256.sh`): update them when bumping versions.
- **tex-syntax**: `tree-sitter` must stay on the same major version that gpui-kit uses.

## Conventions

- All repository content (code, comments, docs, commits) is in English.
- Commit messages follow Conventional Commits (`type(scope): subject`).
