//! Time and memory of loading a SyncTeX file, as the PDF pane does after every build.
//!
//!     cargo run --release -p synctex --example synctex-perf -- [path/to/doc.synctex.gz]
//!
//! Without a path, the sample project's file (`tests/fixtures/synctex`) is loaded.

use std::hint::black_box;
use std::time::{Duration, Instant};

use synctex::SyncTex;
use test_support::alloc::{self, human};

#[global_allocator]
static ALLOC: alloc::Counting = alloc::Counting;

const RUNS: usize = 5;

fn main() {
    let path = std::env::args_os().nth(1).map_or_else(
        || test_support::fixtures().join("synctex/sample.synctex.gz"),
        Into::into,
    );
    let size = std::fs::metadata(&path).expect("a SyncTeX file").len();
    let mut best = Duration::MAX;
    let (mut kept, mut peak) = (0, 0);
    for _ in 0..RUNS {
        let before = alloc::current();
        alloc::reset_peak();
        let start = Instant::now();
        let st = black_box(SyncTex::open(&path).expect("a readable SyncTeX file"));
        best = best.min(start.elapsed());
        kept = alloc::current().saturating_sub(before);
        peak = alloc::peak().saturating_sub(before);
        drop(st);
    }
    println!(
        "{} ({} on disk), best of {RUNS} runs: {best:.2?}, kept {}, peak {}",
        path.display(),
        human(size as usize),
        human(kept),
        human(peak)
    );
}
