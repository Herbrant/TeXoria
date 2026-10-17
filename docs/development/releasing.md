# Releasing

1. Bump `version` in `[workspace.package]` in `Cargo.toml`.
2. Add a `## [X.Y.Z] - YYYY-MM-DD` section to `CHANGELOG.md` with the changes since the previous
   tag (`git log vPREV..HEAD`), grouped under Added, Changed, Security, Fixed, and Build and
   documentation. Write the entries for users and link each one to its commits
   (`[abc1234](https://github.com/Herbrant/TeXoria/commit/abc1234)`), and end the section with the
   `compare/vPREV...vX.Y.Z` link. `scripts/release-notes.sh X.Y.Z` prints what the release notes
   will be.
3. Commit both as `chore(release): vX.Y.Z`.
4. Tag and push:
   ```sh
   git tag vX.Y.Z && git push origin main vX.Y.Z
   ```

`.github/workflows/release.yml` then:

- checks that the tag matches the workspace version (`scripts/version.sh`) and that
  `CHANGELOG.md` has a section for it;
- builds every package on native runners: macOS arm64 and x64, Ubuntu 22.04 x64 and arm64 (the
  oldest supported glibc) and Windows x64;
- publishes the GitHub release with a `SHA256SUMS` file, its signature `SHA256SUMS.minisig`
  (secret `MINISIGN_SECRET_KEY`, trusted comment `texoria X.Y.Z`) and the version's changelog
  section as its notes (`scripts/release-notes.sh`);
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
- The in-app updater installs only what `SHA256SUMS.minisig` vouches for, checked against
  `crates/tex-update/src/texoria.pub`. The secret key is the `MINISIGN_SECRET_KEY` secret
  (an unencrypted minisign key, `minisign -G -W`); the workflow fails without it. Changing the
  key means copies built with the old public key can no longer update themselves.
- PDFium, Tectonic and nfpm versions are pinned with their SHA-256 in `scripts/fetch-*.sh` and
  `release.yml`: update the hashes when bumping them. The PDFium version must match
  pdfium-render's `pdfium_*` feature.
