# Performance

How to measure TeXoria's memory and CPU, and the numbers to compare a change against. Measure
before and after a change that may cost either, on the same machine, with release builds.

## The app

```sh
cargo build --release -p texoria
./scripts/measure.sh [-s SECONDS] [-w WARMUP] [-b BINARY] path/to/document.tex
```

`scripts/measure.sh` starts the app on a document, waits for the warm-up (the first build, the
PDF, the indexes), brings its window to the front (macOS: a hidden window draws nothing), then
prints its average CPU over a while and its memory. On macOS that is the footprint (what Activity
Monitor shows), the bytes allocated in malloc (the app's own data: the steadiest number, compare
this one first) and the GPU's memory (the PDF's pages, glyphs); elsewhere, the RSS. Leave the
window alone for an idle measure, or type in it for an editing one. The app runs with its own
home folder, `target/measure-home`: default settings, and the user's recent projects and
sessions are left alone.

Pitfalls met while measuring:

- Nothing may sample the app while its CPU is measured: `footprint` and `vmmap` walk its memory
  and the app is charged for it (a second of CPU every 30 seconds with a big document).
- On macOS the RSS counts the system's shared libraries (about 600 MB): it says little.
- A binary built in another checkout (a `git worktree` of an older commit, to compare with)
  looks for PDFium in that checkout's `vendor/` and only when it runs from that checkout's
  `target/`: without it, it shows no PDF and seems to need less memory. Link `vendor/` there and
  copy the binary under its `target/`.
- The footprint varies by tens of MB from one run to the next; the malloc and GPU numbers by
  less than one. Run each binary a few times, alternating them.

## The indexing and the checks

What the editor runs after an edit (parsing, the index, the spelling's words, the word count, the
checks) and after a build is timed without the UI:

```sh
cargo run --release -p tex-project --example index-perf -- [chapters] [lines per chapter]
cargo run --release -p synctex --example synctex-perf -- [path/to/doc.synctex.gz]
cargo run --release -p tex-spell --example speller-perf -- path/to/index.aff path/to/index.dic
```

Each step prints its best time of five runs, the memory its result keeps and the most it used
(counted by `test_support::alloc::Counting`, so tree-sitter's own allocations, made in C, are not
included).

## Reference numbers

Apple M5, macOS, release builds, October 2026. Generated documents: `index-perf`'s chapters (a
heading every 25 lines, equations, references and citations) and, for the app, 400 sections of
25 lines (10,000 lines, about 250 pages). Idle, window in front, PDF shown.

| App, 10,000 lines              | Before (`a99ce8e`) | After (`cfffba8`) |
| ------------------------------ | -----------------: | ----------------: |
| CPU, idle                      | 5.0%               | 2.2%              |
| Allocated in malloc            | 113.7 MB           | 99.4 MB           |
| Footprint                      | 310–344 MB         | 279–281 MB        |
| GPU memory                     | 20.6 MB            | 20.6 MB           |

The sample project (`tests/fixtures/sample`): 1.5% of CPU idle (the cursor's blinks redraw the
window, and the editor highlights its lines again on each frame), 181 MB footprint, 15.7 MB in
malloc.

| `index-perf`                    | 500 lines (36 KB) | 10,000 lines (728 KB) |
| ------------------------------- | ----------------: | --------------------: |
| `tex_syntax::parse`             | 4.4 ms            | 68 ms                 |
| `Index::from_text`              | 4.8 ms            | 89 ms                 |
| `extract::words` (spelling)     | 1.2 ms            | 25 ms                 |
| `extract::count` (word count)   | 1.7 ms            | 38 ms (197 ms before) |
| `Project::update` (one buffer)  | 4.5 ms            | 90 ms                 |
| `check::run` (whole project)    | 0.12 ms           | 0.05 ms               |

After an edit the app parses only the buffers that changed, once, on a background thread (the
index and the word count share the tree); the spelling parses the text again on its own.

`synctex-perf` on a real 95 KB `.synctex.gz`: 2.4 ms, 635 KB kept (about 6.5 times the
compressed size). `speller-perf`: the English dictionary loads in 9 ms and keeps 2.1 MB, the
Italian one 11 ms and 4.5 MB.
