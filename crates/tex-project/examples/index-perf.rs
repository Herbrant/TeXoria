//! Time and memory of what the editor runs after an edit, on a generated project.
//!
//!     cargo run --release -p tex-project --example index-perf -- [chapters] [lines per chapter]
//!
//! The project is a root that `\input`s every chapter and a bibliography; each step is run a
//! few times and its best time is printed, with the memory it keeps (and the most it used).

use std::fmt::Write as _;
use std::hint::black_box;
use std::path::Path;
use std::time::{Duration, Instant};

use test_support::alloc::{self, human};

#[global_allocator]
static ALLOC: alloc::Counting = alloc::Counting;

const RUNS: usize = 5;

fn main() {
    let mut args = std::env::args()
        .skip(1)
        .map(|a| a.parse::<usize>().expect("a number"));
    let chapters = args.next().unwrap_or(20);
    let lines = args.next().unwrap_or(500);

    let dir = tempfile::tempdir().unwrap();
    let root = write_project(dir.path(), chapters, lines);
    let chapter = dir.path().join("chapters/ch0.tex");
    let text = std::fs::read_to_string(&chapter).unwrap();
    println!(
        "{chapters} chapters of {lines} lines ({} each), best of {RUNS} runs",
        human(text.len())
    );
    println!(
        "{:<34} {:>10} {:>10} {:>10}",
        "step", "time", "kept", "peak"
    );

    measure("tex_syntax::parse (one chapter)", || {
        tex_syntax::parse(&text, None)
    });
    measure("Index::from_text (one chapter)", || {
        tex_syntax::Index::from_text(&text)
    });
    let tree = tex_syntax::parse(&text, None).unwrap();
    measure("extract::words (one chapter)", || {
        tex_spell::extract::words(&tree, &text)
    });
    measure("extract::count (one chapter)", || {
        tex_spell::extract::count(&tree, &text)
    });
    measure("Project::open", || tex_project::Project::open(&root));
    let mut project = tex_project::Project::open(&root);
    measure("Project::update (one chapter)", || {
        project.update(&chapter, &text);
    });
    measure("check::run", || tex_project::check::run(&project));
    measure("Project::outline", || project.outline());
}

/// Run `step` `RUNS` times and print its best time, the bytes its result holds and the most
/// it used at once.
fn measure<T>(name: &str, mut step: impl FnMut() -> T) {
    let mut best = Duration::MAX;
    let (mut kept, mut peak) = (0, 0);
    for _ in 0..RUNS {
        let before = alloc::current();
        alloc::reset_peak();
        let start = Instant::now();
        let out = black_box(step());
        best = best.min(start.elapsed());
        kept = alloc::current().saturating_sub(before);
        peak = alloc::peak().saturating_sub(before);
        drop(out);
    }
    println!(
        "{name:<34} {:>10} {:>10} {:>10}",
        format!("{:.2?}", best),
        human(kept),
        human(peak)
    );
}

/// A root including `chapters` chapters of about `lines` lines each, with sections, labels,
/// references, citations and math; returns the root's path.
fn write_project(dir: &Path, chapters: usize, lines: usize) -> std::path::PathBuf {
    std::fs::create_dir(dir.join("chapters")).unwrap();
    let mut root =
        String::from("\\documentclass{article}\n\\usepackage{amsmath}\n\\begin{document}\n");
    for c in 0..chapters {
        writeln!(root, "\\input{{chapters/ch{c}}}").unwrap();
        std::fs::write(dir.join(format!("chapters/ch{c}.tex")), chapter(c, lines)).unwrap();
    }
    root.push_str("\\bibliographystyle{plain}\n\\bibliography{refs}\n\\end{document}\n");
    let mut bib = String::new();
    for k in 0..100 {
        writeln!(
            bib,
            "@article{{key{k},\n  author = {{Ada Lovelace}},\n  title = {{Title {k}}},\n  \
             journal = {{Journal}},\n  year = {{1843}},\n}}\n"
        )
        .unwrap();
    }
    std::fs::write(dir.join("refs.bib"), bib).unwrap();
    let path = dir.join("main.tex");
    std::fs::write(&path, root).unwrap();
    path
}

fn chapter(c: usize, lines: usize) -> String {
    let mut s = format!("\\section{{Chapter {c}}}\n\\label{{sec:{c}}}\n");
    for l in 0..lines {
        match l % 25 {
            0 => writeln!(s, "\\subsection{{Part {l}}}\n\\label{{sub:{c}:{l}}}").unwrap(),
            10 => writeln!(
                s,
                "\\begin{{equation}}\n  e^{{i\\pi}} + {l} = \\int_0^1 x\\,dx\n  \
                 \\label{{eq:{c}:{l}}}\n\\end{{equation}}"
            )
            .unwrap(),
            20 => writeln!(
                s,
                "As Section~\\ref{{sec:{c}}} shows, see \\cite{{key{}}} and \\emph{{this}}.",
                l % 100
            )
            .unwrap(),
            _ => s.push_str(
                "The quick brown fox jumps over the lazy dog, with $x^2 + y^2 = z^2$ inline.\n",
            ),
        }
    }
    s
}
