//! LaTeX sources: the index's ranges must slice the text and name what they say.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    let index = tex_syntax::Index::from_text(text);
    for label in &index.labels {
        assert_eq!(&text[label.range.clone()], label.name);
    }
    for r in &index.references {
        assert_eq!(&text[r.range.clone()], r.name);
    }
});
