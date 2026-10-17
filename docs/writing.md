# Writing

## Completion

Suggestions appear as you type and depend on where the cursor is:

| Where | What is suggested |
|---|---|
| After `\` | Commands |
| `\begin{…}` | Environments; the matching `\end{…}` is inserted for you |
| `\ref{…}`, `\eqref{…}`, … | The labels defined anywhere in the project |
| `\cite{…}` | The entries of the project's `.bib` files, matched by key, title or author |
| `\usepackage{…}` | Package names |
| `\input{…}`, `\include{…}` | The project's folders and `.tex` files (without the extension) |
| `\includegraphics{…}` | Folders, pictures and PDFs |
| `\bibliography{…}`, `\addbibresource{…}` | Folders and `.bib` files |

Paths are relative to the root file's folder, like TeX's; pick a folder to see what is inside.
Ranking is fuzzy, so `\sec` finds `\section` and `\subsection`. Completions follow unsaved
changes in every open tab.

## Definitions and hover

⌘-click (Ctrl-click) a name, or press F12 with the cursor on it, to go where it is defined:

| On | Goes to |
|---|---|
| `\ref{…}`, `\eqref{…}`, `\cref{…}`… | its `\label` |
| `\cite{…}` | the entry in the `.bib` file (or the `\bibitem`) |
| `\input{…}`, `\include{…}`, `\bibliography{…}` | that file |
| `\includegraphics{…}` | the picture, opened in its app |
| a command or environment of your own | its `\newcommand` / `\newenvironment` |

Hovering shows what a name stands for: a citation's authors, title and year, what a label is
attached to, how many times a label is referenced, a file's size, what a command does, and a
link to a package's documentation. Right-click a package in `\usepackage{…}` and choose
*Documentation of …* to open its manual: the copy installed with your TeX distribution
(through `texdoc`), or else the one on [texdoc.org](https://texdoc.org).

## Comments

⌘/ (Ctrl+/) comments the lines of the selection out with `%`, at the indentation of the
least indented one, or uncomments them if they all are comments already. Also in *Edit* →
*Toggle Comment*.

## Outline

The outline (⌘1) lists the parts, chapters, sections and subsections of the whole project,
following `\input` and `\include`, and highlights the section you are editing. Click an entry to
jump to it.

## Files

The Files view (⌘⇧E) shares the sidebar with the outline and shows the project folder as a tree,
colored by Git status. From it you can open, create, rename and delete files and folders. Tabs
follow a renamed file and close when their file is deleted. Documents and pictures that are not
text, such as images and PDFs, open in their usual app. Any other file could be a program (a
project may come from anywhere), so TeXoria shows it in its folder instead of starting it.

## Search

⌘F (Ctrl+F) finds in the open file, ⌥⌘F (Ctrl+H) replaces in it.

⇧⌘F (Ctrl+Shift+F), or the magnifying glass in the title bar, searches every text file of the
project folder, starting with the selected text. The buttons in the search field match the
case, whole words only, or a regular expression. Results are grouped by file: click one to
jump to it. Open files are searched as they are on screen, unsaved edits included.

The arrow on the left shows the replace field. *Replace All* changes every match: in open files
through the editor, so ⌘Z undoes it, and directly on disk for the others. With a regular
expression, `$1`, `$2`… in the replacement stand for its groups.

## Diagnostics

After each run, the errors and warnings of the TeX log are underlined in the source and listed
in the *Problems* panel (⌘⇧M). Click a problem to jump to its line.

## Spell checking

Misspelled words are underlined with a wavy line. Only the prose is checked: commands, math,
labels, citations and the preamble (except `\title`) are skipped. Right-click a word, or press
⌘. (Ctrl+.) on it, to pick a correction or add the word to your dictionary.

Each file's language is detected automatically from, in order:

1. a magic comment, in the file or in the root file:
   ```latex
   % !TeX spellcheck = en_GB
   ```
2. `babel` or `polyglossia` options in the root file;
3. the text itself;
4. the system language.

The language is shown in the status bar: click it to choose another one, go back to automatic
detection, or turn checking off for that file. About 90 languages are available (the
[Hunspell dictionaries of wooorm/dictionaries](https://github.com/wooorm/dictionaries)).
Dictionaries already installed on your system are used as they are; the others are downloaded
the first time they are needed and verified against a pinned checksum.

Turn spell checking off entirely, or remove words from your dictionary, in
[Settings](settings.md) → *Editor*.
