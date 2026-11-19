# Building from source

## Requirements

- Stable [Rust](https://rustup.rs) (edition 2024).
- On Linux, the libraries GPUI builds against:
  ```sh
  sudo apt install gcc g++ clang libfontconfig-dev libwayland-dev libxkbcommon-x11-dev \
    libx11-xcb-dev libssl-dev libzstd-dev libasound2-dev libvulkan1
  ```
- Optionally a TeX distribution with latexmk, and Git, to try every feature.

## Build and run

```sh
git clone https://github.com/Herbrant/TeXoria.git && cd TeXoria
./scripts/fetch-pdfium.sh       # PDFium into vendor/pdfium (needed by the PDF preview)
./scripts/fetch-tectonic.sh     # Tectonic into vendor/tectonic (optional fallback engine)
cargo run -p texoria -- tests/fixtures/sample/main.tex
```

`cargo run` accepts a `.tex` file. Both fetch scripts verify the SHA-256 of what
they download. The `vendor/` folders are only used by binaries built in the checkout
(`target/`): a release build never looks at paths of the machine it was built on.

## Test and lint

CI runs these on macOS, Linux and Windows; keep them green:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

[Testing](testing.md) explains the helpers, the tests that need TeX, PDFium or Git, coverage,
property tests and fuzzing.

## Disk space

From scratch on macOS, `target/` takes about 4.2 GB for the commands above, 1.9 GB more for a
release build and 2.8 GB for coverage (`target/llvm-cov-target`). Nearly all of it is GPUI, which
`cargo test` builds a second time with its `test-support` feature. The dev profile keeps file and
line numbers for the workspace and no debuginfo for the dependencies (they are optimized anyway):
`CARGO_PROFILE_DEV_DEBUG=true` gives the workspace a debugger's variables, at about 15% more.

Cargo never deletes what it built before: every Rust update, dependency bump or change of
features or flags adds a full copy of the artifacts next to the old ones, and `target/` grows to
tens of GB. Run `cargo clean` after `rustup update` or a `cargo update`.

## Packaging

Packages are built with [cargo-packager](https://github.com/crabnebula-dev/cargo-packager)
after fetching PDFium and Tectonic, and land in `target/dist`:

```sh
cargo install cargo-packager --locked

# macOS: ad-hoc signed .dmg
cargo packager --release -p texoria --formats app && ./scripts/package-macos.sh

# Windows: NSIS installer
cargo packager --release -p texoria --formats nsis

# Linux: AppImage, then .tar.gz, .deb and .rpm (needs nfpm)
cargo packager --release -p texoria --formats appimage
./scripts/package-linux.sh
```

`package-linux.sh` fails if a binary links a shared library that is not in its allowlist: a new
one must also be added to the package dependencies in `crates/texoria/packaging/linux/nfpm.yaml`
and `crates/texoria/packaging/aur/PKGBUILD.in`.

CI does not build in release mode: the release workflow (`.github/workflows/release.yml`) builds
every package each night when `main` has changed, without publishing anything, and can be run by
hand to get the packages as workflow artifacts.

## Other scripts

| Script | Purpose |
|---|---|
| `scripts/gen-icons.sh` | Rasterize `assets/logo/*.svg` into `crates/texoria/packaging` (macOS only) |
| `scripts/fetch-fonts.sh` | Refresh the bundled IBM Plex Mono |
| `scripts/gen-dictionaries.sh <commit>` | Regenerate the spelling dictionary catalog and checksums |
| `scripts/install.sh` | The end-user installer for macOS and Linux |
| `scripts/build-website.sh` | Assemble the website and the documentation into `target/website` (see below) |

## Website

The project's website, [herbrant.github.io/TeXoria](https://herbrant.github.io/TeXoria/), is
the plain HTML/CSS/JS page in `website/` plus this documentation. `scripts/build-website.sh`
copies the page to `target/website` together with what it shares with the app (logo,
screenshots, IBM Plex Mono, icons), and `crates/site-docs` turns every Markdown file of `docs/`
into a page of `target/website/docs` through `website/doc-template.html`; preview it with:

```sh
./scripts/build-website.sh && python3 -m http.server -d target/website
```

The Markdown stays the only source, so write the documentation as usual, for GitHub:

- `docs/README.md` is the index, and its lists of links (one `## Section` each) are the
  sidebar, in the same order: a new page must be listed there.
- Link pages with relative paths (`writing.md#vim-mode`): on the website they lead to the HTML
  page, and headings get the same ids as on GitHub. Links to other files of the repository
  (`../CHANGELOG.md`) go to GitHub.
- A link to a file or heading that does not exist fails the build, and the test
  `cargo test -p site-docs` builds `docs/` too, so CI catches it.

`.github/workflows/pages.yml` publishes the website to GitHub Pages on every push to `main`
that changes it or `docs/` (or by hand). A change to `website/` alone does not run CI.
