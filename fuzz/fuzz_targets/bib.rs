//! .bib files: every range the parser reports must slice the source.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|src: &str| {
    let entries = tex_project::bib::parse(src);
    for entry in &entries {
        assert_eq!(&src[entry.key_range.clone()], entry.key);
    }
    let _ = tex_project::check::check_bib(std::path::Path::new("/project/refs.bib"), &entries);
});
