//! SyncTeX files, after the header they must start with; then forward and inverse search.
#![no_main]

use std::path::Path;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (&str, u32, f32, f32)| {
    let (body, line, x, y) = input;
    let text = format!("SyncTeX Version:1\nInput:1:/project/main.tex\n{body}");
    let Ok(sync) = synctex::SyncTex::parse(&text, Path::new("/project")) else {
        return;
    };
    let main = Path::new("/project/main.tex");
    let _ = sync.forward(main, line);
    for page in 0..sync.page_count().min(4) {
        let _ = sync.inverse(page, x, y);
    }
});
