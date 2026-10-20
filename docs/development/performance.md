# Performance

How to measure TeXoria's memory and CPU, and the numbers to compare a change against. Measure
before and after a change that may cost either, on the same machine, with release builds.

## The app

```sh
cargo build --release -p texoria
./scripts/measure.sh [-s SECONDS] [-w WARMUP] path/to/document.tex
```

`scripts/measure.sh` starts the app on a document, waits for the warm-up (the first build, the
PDF, the indexes), then samples it for a while and prints its memory (the footprint on macOS, what
Activity Monitor shows; the RSS elsewhere), at the end and at its highest, and its average CPU.
Leave the window alone for an idle measure (it should be close to 0%), or type in it for an
editing one. The app runs with its own home folder, `target/measure-home`: default settings, and
the user's recent projects and sessions are left alone.

## The indexing and the checks

What the editor runs after an edit (parsing, the index, the spelling's words, the word count, the
checks) is timed on a generated project, without the UI:

```sh
cargo run --release -p tex-project --example index-perf -- [chapters] [lines per chapter]
cargo run --release -p synctex --example synctex-perf -- [path/to/doc.synctex.gz]
```

Each step prints its best time of five runs, the memory its result keeps and the most it used
(counted by `test_support::alloc::Counting`, so tree-sitter's own allocations, made in C, are not
included).

## Reference numbers

Apple M5, macOS, release build, October 2026. Generated documents: `index-perf`'s chapters (a
heading every 25 lines, equations, references and citations) and, for the app, 400 sections of
25 lines.

| App                                        | Memory | Peak   | CPU (idle) |
| ------------------------------------------ | -----: | -----: | ---------: |
| Sample project (`tests/fixtures/sample`)   | 138 MB | 181 MB | 0.6%       |
| 10,000 lines (soft wrap off: > 4000 lines) | 249 MB | 249 MB | 0.1%       |

| `index-perf`                    | 500 lines (36 KB) | 10,000 lines (728 KB) |
| ------------------------------- | ----------------: | --------------------: |
| `tex_syntax::parse`             | 5.1 ms            | 68 ms                 |
| `Index::from_text`              | 5.1 ms            | 89 ms                 |
| `extract::words` (spelling)     | 1.2 ms            | 24 ms                 |
| `extract::count` (word count)   | 8.4 ms            | 197 ms                |
| `Project::update` (one buffer)  | 4.3 ms            | 88 ms                 |
| `check::run` (whole project)    | 0.13 ms           | 0.05 ms               |

`synctex-perf` on a real 95 KB `.synctex.gz`: 2.4 ms, 635 KB kept (about 6.5 times the
compressed size).
