# Testing

Every crate has unit tests next to its code (`#[cfg(test)] mod tests`, or a `tests.rs` beside the
module) and, where it drives processes or the file system, integration tests in its `tests/`
folder. CI runs them on every platform a release is built for: macOS (arm64 and x64), Linux
(x64 and arm64) and Windows (x64).

```sh
cargo test --workspace
cargo test -p tex-log                  # one crate
cargo test -p synctex inverse          # tests whose name matches
```

## Shared helpers

`crates/test-support` is a dev-dependency of every crate (and never a regular one):

- `fixtures()`: the path of `tests/fixtures`. The sample project there must compile with zero
  diagnostics; tests copy it to a temporary folder before building or editing it.
- `at("te|xt")`: text with a `|` cursor marker, as `(text, offset)`.
- `http::serve` / `serve_paths`: a loopback HTTP server with canned answers (status, headers,
  delays, a fake `Content-Length`) that records the requests. Network clients (citations,
  LanguageTool, dictionaries, updates) are tested against it, never against the real services.
- `git::isolated_env` / `git::Isolated`: Git with the user's configuration kept out of a test.
- `write`, `write_files`, `copy_dir`, `executable`: files, fixture copies and fake programs.
- `vendor(name)` / `pdfium()`: the repository's `vendor/` folders.
- `within(timeout, done)` / `eventually(timeout, found)`: polling until something happens.
- `http::percent_decode`, `Request::form`, `http::closed_url`: for the HTTP clients' tests.
- `skip(need, why)`: see below.

Programs the code under test starts are faked by small binaries built with the crate's tests:
`tex-compile`'s `fake-engine` (logs, PDF and SyncTeX output, exit codes, a long-running child to
check that a cancelled build stops its whole process tree) and `tex-agent`'s `fake-acp-agent`
(an agent speaking the Agent Client Protocol from a script). Integration tests find them with
`env!("CARGO_BIN_EXE_<name>")`.

## Tests that need the environment

Tests that need a tool of the host call `test_support::skip("TEX", "no TeX engine found")` and
return when it is missing, so they pass on a machine without it. CI sets
`TEXORIA_REQUIRE_<TOOL>=1`, which turns a missing tool into a failure instead:

| Variable | Needed by |
|---|---|
| `TEXORIA_REQUIRE_TEX` | the end-to-end build of the sample project (`tex-compile/tests/real_engine.rs`) |
| `TEXORIA_REQUIRE_TECTONIC` | the same with the bundled Tectonic (`--ignored`: it downloads TeX files the first time) |
| `TEXORIA_REQUIRE_PDFIUM` | rendering and PDF queries (`vendor/pdfium`, from `scripts/fetch-pdfium.sh`) |
| `TEXORIA_REQUIRE_GIT` | the Git integration tests |

Run them all as CI does with:

```sh
TEXORIA_REQUIRE_TEX=1 TEXORIA_REQUIRE_PDFIUM=1 TEXORIA_REQUIRE_GIT=1 cargo test --workspace
TEXORIA_REQUIRE_TECTONIC=1 cargo test -p tex-compile --test real_engine -- --ignored
```

## The app

The `texoria` crate is tested headless with GPUI's test platform (`#[gpui::test]`), which needs
no display on any system. `crates/texoria/src/testing.rs` opens real workspace windows from a
`Startup` (settings, TeX engines, PDFium folders) the test builds itself: tests never read or
write the user's settings (`Settings::load` gives the defaults under `cfg(test)`, and
`Settings::load_from` saves where it loaded from) and never depend on the engines installed. See
`crates/texoria/src/workspace/tests.rs` for examples.

In a test module that imports a file using `use gpui_kit::*`, add
`use core::prelude::v1::test;` (gpui exports a `test` attribute that would shadow the standard
one) and `use gpui_kit::gpui;` (for the paths `#[gpui::test]` generates).

## Property tests

[proptest](https://proptest-rs.github.io/proptest/) checks invariants on generated input:
ranges a parser reports slice its input, offsets stay on character boundaries, Vim key
sequences never leave the text, conversions round-trip. A failing case is saved in the crate's
`proptest-regressions/` folder: commit it, so that it is tried first from then on.
`PROPTEST_CASES=10000 cargo test -p tex-vim` runs more cases than the default 256.

## Coverage

CI measures line coverage with [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) and
fails when a crate falls below its minimum in `scripts/coverage-thresholds.tsv`. The job summary
shows the table for each platform, and the lcov reports are kept as artifacts. Raise a crate's
minimum when its tests improve; never lower one to make CI pass. The Tectonic test runs in the
same instrumented build, as a second `cargo llvm-cov --workspace --no-report` run (with the test
name as filter) merged into one report. CI runs no doctests (there are none, and cargo-llvm-cov only runs them on nightly):
write unit tests instead.

```sh
rustup component add llvm-tools-preview && cargo install cargo-llvm-cov
cargo llvm-cov --workspace --html                     # target/llvm-cov/html/index.html
cargo llvm-cov --workspace --json --summary-only --output-path cov.json
./scripts/coverage-check.sh cov.json
```

## Fuzzing

`fuzz/` holds [cargo-fuzz](https://rust-fuzz.github.io/book/cargo-fuzz.html) targets for the
parsers and the other code that reads untrusted text (engine logs, SyncTeX, `.bib` files, LaTeX
sources, citations, URLs, Vim keys). It needs a nightly toolchain:

```sh
rustup toolchain install nightly && cargo install cargo-fuzz
cd fuzz
cargo +nightly fuzz list
cargo +nightly fuzz run tex_log -- -max_total_time=60
cargo +nightly fuzz run tex_log artifacts/tex_log/crash-…   # reproduce a crash
```

CI runs each target for 30 seconds on every push and for 10 minutes every week. A crash found
there is downloaded from the run's artifacts, fixed, and added as a regular test of its crate,
so that every platform checks it.
