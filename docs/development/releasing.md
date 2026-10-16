# Releasing

1. Bump `version` in `[workspace.package]` in `Cargo.toml`.
2. Commit it as `chore(release): vX.Y.Z`.
3. Tag and push:
   ```sh
   git tag vX.Y.Z && git push origin main vX.Y.Z
   ```

`.github/workflows/release.yml` then:

- checks that the tag matches the workspace version (`scripts/version.sh`);
- builds every package on native runners: macOS arm64 and x64, Ubuntu 22.04 x64 and arm64 (the
  oldest supported glibc) and Windows x64;
- publishes the GitHub release with a `SHA256SUMS` file;
- runs `.github/workflows/publish.yml`, which renders `crates/texoria/packaging/{homebrew,aur}/*.in`
  (`scripts/render-manifests.sh`) and pushes the cask to
  [Herbrant/homebrew-tap](https://github.com/Herbrant/homebrew-tap) (secret `HOMEBREW_TAP_TOKEN`)
  and the `texoria-bin` package to the AUR (secret `AUR_SSH_PRIVATE_KEY`).

Notes:

- Pre-release tags (`v1.2.0-rc.1`) are published as pre-releases and skip Homebrew and the AUR.
  The install script also ignores them, unless asked for with `TEXORIA_VERSION`.
- If the Homebrew or AUR step fails, run `publish.yml` by hand with the tag to retry it.
- Running `release.yml` by hand only builds the packages, as workflow artifacts.
- Asset names (`texoria-<version>-<os>-<arch>.<ext>`, with `aarch64`/`x86_64`) are used by the
  Homebrew and AUR templates and by `scripts/install.sh`: keep them stable.
- There is no Apple Developer ID: `package-macos.sh` signs ad hoc, without notarization, and the
  cask and the install script clear the quarantine flag.
- PDFium, Tectonic and nfpm versions are pinned with their SHA-256 in `scripts/fetch-*.sh` and
  `release.yml`: update the hashes when bumping them. The PDFium version must match
  pdfium-render's `pdfium_*` feature.
