//! What a user pastes to cite (a DOI, an arXiv id, a URL) and the BibTeX servers send back.
#![no_main]

use std::collections::HashSet;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (&str, &str)| {
    let (pasted, bibtex) = input;
    let _ = tex_cite::classify(pasted);
    let key = tex_cite::make_key(bibtex, &HashSet::new());
    assert!(!key.is_empty());
    if let Some(entry) = tex_cite::normalize(bibtex, &key) {
        assert_eq!(
            tex_cite::normalize(&entry, &key).as_deref(),
            Some(entry.as_str())
        );
    }
});
