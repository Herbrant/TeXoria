# Writing

LaTeX sources and `.bib` files are syntax highlighted: in a `.bib` file, entry types, keys,
field names and values each get their own color.

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

## Formatting

*Edit* → *Format Document* (⇧⌥F, Shift+Alt+F) indents the file with
[tex-fmt](https://github.com/WGUNDERWOOD/tex-fmt): environments, lists and groups, two spaces
per level, leaving verbatim-like environments alone. Lines are not rewrapped. It is one edit,
undone with ⌘Z. A `tex-fmt.toml` in the project folder changes the rules (tab size, wrapping,
extra list or verbatim environments). To format on every ⌘S, turn on
[Settings](settings.md) → *Editor* → *Format on save*.

## Adding citations

*Edit* → *Add Citation…* (⇧⌘C) takes a DOI (or its doi.org link), an arXiv identifier (or its
link), or words of a paper's title and authors:

- for a DOI, the entry comes from doi.org, as its publisher registered it;
- for an arXiv identifier, from arXiv (the published version's, if arXiv knows its DOI);
- for words, [Crossref](https://www.crossref.org) lists the papers that match: pick one.

The entry is written, one field per line, at the end of the project's `.bib` file (a new
`references.bib` in the project folder if it has none: TeXoria then tells you to reference it)
under a new key like `knuth1984texbook`, and `\cite{…}` is inserted at the cursor (just the key,
if the cursor is in a `\cite` already). Only these lookups, when you ask for them, go online.

## Vim mode

Turn on *Edit* → *Vim Mode*, or [Settings](settings.md) → *Editor* → *Vim mode*, to edit with
Vim's keys. The status bar shows the mode (`-- NORMAL --`, `-- INSERT --`, `-- VISUAL --`,
`-- VISUAL LINE --`) and what you typed of a command; normal mode draws a block cursor.

| | |
|---|---|
| Modes | `i a I A o O` insert, `v V` visual, Esc (or Ctrl-[) back to normal |
| Motions | `h j k l w b e W B E 0 ^ $ gg G f F t T ; , % { }`, Ctrl-D / Ctrl-U, with counts (`3w`, `5G`) |
| Operators | `d c y > <` with a motion or a text object, doubled for lines (`dd`, `3yy`, `>>`) |
| Text objects | `iw aw iW aW i( a( i{ a{ i[ a[ i< a< i" a"`, and for LaTeX `ie`/`ae` (the environment around the cursor) and `i$`/`a$` (inline math) |
| Edits | `x X s S r D C Y J ~ p P`, `u` undo, Ctrl-R redo, `.` repeats the last change |
| Visual mode | `d x c y > <` and `J ~ u U r p P` act on the selection (`p` keeps the replaced text) |
| Search | `/` and `?` (ignoring case unless the text has a capital), `n N`, `*` for the word under the cursor |
| Commands | `:w`, `:q` (closes the tab), `:wq`, `:x`, `:42` (go to line 42) |

Yanks also go to the system clipboard, and `p` pastes what you copied elsewhere since. ⌘
shortcuts keep working in every mode; of the Ctrl ones, Vim takes only Ctrl-R, Ctrl-D, Ctrl-U,
Ctrl-[ and Ctrl-C (so on Windows and Linux, build with the *Build* button or menu while in
normal mode). Characters your keyboard types with Option (`~ { } [ ]` on an Italian
Mac) work as Vim commands.

## Outline

The outline (⌘1) lists the parts, chapters, sections and subsections of the whole project,
following `\input` and `\include`, and highlights the section you are editing. Click an entry to
jump to it.

Each heading groups what follows it (subsections under their section, figures and tables under
their heading), and groups start collapsed, so only the top level is listed at first. Click a
heading's arrow to open or close its group, or use the button at the top of the outline to
expand or collapse them all. When the section you are editing is in a closed group, the
heading around it is highlighted instead.

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

## Word count

The status bar shows how many words the whole project has (all the files it includes, as they
are on screen), or how many are selected out of them. Click it for the details, counted like
texcount does: words in the text, in headings and in captions, inline and displayed formulas,
and the words of each file. Commands, math, comments and the preamble (except `\title`) are
not words.

## Diagnostics

After each run, the errors and warnings of the TeX log are underlined in the source and listed
in the *Problems* panel (⌘⇧M). Click a problem to jump to its line.

Some problems are found as you type, without building, and underlined with a wavy line (hover
it to read why):

- a `\ref` to a label that does not exist, and a label defined twice;
- a `\cite` of a key that is in no `.bib` file (nor `\bibitem`), once the bibliography can be
  read;
- an `\input`, `\include`, `\bibliography` or `\includegraphics` whose file does not exist
  (pictures are looked for with the usual extensions and in `\graphicspath`);
- in `.bib` files: entries that are not closed, keys used twice, fields an entry type needs
  (an `@article` without `journal`, a `@book` without `publisher`…) and DOIs that are not
  DOIs.

They are listed with the problems of the last run, and the TeX warnings they repeat are left
out. Turn them off in [Settings](settings.md) → *Editor* → *Check references*.

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
3. the text itself (for a file with too little text, like a short chapter, the root file's);
4. the system language.

The language is shown in the status bar: click it to choose another one, go back to automatic
detection, or turn checking off for that file. About 90 languages are available (the
[Hunspell dictionaries of wooorm/dictionaries](https://github.com/wooorm/dictionaries)).
Dictionaries already installed on your system are used as they are; the others are downloaded
the first time they are needed and verified against a pinned checksum.

Turn spell checking off entirely, or remove words from your dictionary, in
[Settings](settings.md) → *Editor*.

## Grammar

With [LanguageTool](https://languagetool.org) installed, TeXoria also underlines grammar and
style mistakes, in a different color from spelling ones, in the language of the file's
spelling. Hover a mistake to read why; right-click it, or press ⌘. (Ctrl+.), to apply a
correction or ignore that rule from then on. Only the prose is checked: commands are read as
what they stand for (a formula as a noun, a citation as a number), so they do not count as
mistakes.

Turn it on in [Settings](settings.md) → *Editor* → *Check grammar*. LanguageTool runs on your
computer, so your text stays there:

- macOS: `brew install languagetool`;
- elsewhere: download the server from [languagetool.org](https://languagetool.org/download/)
  (it needs Java) and start it with
  `java -cp languagetool-server.jar org.languagetool.server.HTTPServer --port 8081`, or install
  your distribution's package.

If the server is not running, TeXoria starts `languagetool-server` itself (and stops it when it
quits); the first check takes a few seconds while it loads. The status bar's *Grammar* item
shows whether LanguageTool is ready; click it to try again after an error. Another server, like
your organization's or LanguageTool Premium's, can be set in *LanguageTool server*: your text is
then sent to it as you type.
