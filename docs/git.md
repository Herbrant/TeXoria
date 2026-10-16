# Git

TeXoria has source control built in (⌘⇧G, or click the branch in the status bar). It runs the
`git` installed on your system, so your credential helpers, SSH keys, commit signing and hooks
keep working as they do in a terminal.

## Trusting a project

A repository's own configuration can run programs (for example `core.fsmonitor` or filters),
even on a plain `git status`. So TeXoria does not run Git in a project you open until you choose
*Trust Project* in the Source Control panel. Projects you clone from TeXoria are trusted
automatically. Trust applies to that exact folder, not to the folders below it, and can be
revoked in [Settings](settings.md#trusted-projects).

## Changes and commits

The *Changes* tab lists merge conflicts, staged files and changes. From there you can:

- open a file's diff, word by word, unified or side by side (see *Side-by-side diffs* in
  [Settings](settings.md#git));
- stage and unstage files, or discard their changes;
- write a message and *Commit* (*Commit All* when nothing is staged), or *Amend* the last
  commit.

The first time, TeXoria asks for the name and email Git records with every commit, for that
repository or for all of them.

## Remotes

*Fetch*, *Pull* and *Push* are in the panel and in the *Git* menu. A branch without an upstream
shows *Publish*, and a repository without a remote *Add Remote…*. Pull merges by default; turn
on *Pull with rebase* in [Settings](settings.md#git) to rebase instead. A network operation stops
after a timeout (2 minutes by default) and can be cancelled.

When a pull stops on conflicts, they are listed first and shown in the diff view; resolve them,
then continue or abort the merge or rebase from the panel.

## History

The *History* tab lists the commits of the current branch. Open one to see what it changed
(against its first parent, for merges), and use *Restore* on a file to put that version back in
the project.

## Branches

The branch menu at the top of the panel switches to a local or remote branch, and creates a new
branch from the current commit (*New Branch…*).

## Clone and init

- *Clone Repository…* (welcome screen or *Git* menu) clones from GitHub, GitLab or any Git URL
  and opens the project.
- *Initialize Repository* in the panel turns the current project into a repository.

Working with Overleaf? See [Overleaf](overleaf.md).

## Limitations

The editor shows no change markers in the gutter, and commits with several parents are shown
against their first parent only.
