//! URLs typed in the settings or found in a repository's remotes.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tex_spell::grammar::{is_local, local_port};
use url::{Host, Url};

fuzz_target!(|url: &str| {
    // Only loopback servers count as local: where an HTTP client would connect (a URL it
    // cannot parse never gets that far).
    if let Ok(parsed) = Url::parse(url)
        && matches!(parsed.scheme(), "http" | "https")
    {
        let loopback = match parsed.host() {
            Some(Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.to_canonical().is_loopback(),
            None => false,
        };
        if is_local(url) {
            assert!(loopback, "{url} is not local");
        }
        if let Some(port) = local_port(url) {
            assert!(loopback && parsed.scheme() == "http", "{url} is not local");
            assert_eq!(parsed.port_or_known_default(), Some(port), "{url}");
        }
    }
    let _ = tex_spell::grammar::validate_url(url);
    let servers = ["https://latex.example.org".to_string()];
    if let Some(project) = tex_git::overleaf::project(url, &servers) {
        assert!(project.git_url.starts_with("https://"));
    }
    let _ = tex_git::clone_dir_name(url);
});
