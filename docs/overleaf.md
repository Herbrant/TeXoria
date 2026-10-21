# Overleaf

TeXoria can work on an [Overleaf](https://www.overleaf.com) project offline and bring both sides
up to date with a single *Sync*. It goes through Overleaf's
[Git integration](https://docs.overleaf.com/integrations-and-add-ons/git-integration-and-github-synchronization/git-integration),
a premium feature: the project's owner needs a paid plan. Git must be installed.

## Open a project

1. Choose *Open from Overleaf…* (welcome screen or *Git* menu) and paste the project's address
   from your browser (`https://www.overleaf.com/project/…`).
2. Paste an authentication token, created in Overleaf's *Account Settings* under *Git
   Integration*.
3. Pick where to put the project.
4. If the project has more than one `.tex` file, pick its main file (the one Overleaf
   compiles).

The token is kept in the system keychain, and only once Overleaf has accepted it. Tokens expire
after a year: when Overleaf rejects one, TeXoria asks for a new one.

## Sync

Edit as usual, with or without a connection. *Sync* in the Source Control panel (or *Git* →
*Sync with Overleaf*):

1. commits your changes, with the message in the commit box or "Update from TeXoria";
2. pulls what your collaborators changed on Overleaf, rebasing your commits on top;
3. pushes the result.

You can also commit with your own messages and Sync afterwards. When files would be added to or
deleted from the Overleaf project, Sync lists them and asks first.

LaTeX build files (`*.aux`, `*.log`, …), system and editor files (`.DS_Store`, `*~`, …) and
package caches (`_minted-*`) never reach your collaborators: TeXoria keeps them out through the
project's `.git/info/exclude`, which, unlike a `.gitignore`, is not pushed.

## What does not sync

- Overleaf projects have a single branch and no tags.
- Comments, tracked changes and chat do not travel through Git.
- The compiler and main document chosen on Overleaf are not part of the project: pick the engine
  in TeXoria, and use a `% !TeX root` comment if needed (see
  [Getting started](getting-started.md#multi-file-projects)).

## Overleaf Server Pro

Add your instance in [Settings](settings.md#git) → *Git* → *Overleaf Server Pro*
(`https://latex.example.org`). It must use HTTPS: TeXoria never sends a token over plain HTTP.
