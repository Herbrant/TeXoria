//! Time and memory of loading a Hunspell dictionary, as the app does for each language used.
//!
//!     cargo run --release -p tex-spell --example speller-perf -- path/to/index.aff path/to/index.dic
//!
//! The app's downloaded dictionaries are in its cache folder (`dictionaries/<commit>/<code>`).

use std::time::Instant;

use test_support::alloc::{self, human};

#[global_allocator]
static ALLOC: alloc::Counting = alloc::Counting;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let (Some(aff), Some(dic)) = (args.next(), args.next()) else {
        eprintln!("usage: speller-perf AFF DIC");
        std::process::exit(2);
    };
    let aff = std::fs::read_to_string(aff).expect("a readable .aff file");
    let dic = std::fs::read_to_string(dic).expect("a readable .dic file");
    let before = alloc::current();
    alloc::reset_peak();
    let start = Instant::now();
    let speller = tex_spell::Speller::new(&aff, &dic).expect("a dictionary spellbook reads");
    let elapsed = start.elapsed();
    println!(
        "{} of .dic: loaded in {elapsed:.2?}, kept {}, peak {}",
        human(dic.len()),
        human(alloc::current().saturating_sub(before)),
        human(alloc::peak().saturating_sub(before))
    );
    drop(speller);
}
