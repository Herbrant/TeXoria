# Getting started

## Open a project

Start TeXoria and choose *Open a document…* (⌘O on macOS, Ctrl+O elsewhere). You can pick a
`.tex` file or a whole folder: for a folder, TeXoria opens `main.tex`, or else the first file
with a `\documentclass`. The welcome screen can also *Clone a repository…* or *Open from
Overleaf…* (see [Git](git.md) and [Overleaf](overleaf.md)).

From a terminal, pass the file or folder as the only argument:

```sh
texoria path/to/thesis/                                        # Linux
/Applications/TeXoria.app/Contents/MacOS/texoria thesis.tex    # macOS
```

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
- **Editor**: one tab per open file.
- **PDF preview**: updated as you type (see [Build and preview](preview.md)).
- **Status bar**: the Git branch, the [spell checking](writing.md#spell-checking) language,
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
