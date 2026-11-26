# Getting started

## Open a project

Start TeXoria, choose *Open a document…* (⌘O on macOS, Ctrl+O elsewhere) and pick the
project's main `.tex` file: the one you would compile, even if its `\documentclass` is in a
file it `\input`s. A part of the project opens it too: its `% !TeX root` comment, or the
document that includes it (in its folder or up to two folders above), leads to the main file.
TeXoria opens projects by a `.tex` file only: not by a folder, nor by a `.bib` on its own
(once a project is open, *Open a document…* opens any text file of its folder, such as its
`.bib`, in a tab). The welcome screen can also *Clone a repository…* or *Open from Overleaf…*
(see [Git](git.md) and [Overleaf](overleaf.md)).

A project opens again with the tabs you left open, and on the one you were working in (unless
you opened a specific file of it).

The installed app also opens `.tex` files from the Finder, Explorer or your file manager: it
is listed under *Open With*, and you can make it their default app there. On macOS a file
opened while TeXoria runs goes to the window of its project if one shows it, else to a window
still on the welcome screen, else to a new window. Windows installs make TeXoria the app of
`.tex` files only when they had none.

From a terminal, pass the file as the only argument:

```sh
texoria path/to/thesis/thesis.tex                              # Linux
/Applications/TeXoria.app/Contents/MacOS/texoria thesis.tex    # macOS
```

`texoria --help` lists the options, `texoria --version` prints the version. A file whose name
starts with `-` goes after `--` (`texoria -- -draft.tex`).

## More than one project

Each window shows one project. *File → New Window* (⌘⇧N on macOS, Ctrl+Shift+N elsewhere;
on macOS also in the Dock icon's menu) opens another one on the welcome screen, where you
open, clone or pick a recent project. In a window that already shows a project, *Open a
document…* replaces it. A project is only ever open in one window: opening it again brings
its window to the front. The settings are the same in every window, and a change made in one
(the theme, the text size, a trusted project…) applies to all of them. Closing a window
closes its PDF and Settings windows, not the other projects.

## The window

Shortcuts are written for macOS: elsewhere, use Ctrl instead of ⌘ (see
[Keyboard shortcuts](keyboard-shortcuts.md)).

```
┌──────────────┬──────────────────────────┬──────────────────────┐
│ Outline      │ main.tex  intro.tex      │                      │
│ Files        │                          │                      │
│ Source       │  LaTeX source            │  PDF preview         │
│ Control      │                          │                      │
├──────────────┴──────────────────────────┴──────────────────────┤
│ branch · language · ● Live · engine                 status bar │
└────────────────────────────────────────────────────────────────┘
```

- **Sidebar**: the document [outline](writing.md#outline), the project's
  [files](writing.md#files) or [source control](git.md). Toggle it with ⌘1 (outline),
  ⌘⇧E (files) or ⌘⇧G (source control).
- **Editor**: one tab per open file. When they do not fit, swipe or scroll over the tabs; the
  bar always shows the active tab, so a file you open is in view at its right end. Close a tab
  with its × button, a middle click or ⌘W; its edits are saved first. The last open tab stays.
  Files changed by another program are reloaded when you come back to TeXoria; if one had
  unsaved edits, TeXoria asks whether to keep them or load the file from disk, and never saves
  over the other changes before you choose.
- **PDF preview**: updated as you type (see [Build and preview](preview.md)).
- **Status bar**: the Git branch, the [word count](writing.md#word-count), the
  [spell checking](writing.md#spell-checking) language,
  the live preview switch and the engine (click it to switch to the next one).
- **Problems** (⌘⇧M): the errors and warnings of the last run.

## Multi-file projects

TeXoria always builds the project's **root file**, whichever file you are editing. It finds
it, in order, from:

1. a `% !TeX root = ../main.tex` comment in the first 20 lines of the file;
2. the file itself, if it has a `\documentclass`;
3. a `.tex` file with a `\documentclass` in the same directory or up to two levels above it,
   that includes the file through `\input` or `\include`;
4. otherwise, the file itself.

## Where the PDF goes

Build files, the PDF included, are written to `.texoria/build` next to the root file. TeXoria
also puts a `.gitignore` in `.texoria`, so none of it ever shows up in Git and your project
folder stays clean.

## Next steps

- Learn the [keyboard shortcuts](keyboard-shortcuts.md).
- Pick a theme and an engine in the [settings](settings.md) (⌘,).
