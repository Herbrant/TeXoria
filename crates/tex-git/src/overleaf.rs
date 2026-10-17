//! Overleaf's Git integration.
//!
//! Overleaf serves each project as `https://git.overleaf.com/<id>`, a Server
//! Pro instance as `https://<host>/git/<id>`. Git signs in with the username
//! `git` and an authentication token as password. There are no branches or
//! tags, and the project must already exist: it cannot be created by a push.

/// Where Overleaf's own Git server is.
pub const CLOUD_GIT: &str = "https://git.overleaf.com";

/// The username that goes with an authentication token.
pub const USERNAME: &str = "git";

/// Whether `token` looks like an authentication token (`olp_…`): printable
/// ASCII without spaces, so it cannot break the credential protocol.
pub fn valid_token(token: &str) -> bool {
    (8..=512).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_graphic())
}

/// A project on Overleaf or on a Server Pro instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// The Git server (`https://git.overleaf.com`): tokens are per server.
    pub server: String,
    pub id: String,
    /// What to clone.
    pub git_url: String,
}

/// The project behind what a user pastes: the project's address in the
/// browser, its Git URL or its bare ID. `servers` are the Server Pro
/// instances the user works with (`https://latex.example.org`); only HTTPS
/// ones count, since the token would otherwise travel in clear text.
pub fn project(input: &str, servers: &[String]) -> Option<Project> {
    let input = input.trim();
    if is_id(input) {
        return Some(cloud(input));
    }
    from_url(&Url::parse(input)?, servers, true)
}

/// The Overleaf project a remote's URL points at.
pub fn remote(url: &str, servers: &[String]) -> Option<Project> {
    from_url(&Url::parse(url)?, servers, false)
}

/// `browser`: also accept the project's page, not only its Git URL.
fn from_url(url: &Url, servers: &[String], browser: bool) -> Option<Project> {
    let segments: Vec<&str> = url.path.split('/').filter(|s| !s.is_empty()).collect();
    if url.host == "git.overleaf.com" {
        return match segments[..] {
            [id] if is_id(id) => Some(cloud(id)),
            _ => None,
        };
    }
    if url.host == "overleaf.com" || url.host.ends_with(".overleaf.com") {
        return match segments[..] {
            ["project", id, ..] if browser && is_id(id) => Some(cloud(id)),
            _ => None,
        };
    }
    let server = servers
        .iter()
        .filter_map(|s| {
            if s.contains("://") {
                Url::parse(s)
            } else {
                Url::parse(&format!("https://{s}"))
            }
        })
        .find(|s| s.scheme == "https" && s.host == url.host)?;
    let id = match segments[..] {
        ["git", id] => id,
        ["project", id, ..] if browser => id,
        _ => return None,
    };
    if !is_id(id) {
        return None;
    }
    let base = format!("{}://{}", server.scheme, server.host);
    Some(Project {
        git_url: format!("{base}/git/{id}"),
        server: base,
        id: id.to_string(),
    })
}

/// The page where the user creates authentication tokens.
pub fn token_page(server: &str) -> String {
    if server == CLOUD_GIT {
        "https://www.overleaf.com/user/settings".to_string()
    } else {
        format!("{server}/user/settings")
    }
}

fn cloud(id: &str) -> Project {
    Project {
        server: CLOUD_GIT.to_string(),
        id: id.to_string(),
        git_url: format!("{CLOUD_GIT}/{id}"),
    }
}

fn is_id(s: &str) -> bool {
    s.len() >= 6 && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The parts of an `http(s)` URL we need, without credentials, query or
/// fragment.
struct Url {
    scheme: &'static str,
    /// Lowercase, with the port if any.
    host: String,
    path: String,
}

impl Url {
    fn parse(s: &str) -> Option<Url> {
        let (scheme, rest) = s.trim().split_once("://")?;
        let scheme = if scheme.eq_ignore_ascii_case("https") {
            "https"
        } else if scheme.eq_ignore_ascii_case("http") {
            "http"
        } else {
            return None;
        };
        let rest = rest.split(['?', '#']).next().unwrap_or_default();
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = authority.rsplit('@').next().unwrap_or_default();
        // The host ends up in a git config key: nothing but a hostname, an
        // IP address and a port. A backslash is read as a slash by some.
        let host_char = |b: u8| b.is_ascii_alphanumeric() || b"-.:[]".contains(&b);
        if host.is_empty() || !host.bytes().all(host_char) || authority.contains('\\') {
            return None;
        }
        Some(Url {
            scheme,
            host: host.to_ascii_lowercase(),
            path: path.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "64ab12cd34ef56ab78cd90ef";

    #[test]
    fn cloud_projects() {
        let want = Some(Project {
            server: CLOUD_GIT.into(),
            id: ID.into(),
            git_url: format!("https://git.overleaf.com/{ID}"),
        });
        for input in [
            ID.to_string(),
            format!("  https://www.overleaf.com/project/{ID}  "),
            format!("https://www.overleaf.com/project/{ID}/"),
            format!("https://www.overleaf.com/project/{ID}?x=1#y"),
            format!("https://cs.overleaf.com/project/{ID}"),
            format!("https://git.overleaf.com/{ID}"),
            format!("https://git@git.overleaf.com/{ID}"),
            format!("HTTPS://Git.Overleaf.com/{ID}/"),
        ] {
            assert_eq!(project(&input, &[]), want, "{input}");
        }
    }

    #[test]
    fn server_pro_projects() {
        let servers = [
            "https://latex.example.org".to_string(),
            "tex.example.com:8443".into(),
        ];
        let p = project(&format!("https://latex.example.org/project/{ID}"), &servers).unwrap();
        assert_eq!(p.server, "https://latex.example.org");
        assert_eq!(p.git_url, format!("https://latex.example.org/git/{ID}"));
        let p = project(
            &format!("https://git@tex.example.com:8443/git/{ID}"),
            &servers,
        )
        .unwrap();
        assert_eq!(p.git_url, format!("https://tex.example.com:8443/git/{ID}"));
        assert_eq!(
            project(&format!("https://other.org/project/{ID}"), &servers),
            None
        );
        // The token must not travel in clear text.
        let plain = ["http://latex.example.org".to_string()];
        assert_eq!(
            project(&format!("http://latex.example.org/git/{ID}"), &plain),
            None
        );
        let p = project(&format!("http://git.overleaf.com/{ID}"), &[]).unwrap();
        assert_eq!(p.git_url, format!("https://git.overleaf.com/{ID}"));
    }

    #[test]
    fn rejects_other_input() {
        for input in [
            "",
            "abc",
            "https://www.overleaf.com/read/abcdefghijkl",
            "https://www.overleaf.com/project",
            "https://git.overleaf.com/",
            "https://github.com/you/paper.git",
            "git@github.com:you/paper.git",
            "../paper",
            "https://git.overleaf.com\\@evil.example/64ab12cd34ef",
            "https://evil example/project/64ab12cd34ef",
            "https://git.overleaf.com=x/64ab12cd34ef",
        ] {
            assert_eq!(project(input, &[]), None, "{input}");
        }
    }

    #[test]
    fn remotes() {
        let servers = ["https://latex.example.org".to_string()];
        let url = format!("https://git.overleaf.com/{ID}");
        assert_eq!(remote(&url, &servers).unwrap().server, CLOUD_GIT);
        let url = format!("https://latex.example.org/git/{ID}");
        assert_eq!(
            remote(&url, &servers).unwrap().server,
            "https://latex.example.org"
        );
        assert_eq!(remote(ID, &servers), None);
        assert_eq!(
            remote(&format!("https://www.overleaf.com/project/{ID}"), &[]),
            None
        );
        assert_eq!(remote("https://github.com/you/paper.git", &servers), None);
    }

    #[test]
    fn tokens() {
        assert!(valid_token("olp_abcdefghijklmnopqrstuvwxyz0123456789"));
        assert!(!valid_token(""));
        assert!(!valid_token("olp_abc\nquit=1"));
        assert!(!valid_token("olp_abc def"));
        assert!(!valid_token("olp_àèìòù_xyz"));
    }

    #[test]
    fn token_pages() {
        assert_eq!(
            token_page(CLOUD_GIT),
            "https://www.overleaf.com/user/settings"
        );
        assert_eq!(
            token_page("https://latex.example.org"),
            "https://latex.example.org/user/settings"
        );
    }

    #[test]
    fn token_lengths() {
        assert!(!valid_token(&"a".repeat(7)));
        assert!(valid_token(&"a".repeat(8)));
        assert!(valid_token(&"a".repeat(512)));
        assert!(!valid_token(&"a".repeat(513)));
        assert!(valid_token("olp_!#$%&'()*+,-./:;<=>?@[]^_`{|}~"));
        assert!(!valid_token("olp_abcdef\t"));
        assert!(!valid_token("olp_abcdef\u{7f}"));
    }

    #[test]
    fn hosts_with_ports_and_addresses() {
        let servers = [
            "https://[::1]:8443".to_string(),
            "https://10.0.0.5".into(),
            "localhost:8080".into(),
        ];
        let p = project(&format!("https://[::1]:8443/project/{ID}"), &servers).unwrap();
        assert_eq!(p.server, "https://[::1]:8443");
        assert_eq!(p.git_url, format!("https://[::1]:8443/git/{ID}"));
        let p = remote(&format!("https://10.0.0.5/git/{ID}"), &servers).unwrap();
        assert_eq!(p.git_url, format!("https://10.0.0.5/git/{ID}"));
        // Without a scheme, a server is taken as HTTPS.
        let p = remote(&format!("https://LOCALHOST:8080/git/{ID}"), &servers).unwrap();
        assert_eq!(p.server, "https://localhost:8080");
        // The port is part of the server.
        assert_eq!(
            remote(&format!("https://10.0.0.5:444/git/{ID}"), &servers),
            None
        );
        assert_eq!(remote(&format!("https://[::1]/git/{ID}"), &servers), None);
    }

    #[test]
    fn credentials_in_urls_are_dropped() {
        // The host is what follows the last `@`.
        let p = remote(&format!("https://git:olp_x@y@git.overleaf.com/{ID}"), &[]).unwrap();
        assert_eq!(p.git_url, format!("https://git.overleaf.com/{ID}"));
        assert_eq!(
            remote(&format!("https://git.overleaf.com@evil.example/{ID}"), &[]),
            None
        );
        let servers = ["https://latex.example.org".to_string()];
        let p = remote(
            &format!("https://user:pass@latex.example.org/git/{ID}?x#y"),
            &servers,
        )
        .unwrap();
        assert!(!p.git_url.contains("pass") && !p.git_url.contains('?'));
    }

    #[test]
    fn server_pro_needs_https() {
        let id = ID;
        for server in [
            "http://latex.example.org",
            "ftp://latex.example.org",
            "",
            "a b",
        ] {
            let servers = [server.to_string()];
            assert_eq!(
                project(&format!("https://latex.example.org/git/{id}"), &servers),
                None,
                "{server}"
            );
        }
        // An HTTP address of an HTTPS server is cloned over HTTPS.
        let servers = ["https://latex.example.org".to_string()];
        let p = project(&format!("http://latex.example.org/project/{id}"), &servers).unwrap();
        assert_eq!(p.git_url, format!("https://latex.example.org/git/{id}"));
        // Project pages are not remotes, and IDs must look like IDs.
        assert_eq!(
            remote(&format!("https://latex.example.org/project/{id}"), &servers),
            None
        );
        for path in ["git/abc", "git/abc-def-ghi", "git", &format!("git/{id}/x")] {
            assert_eq!(
                project(&format!("https://latex.example.org/{path}"), &servers),
                None,
                "{path}"
            );
        }
        assert_eq!(
            project(&format!("https://www.overleaf.com/project/{id}x!"), &[]),
            None
        );
        assert_eq!(
            project(&format!("https://git.overleaf.com/a/{id}"), &[]),
            None
        );
    }

    mod props {
        use proptest::prelude::*;

        use super::*;

        fn check(p: &Project) -> Result<(), TestCaseError> {
            prop_assert!(p.id.len() >= 6 && p.id.bytes().all(|b| b.is_ascii_alphanumeric()));
            prop_assert!(p.server.starts_with("https://"), "{p:?}");
            prop_assert_eq!(
                &p.git_url,
                &if p.server == CLOUD_GIT {
                    format!("{CLOUD_GIT}/{}", p.id)
                } else {
                    format!("{}/git/{}", p.server, p.id)
                }
            );
            let host = &p.server["https://".len()..];
            prop_assert!(
                !host.is_empty()
                    && host
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-.:[]".contains(&b))
            );
            Ok(())
        }

        proptest! {
            #[test]
            fn any_input(input in "\\PC{0,60}", server in "\\PC{0,30}") {
                let servers = [server];
                if let Some(p) = project(&input, &servers) {
                    check(&p)?;
                }
                if let Some(p) = remote(&input, &servers) {
                    check(&p)?;
                }
            }

            #[test]
            fn url_like_input(
                scheme in "(https?|HTTP|ftp)",
                user in prop::option::of("[a-z:@_]{1,8}"),
                host in prop_oneof![
                    Just("git.overleaf.com".to_string()),
                    Just("www.overleaf.com".to_string()),
                    Just("latex.example.org".to_string()),
                    Just("latex.example.org:8443".to_string()),
                    Just("[::1]".to_string()),
                    "[a-zA-Z0-9.:\\[\\]@\\\\=-]{1,20}",
                ],
                path in prop_oneof![
                    "[a-zA-Z0-9]{4,10}",
                    "project/[a-zA-Z0-9]{4,10}(/[a-z]{0,4})?",
                    "git/[a-zA-Z0-9]{4,10}",
                    "[a-z0-9/!._-]{0,30}",
                ],
                server in prop_oneof![
                    Just("https://latex.example.org".to_string()),
                    Just("latex.example.org:8443".to_string()),
                    Just("http://latex.example.org".to_string()),
                    Just("https://[::1]".to_string()),
                    "(https?://)?[a-z0-9.:]{1,15}",
                ],
                tail in "([?#][a-z=&]{0,6})?",
            ) {
                let user = user.map(|u| format!("{u}@")).unwrap_or_default();
                let input = format!("{scheme}://{user}{host}/{path}{tail}");
                let servers = [server];
                for p in [project(&input, &servers), remote(&input, &servers)]
                    .into_iter()
                    .flatten()
                {
                    check(&p)?;
                }
            }

            #[test]
            fn bare_ids(id in "[a-zA-Z0-9]{0,30}") {
                match project(&id, &[]) {
                    Some(p) => {
                        prop_assert!(id.len() >= 6);
                        check(&p)?;
                    }
                    None => prop_assert!(id.len() < 6),
                }
            }

            #[test]
            fn tokens_are_single_words(token in "\\PC{0,40}") {
                if valid_token(&token) {
                    prop_assert!(!token.chars().any(|c| c.is_whitespace() || c.is_control()));
                    prop_assert!(token.is_ascii());
                }
            }
        }
    }
}
