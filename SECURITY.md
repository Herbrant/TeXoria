# Security policy

## Supported versions

Security fixes are released for the latest version of TeXoria only. Please update before
reporting.

## Reporting a vulnerability

Please **do not open a public issue**. Report it privately through
[GitHub's security advisories](https://github.com/Herbrant/TeXoria/security/advisories/new)
instead, with:

- the affected version and operating system;
- a description of the issue and its impact;
- the steps or a sample project to reproduce it.

You should get an answer within a week. Once the issue is confirmed, a fix is prepared in the
private advisory and released as soon as possible, then the advisory is published, crediting you
unless you prefer otherwise.

## Scope

TeXoria's guarantee is that **opening a project never runs code from it** until you allow it.
Examples of what we want to hear about:

- a project's `latexmkrc`, Git configuration or hooks running without being trusted;
- Overleaf tokens or Git credentials leaking (on the command line, in logs, over HTTP, to a
  different server);
- downloads (dictionaries, the install script, release packages) that are not verified, or a way
  to make TeXoria load a library or engine from an unexpected location;
- files written outside the project or the build folder.

Out of scope: the behavior of TeX engines themselves (for example `\write18` when you enable
shell escape in your own configuration), and code that you explicitly allowed to run.
