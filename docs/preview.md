# Build and preview

## Live preview

With live preview on (the *● Live* switch in the status bar, or ⌘⇧P), TeXoria saves and
builds the document a moment after you stop typing (700 ms by default, see
[Settings](settings.md)). The editor never waits for the engine: if you keep typing during a run,
the changes are built in a single follow-up run once it ends. The new PDF replaces the old one
only once its pages are rendered, so the preview never flashes.

With live preview off, build by hand with ⌘R. ⌘. stops a run (outside the editor) and a run
is stopped anyway after a timeout (5 minutes by default).

A project opened before shows its last PDF right away. The first time, TeXoria builds the
document from scratch and only shows the PDF it made: build files that came with the project
(in a clone or an archive) are never shown, since they could be crafted to attack the PDF
renderer.

## SyncTeX

- **Source → PDF**: ⌘J scrolls the preview to the cursor's position.
- **PDF → source**: ⌘-click (Ctrl-click) or double-click in the preview to jump to the matching
  line of the source.

## Zoom

Once you have clicked the preview, ⌘= and ⌘- zoom it in and out and ⌘0 fits the page to the
pane's width (elsewhere they resize the editor's text). The buttons at the bottom right of the
preview and the *View* menu do the same; clicking the zoom level switches between fitting the
width and fitting the whole page (⌘9). In the dark theme, pages are darkened while keeping their
colors.

## Pages

The toolbar shows the page in the middle of the pane and how many there are: click it, or press
⌘G, to type the page to go to. Once you have clicked the preview, ←/→ and Page Up/Page Down go to
the previous or next page, Home and End to the first or last one.

## Finding text

Once you have clicked the preview, ⌘F (Ctrl+F) opens a find bar at the top of the pane (also
*View* → *Find in PDF…*). Every occurrence is highlighted, the current one more strongly; ↵ and
⇧↵ (or the arrows) go to the next and previous one, the button next to them matches the case,
and Esc closes the bar. The search runs again when the PDF is rebuilt.

## Copying text

Drag across a page to select its text, then press ⌘C (Ctrl+C) to copy it; a click elsewhere or
Esc clears the selection. A selection stays within one page.

## Links

Links in the PDF work as in a PDF reader: those made by `hyperref` (`\ref`, `\cite`, the table
of contents) scroll to their target, and web and email links (`\url`, `\href`) open in your
browser or mail app. Links to files or programs are never followed. ⌘-click and double click
still jump to the source.

## Engines

TeXoria looks for the following engines and uses the first one found, unless you pick another:

1. **latexmk** from TeX Live, MacTeX, MiKTeX or TinyTeX, with **pdfLaTeX**, **XeLaTeX** or
   **LuaLaTeX**. Besides your `PATH`, TeXoria looks in the usual install locations
   (`/Library/TeX/texbin`, `/usr/local/texlive/<year>`, MiKTeX's and TinyTeX's folders, …),
   which a GUI app does not always see on its `PATH`.
2. **Tectonic**, bundled with TeXoria. It needs no installation and downloads the LaTeX packages
   a document uses the first time it needs them (an internet connection is required then).

Switch engine by clicking its name in the status bar, or in [Settings](settings.md) →
*Build*. The choice is remembered.

## latexmkrc

A project's `latexmkrc` (or `.latexmkrc`) is a Perl script: opening a project downloaded from the
internet must not run it. TeXoria therefore runs latexmk with `-norc`, keeping only your own and
the system's rc files, and shows a banner when the project has its own. Choose *Allow latexmkrc*
in the banner to use it for that project. You can revoke the permission in
[Settings](settings.md#trusted-projects).

## Shell escape

Some packages run programs while the document builds: `minted` (Pygments), `svg` (Inkscape),
`pythontex`, TikZ externalization, `gnuplottex`… They need *shell escape*, which lets the
document run any command, so it is off by default. When a build fails because a package needed
it, TeXoria shows a banner: choose *Allow Shell Escape* to build that project with
`-shell-escape` (latexmk) or `-Z shell-escape` (Tectonic) from then on. Allow it only for
projects you trust (a coding agent that edits the document could then run commands through the
build too). You can revoke the permission in [Settings](settings.md#trusted-projects).

## Exporting the PDF

*File* → *Export PDF…* (⇧⌘S), or the download button of the preview's toolbar, saves a copy of
the PDF wherever you like: the build folder is TeXoria's. The next export of the project starts
from the same place and name.

## Build files

Everything the engine produces goes to `.texoria/build` next to the root file, which TeXoria
excludes from Git. Delete that folder to force a clean build.
