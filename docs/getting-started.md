# Getting started

## Open a project

Start TeXoria and choose *Open a document…* (⌘O on macOS, Ctrl+O elsewhere). You can pick a
`.tex` file or a whole folder. On Windows and Linux the system dialog cannot show files and
folders together, so folders have their own *Open a folder…* (File › Open Folder…,
Ctrl+Shift+O). The welcome screen can also *Clone a repository…* or *Open from Overleaf…* (see
[Git](git.md) and [Overleaf](overleaf.md)).

For a folder, TeXoria looks for its documents, whatever their name, in the folder and two
levels of subfolders: `.tex` files with a `\documentclass` that no other document includes
(`\input`, `\include`, `\subfile`) and whose `% !TeX root` comment does not name another
file. It opens the one you worked on last, or else one with a `\begin{document}`, preferring
the least deep and then the one that includes the most files.

A project opens again with the tabs you left open, and on the one you were working in (unless
you opened a specific file of it).

From a terminal, pass the file or folder as the only argument:

```sh
texoria path/to/thesis/                                        # Linux
/Applications/TeXoria.app/Contents/MacOS/texoria thesis.tex    # macOS
```

`texoria --help` lists the options, `texoria --version` prints the version. A file whose name
starts with `-` goes after `--` (`texoria -- -draft.tex`).

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
   that includes the file through `\input` or `\include` (or, for a `.bib` file,
   `\bibliography` or `\addbibresource`);
4. for a file that is not a `.tex` one (a `.bib` file, a package, notes), the document of its
   folder;
5. otherwise, the file itself.

## Where the PDF goes

Build files, the PDF included, are written to `.texoria/build` next to the root file. TeXoria
also puts a `.gitignore` in `.texoria`, so none of it ever shows up in Git and your project
folder stays clean.

## Next steps

- Learn the [keyboard shortcuts](keyboard-shortcuts.md).
- Pick a theme and an engine in the [settings](settings.md) (⌘,).
