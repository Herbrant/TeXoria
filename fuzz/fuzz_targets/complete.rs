//! Completion, hover and navigation at any offset of any text.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (&str, usize)| {
    let (text, offset) = input;
    let offset = offset % (text.len() + 1);
    let _ = tex_complete::context(text, offset);
    if text.is_char_boundary(offset) {
        let _ = tex_complete::complete(text, offset, None, "  ");
    }
    if let Some(symbol) = tex_complete::nav::symbol_at(text, offset) {
        assert!(text.get(symbol.range()).is_some());
    }
    let (out, cursor) = tex_complete::expand_snippet(text);
    assert!(out.is_char_boundary(cursor));
});
