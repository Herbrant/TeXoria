# Changelog

Notable changes to TeXoria, newest first. Each release's section is also its GitHub release notes
(`scripts/release-notes.sh`), so every release must have one before it is tagged.

## [0.3.0] - 2026-10-17

### Added

- **Updates from the app.** TeXoria looks for a new release at startup (at most once a day, can be
  turned off in Settings → Updates) and with Check Now or, on macOS, Check for Updates… in the
  TeXoria menu. Copies installed from the `.dmg`, the Windows installer, the AppImage or the install
  script update themselves: "Update to X.Y.Z" in the status bar downloads the release, "Restart to
  Update" saves your files and restarts into it. Nothing is installed unless the release's
  checksums are signed with the project's release key and the download matches them. Homebrew and
  Linux packages are told how to update. ([530a248](https://github.com/Herbrant/TeXoria/commit/530a248), [46847e3](https://github.com/Herbrant/TeXoria/commit/46847e3))
- Only one window updates at a time; the others show "Updating in another window". On Windows,
  TeXoria asks to close its other windows before the installer runs, since it would close them
  without saving. ([46847e3](https://github.com/Herbrant/TeXoria/commit/46847e3))

### Changed

- "Typeset" is now called "Build" everywhere: the button, the menu, the settings page, the
  messages and the agents' `build` tool. ([5729795](https://github.com/Herbrant/TeXoria/commit/5729795))

### Fixed

- On Windows, the buttons of the title bar (light/dark theme, Build, sidebars, Settings) did not
  respond to clicks. ([0e16a53](https://github.com/Herbrant/TeXoria/commit/0e16a53))
- On macOS, a double click on the title bar was handled twice, and clicks on its buttons were
  delayed. ([0e16a53](https://github.com/Herbrant/TeXoria/commit/0e16a53))

### Build and documentation

- Releases publish `SHA256SUMS.minisig`, a minisign signature of the checksums, which the updater
  requires. ([530a248](https://github.com/Herbrant/TeXoria/commit/530a248))
- Wrapped lines of the release notes are joined, since GitHub shows every line break.
  ([82efbbb](https://github.com/Herbrant/TeXoria/commit/82efbbb))

**Full changelog**: [v0.2.0...v0.3.0](https://github.com/Herbrant/TeXoria/compare/v0.2.0...v0.3.0)

## [0.2.0] - 2026-10-17

### Added

- **Coding agents.** A new Agent sidebar works with the coding agents installed on the system
  (Claude Code, Codex, Gemini CLI, GitHub Copilot, Cursor, OpenCode and other Agent Client
  Protocol agents), with your own account: streaming chat with tool calls, plans, permissions,
  sign-in and modes. Rewrite the selection from the context menu, give custom instructions with
  ⌘K / Ctrl+K, or use "Fix with Agent" in the Problems panel. Agents can typeset and read the
  problems and the outline through TeXoria's tools; editors are read-only while an agent works,
  and its changes are reviewed in a diff tab with per-file undo. Agents only start in projects
  you allowed. ([2cab9fc](https://github.com/Herbrant/TeXoria/commit/2cab9fc))
- **Overleaf projects.** "Open from Overleaf…" (welcome screen, File and Git menus) clones a
  project with an authentication token kept in the system keychain. For an Overleaf repository
  the Source Control panel offers Sync (commit everything, pull with rebase, push); Server Pro
  instances can be added in the settings.
  ([fc7e2eb](https://github.com/Herbrant/TeXoria/commit/fc7e2eb),
  [5bca5de](https://github.com/Herbrant/TeXoria/commit/5bca5de))
- A Sync that would add or delete files lists them and asks first, and uses the message typed in
  the commit box. System and editor files (`.DS_Store`, `*.swp`, `.vscode/`…) and package caches
  (`_minted-*`, `svg-inkscape/`) stay out of Overleaf commits.
  ([3a58027](https://github.com/Herbrant/TeXoria/commit/3a58027))
- **Spell checking** in about 90 languages, detected automatically (`% !TeX spellcheck`,
  babel/polyglossia, the text itself, then the system language). Misspelled words are underlined,
  with corrections in the context menu and as code actions; the status bar shows each file's
  language and lets you change it. Dictionaries come from the system or are downloaded on demand
  and verified. ([daadebe](https://github.com/Herbrant/TeXoria/commit/daadebe))
- **Files view** in the sidebar (⇧⌘E / Ctrl+Shift+E): the project tree colored by Git status,
  with create, rename and delete. Open tabs follow renames and close on delete.
  ([55c9833](https://github.com/Herbrant/TeXoria/commit/55c9833))
- **Settings window** (⌘, / Ctrl+, or the gear in the title bar): theme, soft wrap, engine,
  timeouts, live preview, Git options, Overleaf Server Pro instances and trusted projects, which
  can be revoked. Changes apply right away, and the theme choice is now remembered.
  ([8dd10ea](https://github.com/Herbrant/TeXoria/commit/8dd10ea))
- **One-command installer** for macOS and Linux (`scripts/install.sh`): downloads the latest
  release, checks it against `SHA256SUMS` and installs it without root.
  ([fd50661](https://github.com/Herbrant/TeXoria/commit/fd50661))
- User and developer documentation in `docs/`, plus `CONTRIBUTING.md` and `SECURITY.md`.
  ([26a6af9](https://github.com/Herbrant/TeXoria/commit/26a6af9),
  [c032f6b](https://github.com/Herbrant/TeXoria/commit/c032f6b),
  [797e402](https://github.com/Herbrant/TeXoria/commit/797e402))

### Changed

- TeXoria is now licensed under MIT.
  ([0e8ddc8](https://github.com/Herbrant/TeXoria/commit/0e8ddc8))
- Trusting a project for Git no longer lets its `latexmkrc` run: a cloned project's rc file needs
  its own consent. Projects trusted before keep both permissions.
  ([0c00755](https://github.com/Herbrant/TeXoria/commit/0c00755))
- Git trust now names the repository's working tree (shown in the prompt) and covers exactly that
  folder: Git never climbs to an untrusted repository in a parent folder.
  ([d71b6c2](https://github.com/Herbrant/TeXoria/commit/d71b6c2),
  [dfef51a](https://github.com/Herbrant/TeXoria/commit/dfef51a))
- A PDF already in the build folder is shown only for projects opened before; otherwise TeXoria
  typesets from scratch and shows the PDF once a run wrote it.
  ([7122013](https://github.com/Herbrant/TeXoria/commit/7122013))
- Files that are not text open with the system app only if they are documents or pictures;
  anything else is revealed in its folder.
  ([9a81308](https://github.com/Herbrant/TeXoria/commit/9a81308))

### Security

- The engine, Git and the agents no longer run a program planted in the project in place of one
  on `PATH` (empty or relative `PATH` entries, Windows' current-directory lookup).
  ([91515ea](https://github.com/Herbrant/TeXoria/commit/91515ea))
- Agents' reads, writes and undo stay inside the project and outside `.git`; agents cannot write
  `latexmkrc`, and the chat warns when Git's configuration or hooks changed during a turn.
  ([39afa8d](https://github.com/Herbrant/TeXoria/commit/39afa8d))
- The token of TeXoria's tool server comes from the OS random generator, and connections that
  have not shown it get little time and memory.
  ([2382d71](https://github.com/Herbrant/TeXoria/commit/2382d71))
- Overleaf credentials: tokens are never written to the logs, values with control characters are
  refused, only HTTPS Server Pro instances receive a token, and a rejected token is never deleted
  from the keychain. ([dfef51a](https://github.com/Herbrant/TeXoria/commit/dfef51a))

### Fixed

- An error in `settings.toml` no longer resets every setting: the readable values are kept, the
  file is saved as `settings.toml.bak` and you are told.
  ([6919bb4](https://github.com/Herbrant/TeXoria/commit/6919bb4))
- Undoing an agent's turn never deletes a file the checkpoint did not see.
  ([b4e4597](https://github.com/Herbrant/TeXoria/commit/b4e4597))
- Looking for agents no longer hangs when the login shell starts a background job.
  ([3346fc8](https://github.com/Herbrant/TeXoria/commit/3346fc8))
- An agent whose output is not valid UTF-8 keeps working instead of being disconnected or
  blocked. ([1c0147e](https://github.com/Herbrant/TeXoria/commit/1c0147e))
- An agent's typeset request is answered when typesetting is stopped, instead of waiting for the
  30-minute timeout. ([85f6672](https://github.com/Herbrant/TeXoria/commit/85f6672))
- Files larger than 20 MB are refused by every way of opening them (diagnostics, inverse search,
  agent locations), not only by the Files view.
  ([d0e59c3](https://github.com/Herbrant/TeXoria/commit/d0e59c3))
- New files (written by an agent, or brought back by an undo) get the usual permissions instead
  of being readable by their owner only.
  ([f116b45](https://github.com/Herbrant/TeXoria/commit/f116b45))
- Windows: stopping a process no longer flashes a console window.
  ([bc62a8c](https://github.com/Herbrant/TeXoria/commit/bc62a8c))
- The Overleaf branch check compares the upstream's branch, and LaTeX build files stay out of an
  Overleaf Sync. ([dfef51a](https://github.com/Herbrant/TeXoria/commit/dfef51a))
- Publishing to the Homebrew tap and the AUR works on empty repositories and can be retried by
  hand. ([37579a1](https://github.com/Herbrant/TeXoria/commit/37579a1))

### Build and documentation

- README restyled, Homebrew instructions fixed.
  ([5a4bc86](https://github.com/Herbrant/TeXoria/commit/5a4bc86),
  [5ff0757](https://github.com/Herbrant/TeXoria/commit/5ff0757),
  [2a3c254](https://github.com/Herbrant/TeXoria/commit/2a3c254))
- Windows build and test fixes.
  ([f64e3ca](https://github.com/Herbrant/TeXoria/commit/f64e3ca),
  [035ecf4](https://github.com/Herbrant/TeXoria/commit/035ecf4),
  [2fffdd4](https://github.com/Herbrant/TeXoria/commit/2fffdd4),
  [d72aad3](https://github.com/Herbrant/TeXoria/commit/d72aad3))

**Full changelog**: [v0.1.0...v0.2.0](https://github.com/Herbrant/TeXoria/compare/v0.1.0...v0.2.0)

## [0.1.0] - 2026-10-16

First public release.
