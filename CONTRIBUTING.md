# Contributing to TeXoria

Thanks for helping! Bug reports, ideas and pull requests are all welcome.

## Reporting bugs and requesting features

Search the [existing issues](https://github.com/Herbrant/TeXoria/issues) first, then
[open a new one](https://github.com/Herbrant/TeXoria/issues/new). For a bug, include:

- your TeXoria version, operating system and TeX distribution (or Tectonic);
- the steps to reproduce it, ideally with a small `.tex` file;
- what you expected and what happened instead, with the
  [log](docs/troubleshooting.md#getting-logs) if relevant.

Security issues are different: please follow [SECURITY.md](SECURITY.md) and do not open a
public issue.

## Development setup

See [Building from source](docs/development/building.md). In short:

```sh
./scripts/fetch-pdfium.sh
cargo run -p texoria -- tests/fixtures/sample/main.tex
```

[Architecture](docs/development/architecture.md) explains how the crates fit together.

## Pull requests

1. For anything larger than a small fix, open an issue first to agree on the approach.
2. Keep each pull request focused on one change, and add tests for new logic. Only the
   `texoria` crate depends on the UI: logic belongs in the other crates, where it can be
   unit-tested.
3. Make sure the CI checks pass locally:
   ```sh
   cargo fmt --all --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
4. Update the [documentation](docs/README.md) when you change what users see.

### Conventions

- Code, comments, documentation and commit messages are in English.
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org):
  `type(scope): subject`, e.g. `fix(synctex): handle empty boxes` or `feat(git): show tags`.
- Lines are at most 100 characters (`rustfmt.toml`).
- Opening a project must never run code from it: changes touching `latexmkrc`, Git
  configuration or credentials need extra care (see the
  [security model](docs/development/architecture.md#security-model)).

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
