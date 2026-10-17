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

⌘= and ⌘- zoom the preview in and out, ⌘0 fits the page to the pane's width. The buttons at the
top of the preview do the same. In the dark theme, pages are darkened while keeping their colors.

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

## Build files

Everything the engine produces goes to `.texoria/build` next to the root file, which TeXoria
excludes from Git. Delete that folder to force a clean build.
