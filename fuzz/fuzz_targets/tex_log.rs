//! Engine logs: whatever bytes an engine (or a project's file) left in the build folder.
#![no_main]

use std::path::Path;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|log: &[u8]| {
    let root = Path::new("/project/main.tex");
    let _ = tex_log::parse_bytes(log, root);
    if let Ok(text) = std::str::from_utf8(log) {
        let _ = tex_log::needs_shell_escape(text);
        let _ = tex_log::parse(text, root);
    }
});
