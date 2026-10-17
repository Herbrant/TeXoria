# Settings

Open the Settings window with ⌘, (Ctrl+,). Every change applies right away.

## Editor

| Setting | Default | |
|---|---|---|
| Theme | System | *Paper* (light), *Ink* (dark), or follow the system appearance. ⌘⇧L switches between them. |
| Font size | 14.5 | Size of the editor's text, in pixels. ⌘= and ⌘- change it, ⌘0 restores it. |
| Soft wrap | On | Wrap long lines. Very long files always open unwrapped; ⌥Z toggles the current tab. |
| Check spelling | On | See [Spell checking](writing.md#spell-checking). |
| Dictionary | | The words you added to your dictionary; remove them from here. |

## Build

| Setting | Default | |
|---|---|---|
| Engine | First found | latexmk with pdfLaTeX, XeLaTeX or LuaLaTeX, or Tectonic (see [Engines](preview.md#engines)). |
| Timeout | 300 s | Stop a run after this many seconds (0: never). |
| Live preview | On | Save and build automatically after edits. |
| Delay | 700 ms | Pause after the last keystroke before a live run. |

## Git

| Setting | Default | |
|---|---|---|
| Pull with rebase | Off | Rebase local commits on top of the pulled ones instead of merging. |
| Side-by-side diffs | Off | Show changes in two columns instead of one. |
| Network timeout | 120 s | Stop a fetch, pull or push after this many seconds (0: never). |
| Overleaf Server Pro | | Your own Overleaf instances, which must use HTTPS. |

## Agents

| Setting | Default | |
|---|---|---|
| Agent | First found | The coding agent of the Agent panel and of the quick edits (see [Coding agents](agents.md)). |
| Other agents | | Agents TeXoria does not know: a name, the command and the arguments that start it in ACP mode. |

## Trusted projects

A project's Git configuration and its `latexmkrc` can run programs, and coding agents read and
change its files, so TeXoria asks before letting them (see [Git](git.md#trusting-a-project),
[latexmkrc](preview.md#latexmkrc) and [Coding agents](agents.md#the-agent-panel)). This page
lists the folders you allowed, separately for Git, for `latexmkrc` and for agents. Remove a
folder and TeXoria will ask again.

## Updates

| Setting | Default | |
|---|---|---|
| Check for updates | On | Look for a new release at startup, at most once a day. See [Updating](installation.md#updating). |
| Check now | | Look for a new release right away (on macOS, also **Check for Updates…** in the TeXoria menu); the answer appears in the main window. |

## Where files are stored

Settings are a TOML file, `settings.toml`, that you can also edit by hand while TeXoria is
closed. A value TeXoria cannot read resets only that setting: TeXoria says so when it starts and
keeps the file as it was in `settings.toml.bak`, next to it. Downloaded spelling dictionaries go to the cache folder.

| | Settings | Dictionaries |
|---|---|---|
| macOS | `~/Library/Application Support/app.TeXoria.TeXoria/` | `~/Library/Caches/app.TeXoria.TeXoria/dictionaries/` |
| Linux | `~/.config/texoria/` | `~/.cache/texoria/dictionaries/` |
| Windows | `%APPDATA%\TeXoria\TeXoria\config\` | `%LOCALAPPDATA%\TeXoria\TeXoria\cache\dictionaries\` |

Overleaf tokens are not in the settings: they are in the system keychain (Keychain on macOS,
Credential Manager on Windows, the Secret Service on Linux).
