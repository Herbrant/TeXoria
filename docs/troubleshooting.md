# Troubleshooting

## macOS says TeXoria "is damaged" or "cannot be opened"

The app is not notarized by Apple. If you installed it from the `.dmg` by hand, clear the
quarantine flag once:

```sh
xattr -dr com.apple.quarantine /Applications/TeXoria.app
```

The [install script](installation.md#quick-install) and the Homebrew cask do this for you.

## TeX Live is installed but TeXoria uses Tectonic

TeXoria uses latexmk only when it finds it. Check that `latexmk` exists (`latexmk -v` in a
terminal); some distributions package it separately (`latexmk` on Debian and Fedora,
`texlive-binextra` on Arch). Then pick the engine by clicking its name in the status bar. If
latexmk is in an unusual folder, make sure that folder is on the `PATH` TeXoria starts with.

## Tectonic fails on first use

Tectonic downloads the LaTeX packages a document needs the first time it builds it, so the
first run needs an internet connection and takes longer.

## A project's `latexmkrc` is ignored

That is on purpose, until you allow it: see [latexmkrc](preview.md#latexmkrc).

## The Source Control panel is empty

- *Git is not installed*: install Git (on macOS, `xcode-select --install` or Homebrew), then
  reopen the project.
- *Trust this repository to use Git*: see [Trusting a project](git.md#trusting-a-project).

## Push or pull fails with an authentication error

TeXoria never asks for passwords in a terminal: set up a
[credential helper](https://git-scm.com/doc/credential-helpers) or an SSH key so that
`git push` works in a terminal without prompting, and it will work in TeXoria too. For Overleaf,
TeXoria asks for a new token when the old one is rejected.

## The preview stays blank

The preview needs PDFium, which every package bundles. If you built TeXoria yourself, run
`./scripts/fetch-pdfium.sh` (see [Building from source](development/building.md)).

## Linux: the window does not open

TeXoria renders with Vulkan. Make sure a Vulkan driver is installed (`mesa-vulkan-drivers` on
Debian/Ubuntu/Fedora, `vulkan-radeon`, `vulkan-intel` or the NVIDIA driver on Arch).

## Getting logs

Start TeXoria from a terminal to see its log, and set `RUST_LOG` for more detail:

```sh
RUST_LOG=debug texoria path/to/document.tex
```

Please include the relevant lines when you [report a bug](https://github.com/Herbrant/TeXoria/issues).
