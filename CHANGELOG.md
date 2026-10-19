# Changelog

Notable changes to TeXoria, newest first. Each release's section is also its GitHub release notes
(`scripts/release-notes.sh`), so every release must have one before it is tagged.

## [0.4.2] - 2026-10-19

### Added

- `texoria --help` and `texoria --version` on the command line; unknown options are refused
  instead of being opened as files (macOS's `-psn_` argument no longer shows "Cannot open").
  ([f87eb44](https://github.com/Herbrant/TeXoria/commit/f87eb44))

### Security

- An agent can no longer write `latexmkrc` through a link to it, or on Windows under another
  spelling of its name (`latexmkrc.`, a trailing space, `::$DATA`); undoing an added file never
  follows a folder out of the project and removes an added link, not what it points to.
  ([5da7f0b](https://github.com/Herbrant/TeXoria/commit/5da7f0b))
- The editor really stays read-only while an agent works: it became editable again as soon as it
  was drawn. ([0eac005](https://github.com/Herbrant/TeXoria/commit/0eac005))
- Changes an agent makes to the Git configuration and hooks are also noticed in worktrees,
  submodules and with `core.hooksPath`. ([7d4dad2](https://github.com/Herbrant/TeXoria/commit/7d4dad2))
- When an agent session ends, the bridges still connected to TeXoria's tools are hung up on.
  ([92540e2](https://github.com/Herbrant/TeXoria/commit/92540e2))
- Discarding untracked files in Source Control stays inside the working tree: on Windows a path
  with `\` or a drive could leave it, and `.git/config` or a file behind a linked folder could be
  deleted. ([ee881ad](https://github.com/Herbrant/TeXoria/commit/ee881ad))
- The engine never runs with an empty `PATH`, which Unix reads as the project folder; one odd
  entry no longer empties it. ([812a186](https://github.com/Herbrant/TeXoria/commit/812a186),
  [d24713c](https://github.com/Herbrant/TeXoria/commit/d24713c))
- A LanguageTool server is "on this computer" only when its host really is a loopback address
  (not `localhost:pw@example.com` or an IPv4-mapped address).
  ([f2a6630](https://github.com/Herbrant/TeXoria/commit/f2a6630))
- Dictionary codes cannot point outside the dictionary folders.
  ([3a9beee](https://github.com/Herbrant/TeXoria/commit/3a9beee))
- Hovers escape the names and paths they show and link only real package names.
  ([cdd28e6](https://github.com/Herbrant/TeXoria/commit/cdd28e6))
- Updates stop on a `SHA256SUMS` that gives one file two hashes, install links in a Linux
  tarball update as links instead of copying (or writing through) what they point to, and never
  remove a download another running copy is using.
  ([ab5e381](https://github.com/Herbrant/TeXoria/commit/ab5e381),
  [34efbbc](https://github.com/Herbrant/TeXoria/commit/34efbbc),
  [32cbb24](https://github.com/Herbrant/TeXoria/commit/32cbb24))
- A settings file that cannot be read is left alone, instead of being overwritten with the
  defaults (losing the trusted folders and the personal dictionary).
  ([8c95cc5](https://github.com/Herbrant/TeXoria/commit/8c95cc5))

### Fixed

- **PDF preview.** Pages are drawn pixel for pixel at the display's exact scale, so they are
  sharp on low-resolution displays too ([000a274](https://github.com/Herbrant/TeXoria/commit/000a274));
  a PDF's first pages show at once instead of staying blank, Enter in the find bar no longer
  jumps back to the first hit, and a rebuild keeps the current hit
  ([224bb80](https://github.com/Herbrant/TeXoria/commit/224bb80)); a presentation whose page
  disappears in a rebuild goes to the last page left
  ([d99d67d](https://github.com/Herbrant/TeXoria/commit/d99d67d)).
- **macOS.** Clicking the Dock icon after closing the window opens it again; closing the main
  window also closes the Settings and presentation windows
  ([816f21d](https://github.com/Herbrant/TeXoria/commit/816f21d)). Installing an update retries
  mounting the disk image, and a failing tool says why
  ([27e50b0](https://github.com/Herbrant/TeXoria/commit/27e50b0)).
- **Windows.** Paths are shown and saved in their usual spelling, not as `\\?\C:\…`
  ([6090160](https://github.com/Herbrant/TeXoria/commit/6090160)); Git colors show in the Files
  view, renaming a file to its own name works, and links to folders can be deleted
  ([ac28c32](https://github.com/Herbrant/TeXoria/commit/ac28c32),
  [d3cc717](https://github.com/Herbrant/TeXoria/commit/d3cc717),
  [b2c115e](https://github.com/Herbrant/TeXoria/commit/b2c115e)); names Windows cannot hold are
  refused when creating or renaming files ([825bf45](https://github.com/Herbrant/TeXoria/commit/825bf45)).
- **Build messages.** Errors and warnings land on the right file and line more often: pdfTeX and
  LuaTeX logs with accented text are unwrapped correctly
  ([fc87442](https://github.com/Herbrant/TeXoria/commit/fc87442)), the text of overfull and
  underfull boxes no longer confuses the file being read
  ([779f65a](https://github.com/Herbrant/TeXoria/commit/779f65a)), Tectonic's `\input`s without
  extension are followed ([a209f01](https://github.com/Herbrant/TeXoria/commit/a209f01)), font
  warnings keep their line ([54b6c4f](https://github.com/Herbrant/TeXoria/commit/54b6c4f)) and
  `file:line:error` messages take the right file
  ([14b34d9](https://github.com/Herbrant/TeXoria/commit/14b34d9)). A cancelled build no longer
  reads a stale log ([9b79872](https://github.com/Herbrant/TeXoria/commit/9b79872)), texdoc no
  longer hangs on long output ([254bae2](https://github.com/Herbrant/TeXoria/commit/254bae2)), and
  a root file given without a folder builds ([8944082](https://github.com/Herbrant/TeXoria/commit/8944082)).
- **Problems and navigation.** The live checks underline their findings as soon as a project
  opens ([cb6604e](https://github.com/Herbrant/TeXoria/commit/cb6604e)); files and bibliographies
  the project no longer includes are forgotten ([3dcb756](https://github.com/Herbrant/TeXoria/commit/3dcb756));
  `.bib` entries are read correctly with `#` concatenations, quotes in braces or a missing
  delimiter ([bbcedb5](https://github.com/Herbrant/TeXoria/commit/bbcedb5),
  [1455069](https://github.com/Herbrant/TeXoria/commit/1455069)); more DOI spellings are accepted
  ([fbcbdaa](https://github.com/Herbrant/TeXoria/commit/fbcbdaa)); `% !TEX root` is read in any
  case ([bc3a436](https://github.com/Herbrant/TeXoria/commit/bc3a436)); captions inside `center`
  or `minipage`, `\bibitem` keys with braces, comments in key lists and includes on a section's
  line are handled ([3bf8e05](https://github.com/Herbrant/TeXoria/commit/3bf8e05),
  [19127f3](https://github.com/Herbrant/TeXoria/commit/19127f3),
  [0adeed1](https://github.com/Herbrant/TeXoria/commit/0adeed1),
  [dcc2f60](https://github.com/Herbrant/TeXoria/commit/dcc2f60)); `\href` arguments are not taken
  for labels, `%` after `\\` is a comment, and environments in comments are not closed
  ([406edee](https://github.com/Herbrant/TeXoria/commit/406edee),
  [2938c4f](https://github.com/Herbrant/TeXoria/commit/2938c4f),
  [888e557](https://github.com/Herbrant/TeXoria/commit/888e557)).
- **Project search.** Replace All changes every match even when the list stopped at 5,000
  ([9c37778](https://github.com/Herbrant/TeXoria/commit/9c37778)); a stopped search no longer
  shows its results under an empty field ([ab3f8a5](https://github.com/Herbrant/TeXoria/commit/ab3f8a5));
  whole-word queries with symbols and matches in leading whitespace are found and highlighted
  ([f199898](https://github.com/Herbrant/TeXoria/commit/f199898),
  [6058fe9](https://github.com/Herbrant/TeXoria/commit/6058fe9),
  [e7e6f2a](https://github.com/Herbrant/TeXoria/commit/e7e6f2a)).
- **SyncTeX.** Forward search picks the right file when two inputs share a name, and pages,
  `..` paths and odd numbers in a SyncTeX file are read correctly
  ([bdcf88a](https://github.com/Herbrant/TeXoria/commit/bdcf88a),
  [2f0b9d9](https://github.com/Herbrant/TeXoria/commit/2f0b9d9),
  [51f1808](https://github.com/Herbrant/TeXoria/commit/51f1808),
  [b1fdb7b](https://github.com/Herbrant/TeXoria/commit/b1fdb7b)).
- **Vim mode.** `J`, `~`, `u`, `U`, `r`, `p` and `P` act on the visual selection
  ([a601720](https://github.com/Herbrant/TeXoria/commit/a601720)); `cw` on a word's last
  character and `i<`/`a<` behave like Vim's ([2a0633b](https://github.com/Herbrant/TeXoria/commit/2a0633b),
  [fc85c38](https://github.com/Herbrant/TeXoria/commit/fc85c38)); huge counts on `p` and `.` no
  longer freeze the editor ([33318c5](https://github.com/Herbrant/TeXoria/commit/33318c5)); and
  several crashes with accented characters, `i$` outside a formula or a selection left past the
  text's end are gone ([a9af31b](https://github.com/Herbrant/TeXoria/commit/a9af31b),
  [ce90134](https://github.com/Herbrant/TeXoria/commit/ce90134),
  [bde9c0d](https://github.com/Herbrant/TeXoria/commit/bde9c0d),
  [334d5c3](https://github.com/Herbrant/TeXoria/commit/334d5c3),
  [96e5975](https://github.com/Herbrant/TeXoria/commit/96e5975)).
- **Agents.** A message sent before the agents were found is no longer dropped
  ([4f50fb5](https://github.com/Herbrant/TeXoria/commit/4f50fb5)); a stopped build is reported to
  the agent as stopped, not failed ([7087ea5](https://github.com/Herbrant/TeXoria/commit/7087ea5));
  the last lines an agent writes before exiting are kept
  ([9f29f0c](https://github.com/Herbrant/TeXoria/commit/9f29f0c)); the tool server no longer spins
  at full CPU after a failed accept, nor drops quiet connections on Windows
  ([cbc65d6](https://github.com/Herbrant/TeXoria/commit/cbc65d6),
  [7be01b1](https://github.com/Herbrant/TeXoria/commit/7be01b1)).
- **Citations.** Old arXiv identifiers (`math.GT/0309136`, `solv-int/9901001v2`) are recognized,
  `%`, `&`, `#` and character references in arXiv titles come out right, generated keys never
  clash, and BibTeX that would come out unbalanced is refused
  ([d725707](https://github.com/Herbrant/TeXoria/commit/d725707),
  [e97e17e](https://github.com/Herbrant/TeXoria/commit/e97e17e),
  [5fe5841](https://github.com/Herbrant/TeXoria/commit/5fe5841),
  [83d3777](https://github.com/Herbrant/TeXoria/commit/83d3777),
  [23e0c99](https://github.com/Herbrant/TeXoria/commit/23e0c99),
  [8df5e49](https://github.com/Herbrant/TeXoria/commit/8df5e49)).
- **Spelling and grammar.** Dictionaries with one-character long flags load fully, spelling no
  longer panics on a stale range, and LanguageTool's findings stay in their paragraph with a
  bounded cache ([fd08032](https://github.com/Herbrant/TeXoria/commit/fd08032),
  [62515fc](https://github.com/Herbrant/TeXoria/commit/62515fc),
  [2437a69](https://github.com/Herbrant/TeXoria/commit/2437a69)).
- **Updates.** Copies installed with `install.sh` under `/usr/local` or `/opt` update themselves,
  and a failed tarball update removes the new libraries
  ([31be767](https://github.com/Herbrant/TeXoria/commit/31be767),
  [8e9c229](https://github.com/Herbrant/TeXoria/commit/8e9c229)).
- Files ending in `.TEX` are highlighted as LaTeX, a diff shows again after a failed reload, and
  root detection skips huge `.tex` files ([ffa7fe9](https://github.com/Herbrant/TeXoria/commit/ffa7fe9),
  [78a9265](https://github.com/Herbrant/TeXoria/commit/78a9265),
  [5ec4c8b](https://github.com/Herbrant/TeXoria/commit/5ec4c8b)).

### Build and documentation

- A much larger test suite: a shared `test-support` crate
  ([2408ede](https://github.com/Herbrant/TeXoria/commit/2408ede)), property tests in every crate
  ([1585c71](https://github.com/Herbrant/TeXoria/commit/1585c71)), fuzz targets for the parsers
  ([288296a](https://github.com/Herbrant/TeXoria/commit/288296a)), the app tested headless
  ([cfb4b83](https://github.com/Herbrant/TeXoria/commit/cfb4b83)) against a fake engine and a fake
  agent ([4db14d8](https://github.com/Herbrant/TeXoria/commit/4db14d8),
  [800a168](https://github.com/Herbrant/TeXoria/commit/800a168)).
- CI measures coverage on every release platform and enforces a minimum per crate
  ([497e644](https://github.com/Herbrant/TeXoria/commit/497e644),
  [fff5d31](https://github.com/Herbrant/TeXoria/commit/fff5d31)).
- A new Testing page in the developer documentation
  ([2a4c68c](https://github.com/Herbrant/TeXoria/commit/2a4c68c)).

**Full changelog**: [v0.4.1...v0.4.2](https://github.com/Herbrant/TeXoria/compare/v0.4.1...v0.4.2)

## [0.4.1] - 2026-10-17

### Fixed

- TeXoria no longer crashes when the pointer rests on the text after a click: the hover read the
  editor while the editor was updating it. ([5ba2d1d](https://github.com/Herbrant/TeXoria/commit/5ba2d1d))

### Build and documentation

- The new tests pass on Windows. ([7e451ba](https://github.com/Herbrant/TeXoria/commit/7e451ba))

**Full changelog**: [v0.4.0...v0.4.1](https://github.com/Herbrant/TeXoria/compare/v0.4.0...v0.4.1)

## [0.4.0] - 2026-10-17

### Added

- **Find and replace in the whole project.** The new Search view (⇧⌘F / Ctrl+Shift+F, or the
  magnifying glass in the title bar) looks through every text file of the project, unsaved edits
  included, matching the case, whole words or a regular expression. Replace All changes open
  files through the editor, so it can be undone, and writes the others. ([47cebfa](https://github.com/Herbrant/TeXoria/commit/47cebfa))
- **Go to definition and hover.** ⌘-click or F12 on a `\ref`, `\cite`, `\input`,
  `\includegraphics` or one of your commands and environments goes to its label, `.bib` entry,
  file or definition. Hovering shows a citation's authors and title, what a label is attached to
  and how often it is referenced, file sizes and what a command does. ([a75284e](https://github.com/Herbrant/TeXoria/commit/a75284e))
- **Problems found as you type.** References to labels that do not exist, labels defined twice,
  citations missing from the bibliography, `\input`/`\includegraphics` of missing files, and
  `.bib` entries that are not closed, share a key, lack a required field or have a malformed
  DOI are underlined without building, explained on hover and listed with the problems.
  ([8e84e19](https://github.com/Herbrant/TeXoria/commit/8e84e19))
- **Grammar checking with LanguageTool.** With LanguageTool installed (`brew install
  languagetool`), grammar and style mistakes are underlined in the file's language, with their
  corrections in the context menu. LaTeX commands are read as what they stand for, so they are
  not mistakes. TeXoria starts the local server when needed and stops it on quit; your text
  stays on your computer unless you set another server. Off by default. ([87261f9](https://github.com/Herbrant/TeXoria/commit/87261f9))
- **Citations from a DOI, arXiv or a search.** Edit → Add Citation… (⇧⌘C) fetches the entry of a
  DOI or an arXiv preprint, or lists Crossref's matches for some words of a title and authors,
  writes it to the project's `.bib` and cites it at the cursor. ([6448c12](https://github.com/Herbrant/TeXoria/commit/6448c12))
- **Word count.** The status bar shows the project's words (or the selection's); click it for
  the words in the text, headings and captions, the formulas, and each file's. ([db9917b](https://github.com/Herbrant/TeXoria/commit/db9917b))
- **Vim mode.** Edit → Vim Mode (or Settings → Editor) turns on Vim's keys: normal, insert and
  visual modes, counts, operators with motions and text objects, including `ie`/`ae` for the
  environment around the cursor and `i$`/`a$` for inline math, registers shared with the
  clipboard, search, `.` and `:w`/`:q`. ([9e14f1b](https://github.com/Herbrant/TeXoria/commit/9e14f1b))
- **A complete PDF viewer.** The preview shows the current page and goes to any page (⌘G), fits
  the whole page (⌘9), moves page by page with the arrows, Page Up/Down, Home and End
  ([028b310](https://github.com/Herbrant/TeXoria/commit/028b310)); follows `hyperref` links, opening web and email links in their apps and never
  files or programs ([b8d527b](https://github.com/Herbrant/TeXoria/commit/b8d527b)); finds text with ⌘F ([cb539f6](https://github.com/Herbrant/TeXoria/commit/cb539f6)); and copies the text you
  drag across with ⌘C ([3f800d4](https://github.com/Herbrant/TeXoria/commit/3f800d4)).
- **Presentations.** View → Present PDF (⇧⌘↵) shows the PDF full screen, one page at a time, with
  the usual presentation keys; it follows rebuilds. ([7da9608](https://github.com/Herbrant/TeXoria/commit/7da9608))
- **Export the PDF.** File → Export PDF… (⇧⌘S) saves a copy where you like, starting from where
  the last one went. ([753d5f7](https://github.com/Herbrant/TeXoria/commit/753d5f7))
- **Formatting.** Edit → Format Document (⇧⌥F) indents the file with tex-fmt in one undoable
  edit, following a `tex-fmt.toml` in the project; Format on save does it on every ⌘S.
  ([855ea3c](https://github.com/Herbrant/TeXoria/commit/855ea3c))
- **Shell escape for trusted projects.** When a build fails because a package (`minted`, `svg`,
  `pythontex`…) needs to run programs, a banner lets you allow shell escape for that project; it
  is listed and revocable with the other trusted projects. ([aeeeb8d](https://github.com/Herbrant/TeXoria/commit/aeeeb8d))
- Comment or uncomment the selected lines with ⌘/ (Ctrl+/). ([dbc7813](https://github.com/Herbrant/TeXoria/commit/dbc7813))
- Path completion in `\input`, `\include`, `\includegraphics`, `\bibliography` and
  `\addbibresource`. ([e3213ee](https://github.com/Herbrant/TeXoria/commit/e3213ee))
- A project opens again with the tabs you left open. ([a414c3e](https://github.com/Herbrant/TeXoria/commit/a414c3e))
- Right-click a package in `\usepackage` to open its documentation, from your TeX distribution
  or texdoc.org. ([ababb1b](https://github.com/Herbrant/TeXoria/commit/ababb1b))
- Agents can see the PDF: new tools give them a picture of a page (to check the layout), the
  PDF's text, the bibliography entries matching a search and the word count; the problems they
  get include those found as you type. ([6569d9b](https://github.com/Herbrant/TeXoria/commit/6569d9b))
- ⌘= / ⌘- / ⌘0 change the editor's font size (a new *Font size* setting); after clicking the
  preview they zoom the PDF as before. Buttons, the sidebar and the status bar are larger.
  ([15aa17d](https://github.com/Herbrant/TeXoria/commit/15aa17d))

### Changed

- The editor's Replace moved from ⇧⌘F to ⌥⌘F (Ctrl+H stays), since ⇧⌘F now searches the whole
  project. ([47cebfa](https://github.com/Herbrant/TeXoria/commit/47cebfa))
- The Edit menu lists the editing commands (undo, find, comment, format, citations, Vim mode).
  ([dbc7813](https://github.com/Herbrant/TeXoria/commit/dbc7813), [47cebfa](https://github.com/Herbrant/TeXoria/commit/47cebfa))

### Build and documentation

- New crates: `tex-cite` (DOI, arXiv and Crossref lookups) and `tex-vim` (the Vim emulation);
  `pdf-view` answers queries about the text, links and pages of a PDF, and a small hyperref PDF
  is a new test fixture. ([6448c12](https://github.com/Herbrant/TeXoria/commit/6448c12), [9e14f1b](https://github.com/Herbrant/TeXoria/commit/9e14f1b), [6fc3148](https://github.com/Herbrant/TeXoria/commit/6fc3148))
- The project index and the bibliography reader record where every label, reference, citation,
  include and entry is. ([ff54788](https://github.com/Herbrant/TeXoria/commit/ff54788))
- New README screenshots. ([5d39d26](https://github.com/Herbrant/TeXoria/commit/5d39d26))

**Full changelog**: [v0.3.0...v0.4.0](https://github.com/Herbrant/TeXoria/compare/v0.3.0...v0.4.0)

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
