//! The text sent to LanguageTool: prose pieces must cover the source in order, and offsets
//! read back in UTF-16 units must land on characters.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tex_spell::{extract, grammar};

fuzz_target!(|input: (&str, usize)| {
    let (text, utf16) = input;
    let _ = grammar::utf16_to_byte(text, utf16).map(|b| assert!(text.is_char_boundary(b)));
    let Some(tree) = tex_syntax::parse(text, None) else {
        return;
    };
    let pieces = grammar::annotate(&tree, text);
    let mut at = 0;
    for p in &pieces {
        assert_eq!(p.range.start, at, "pieces leave a gap or overlap");
        at = p.range.end;
    }
    let _ = grammar::chunks(text, pieces);
    for w in extract::words(&tree, text) {
        assert!(text.get(w.clone()).is_some());
    }
    let _ = extract::count(&tree, text);
});
