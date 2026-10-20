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

`cargo run` accepts a `.tex` file or a folder. Both fetch scripts verify the SHA-256 of what
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
