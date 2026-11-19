//! `site-docs <docs dir> <template> <out dir>`: see the library.

fn main() {
    if let Err(error) = site_docs::run(std::env::args().skip(1)) {
        eprintln!("site-docs: {error:#}");
        std::process::exit(1);
    }
}
