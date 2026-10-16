<div align="center">

<img src="assets/logo/texoria.svg" alt="TeXoria logo" width="128" height="128">

# TeXoria

**Write LaTeX. See it typeset as you type.**

A fast, native LaTeX editor for macOS, Windows and Linux, written in Rust on
[GPUI](https://www.gpui.rs), the GPU-accelerated UI framework behind Zed.

[![CI](https://img.shields.io/github/actions/workflow/status/Herbrant/TeXoria/ci.yml?branch=main&style=flat-square&label=CI&labelColor=2B2A27)](https://github.com/Herbrant/TeXoria/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Herbrant/TeXoria?style=flat-square&labelColor=2B2A27&color=3E6A8C)](https://github.com/Herbrant/TeXoria/releases/latest)
[![Platforms](https://img.shields.io/badge/macOS%20%C2%B7%20Windows%20%C2%B7%20Linux-3E6A8C?style=flat-square&label=platforms&labelColor=2B2A27)](#install)
[![License](https://img.shields.io/github/license/Herbrant/TeXoria?style=flat-square&labelColor=2B2A27&color=3E6A8C)](LICENSE)

[**Download**](https://github.com/Herbrant/TeXoria/releases/latest) ·
[Features](#features) ·
[Install](#install) ·
[Build from source](#building-from-source)

<br>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/screenshot-ink.png">
  <img src="assets/screenshot-paper.png" alt="TeXoria: LaTeX source on the left, live PDF preview on the right" width="900">
</picture>

</div>

## Why TeXoria

LaTeX editors tend to be either heavyweight IDEs or a text box next to a slow preview.
TeXoria is neither: source and typeset page sit side by side, the PDF updates while you write,
and the whole app is native, GPU-rendered and light on resources. It works out of the box with
no TeX distribution installed, and uses the one you already have when there is one.

## Features

### Writing

- **Context-aware completion** for commands, environments (closing `\end{}` included), `\ref`
  from the project's labels, `\cite` from your `.bib` files (matching title and author too) and
  `\usepackage`.
- **Document outline** across multi-file projects (`\input`, `\include`), following the section
  you are editing.
- **File browser** next to the outline: the project folder as a tree, colored by Git status,
  to open, create, rename and delete files. Images and PDFs open in their usual app.
- **Inline diagnostics**: errors and warnings from the TeX log are underlined in the source and
  collected in the *Problems* panel.

### Preview

- **Live preview** that recompiles as you type without ever blocking the editor. The new PDF
  replaces the old one only once it is rendered, so the preview never flashes.
- **Two-way SyncTeX**: jump from the cursor to the matching spot in the PDF, or ⌘-click
  (Ctrl-click) or double-click in the PDF to jump back to the source line.
- **Hybrid engine**: uses your TeX Live, MacTeX or MiKTeX installation through latexmk
  (pdfLaTeX, XeLaTeX, LuaLaTeX) when available, and the bundled
  [Tectonic](https://tectonic-typesetting.github.io) otherwise.

### Version control

- **Git built in**: word-level diffs (unified or side by side), stage, commit, amend, pull, push
  and fetch, browse history and restore files from older commits, switch and create branches,
  resolve merge conflicts, and clone from GitHub, GitLab or Overleaf.
- **Your Git, your setup**: TeXoria drives the `git` already on your system, so credential
  helpers, SSH keys, commit signing and hooks keep working.
- **Overleaf, offline**: open an Overleaf project, work on it without a connection, and bring
  both sides up to date with one *Sync* (see [Overleaf](#overleaf)).

### Design

- **Paper** and **Ink**: a warm light theme and a dark one. In dark mode the PDF is darkened
  while keeping its colors.
- **Safe with untrusted projects**: a project's `latexmkrc` and Git configuration can run
  arbitrary programs, so each is ignored until you allow it. A project you clone is trusted
  for Git, but its `latexmkrc` still asks first.

## Install

Every package bundles PDFium and Tectonic, so TeXoria works without a TeX distribution. Install
TeX Live (or MacTeX/MiKTeX) to compile with latexmk, and `git` to use source control.

| Platform | Package |
|---|---|
| **macOS** (Apple silicon, Intel) | Homebrew (below), or the `.dmg` |
| **Windows** (x64) | `texoria-<version>-windows-x86_64-setup.exe` |
| **Arch Linux** (x86_64, aarch64) | `yay -S texoria-bin` (or any AUR helper) |
| **Debian, Ubuntu** ≥ 22.04 | `sudo apt install ./texoria_<version>_amd64.deb` (or `_arm64`) |
| **Fedora, openSUSE** | `sudo dnf install ./texoria-<version>-1.x86_64.rpm` (or `.aarch64`) |
| **Other Linux** | `texoria-<version>-linux-<arch>.AppImage`, or the `.tar.gz` (a `/usr` tree) |

All packages are on the [releases page](https://github.com/Herbrant/TeXoria/releases/latest),
together with a `SHA256SUMS` file.

### Homebrew

Homebrew (≥ 7) only loads casks from third-party taps you trust:

```sh
brew tap herbrant/tap
brew trust --cask herbrant/tap/texoria
brew install --cask texoria
```

<details>
<summary><b>About code signing</b></summary>

<br>

TeXoria is not signed with an Apple Developer ID or a Windows code-signing certificate.

- **macOS**: the Homebrew cask clears the quarantine flag for you. If you install from the
  `.dmg`, run this once after copying the app:
  ```sh
  xattr -dr com.apple.quarantine /Applications/TeXoria.app
  ```
- **Windows**: SmartScreen asks for confirmation the first time (*More info* → *Run anyway*).

</details>

## Getting started

Open a `.tex` file or a project folder with ⌘O (Ctrl+O). For a folder, TeXoria opens `main.tex`,
or the first file with a `\documentclass`. The Linux packages also put `texoria` on your `PATH`:

```sh
texoria path/to/thesis/          # or path/to/document.tex
```

Build output goes to a `.texoria/build` directory next to the root file, which is ignored by Git
automatically, so your project stays clean.

### Overleaf

TeXoria works with Overleaf through its
[Git integration](https://docs.overleaf.com/integrations-and-add-ons/git-integration-and-github-synchronization/git-integration),
a premium feature: the project's owner needs a paid plan.

1. Choose *Open from Overleaf…* and paste the project's address from your browser
   (`https://www.overleaf.com/project/…`).
2. Paste an authentication token, created in Overleaf's *Account Settings* under *Git
   Integration*. TeXoria keeps it in the system keychain; tokens expire after a year, and
   TeXoria asks for a new one when Overleaf rejects it.
3. Pick where to put the project.

Edit offline as usual. *Sync* in the Source Control panel commits your changes (with the message
in the commit box, or "Update from TeXoria"), pulls what changed on Overleaf and pushes; you can
also commit with your own messages and Sync afterwards. When files would be added to or deleted
from the project, Sync lists them and asks first. Overleaf has no branches or tags, and its comments and tracked changes do not
travel through Git. The compiler and main document chosen on Overleaf are not part of the project
either: pick them in TeXoria.

LaTeX build files (`*.aux`, `*.log`, …), system and editor files (`.DS_Store`, `*~`, …) and
package caches (`_minted-*`) are kept out of Sync through the project's `.git/info/exclude`, so
they never reach your collaborators. For Overleaf Server Pro,
add your instance to `overleaf_servers`: it must use HTTPS, or TeXoria will not send it the token.

## Configuration

Settings are stored as TOML in `settings.toml`:

| OS | Location |
|---|---|
| macOS | `~/Library/Application Support/app.TeXoria.TeXoria/` |
| Linux | `~/.config/texoria/` |
| Windows | `%APPDATA%\TeXoria\TeXoria\config\` |

| Key | Default | |
|---|---|---|
| `live_preview` | `true` | Recompile automatically after edits |
| `live_delay_ms` | `700` | Pause after the last keystroke before a live compile |
| `soft_wrap` | `true` | Wrap long lines in the editor |
| `compile_timeout_secs` | `300` | Stop a compile after this long (`0`: never) |
| `diff_split` | `false` | Show diffs side by side |
| `pull_rebase` | `false` | Pull with `--rebase` instead of merging |
| `git_timeout_secs` | `120` | Stop a fetch, pull or push after this long (`0`: never) |
| `overleaf_servers` | `[]` | Overleaf Server Pro instances, e.g. `["https://latex.example.org"]` |

## Building from source

Requires stable Rust (edition 2024). On Linux, GPUI also needs:

```sh
sudo apt install gcc g++ clang libfontconfig-dev libwayland-dev libxkbcommon-x11-dev \
  libx11-xcb-dev libssl-dev libzstd-dev libasound2-dev libvulkan1
```

```sh
./scripts/fetch-pdfium.sh      # PDFium into vendor/pdfium (required for the preview)
./scripts/fetch-tectonic.sh    # Tectonic into vendor/tectonic (optional)
cargo run -p texoria -- path/to/document.tex
cargo test --workspace
cargo test -p tex-compile -- --include-ignored   # also exercises Tectonic (downloads TeX files on first run)
```

<details>
<summary><b>Packaging</b></summary>

<br>

```sh
cargo install cargo-packager --locked
cargo packager --release -p texoria --formats app && ./scripts/package-macos.sh   # signed .dmg
cargo packager --release -p texoria --formats nsis       # Windows installer
cargo packager --release -p texoria --formats appimage   # Linux AppImage
./scripts/package-linux.sh   # after a release build: .tar.gz, .deb and .rpm (needs nfpm)
```

Everything lands in `target/dist`. Releases are built by `.github/workflows/release.yml` when a
`v*` tag is pushed.

</details>

### Architecture

TeXoria is a Cargo workspace. Only `texoria` depends on the UI: every other crate is plain logic,
testable without a window.

| Crate | Role |
|---|---|
| [`texoria`](crates/texoria) | GPUI app: window, editor, panels, themes |
| [`tex-compile`](crates/tex-compile) | Engine discovery, background compile queue in isolated processes (debounce, cancellation) |
| [`tex-git`](crates/tex-git) | Git through the system `git` CLI: status, diffs, commit, sync, history, branches |
| [`tex-log`](crates/tex-log) | TeX log parser → diagnostics |
| [`synctex`](crates/synctex) | `.synctex(.gz)` reader, forward and inverse search |
| [`tex-syntax`](crates/tex-syntax) | Tree-sitter grammar, highlight query, semantic index |
| [`tex-project`](crates/tex-project) | Root file detection, include graph, `.bib` files |
| [`tex-complete`](crates/tex-complete) | Context-aware completion with fuzzy ranking |
| [`pdf-view`](crates/pdf-view) | PDF rasterization with PDFium on a dedicated thread |

## Contributing

Issues and pull requests are welcome. Before opening a PR, make sure these pass (CI runs them on
macOS, Windows and Linux):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org)
(`type(scope): subject`).

## License

TeXoria is licensed under the [Apache License 2.0](LICENSE).

The editor typeface is [IBM Plex Mono](https://github.com/IBM/plex) (SIL Open Font License),
bundled in `crates/texoria/assets/fonts` (refresh it with `scripts/fetch-fonts.sh`). Icons are
from [Lucide](https://lucide.dev) (ISC). PDF rendering uses [PDFium](https://pdfium.googlesource.com/pdfium/).

<div align="center">
<br>
<img src="assets/logo/texoria-small.svg" alt="" width="32" height="32">
<br>
<sub>Made with Rust and GPUI</sub>
</div>
