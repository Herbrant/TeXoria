//! URLs typed in the settings or found in a repository's remotes.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|url: &str| {
    if tex_spell::grammar::is_local(url) {
        // Only loopback servers count as local.
        assert!(tex_spell::grammar::local_port(url).is_some() || !url.contains("://"));
    }
    let _ = tex_spell::grammar::validate_url(url);
    let servers = ["https://latex.example.org".to_string()];
    if let Some(project) = tex_git::overleaf::project(url, &servers) {
        assert!(project.git_url.starts_with("https://"));
    }
    let _ = tex_git::clone_dir_name(url);
});
