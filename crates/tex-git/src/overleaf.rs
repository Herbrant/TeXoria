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
}
