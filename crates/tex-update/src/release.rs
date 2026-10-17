//! Which release is the latest, and the signed list of its files.
//!
//! A release has `SHA256SUMS` (the hash of every package) and
//! `SHA256SUMS.minisig`, its minisign signature made by the release workflow
//! with a key only it holds. The trusted comment of the signature names the
//! version, so the list of one release cannot be passed off as another's.

use std::collections::HashMap;
use std::io::Read;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use minisign_verify::{PublicKey, Signature};
use semver::Version;

/// The releases of the project on GitHub.
pub const RELEASES: &str = "https://github.com/Herbrant/TeXoria/releases";
/// Replaces [`RELEASES`], to try the updater against a local server.
pub const URL_VAR: &str = "TEXORIA_UPDATE_URL";
/// The public half of the release workflow's signing key.
const PUBLIC_KEY: &str = include_str!("texoria.pub");
/// Replaces [`PUBLIC_KEY`] in debug builds, to sign test releases.
#[cfg(debug_assertions)]
const KEY_VAR: &str = "TEXORIA_UPDATE_KEY";

/// `SHA256SUMS` lists a few dozen files.
const MAX_SUMS: u64 = 64 << 10;
const TIMEOUT: Duration = Duration::from_secs(30);

/// Where releases come from, and the key their files must be signed with.
pub struct Source {
    base: String,
    key: PublicKey,
}

impl Source {
    /// The project's GitHub releases, unless [`URL_VAR`] says otherwise.
    pub fn new() -> Result<Self> {
        Self::from_env(|name| std::env::var(name).ok())
    }

    /// [`Source::new`], with `var` reading the environment. [`URL_VAR`] counts
    /// in release builds too, plain HTTP included: what makes a release
    /// trustworthy is its signature by the embedded key, not where it came
    /// from. Only debug builds take another key.
    fn from_env(var: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let base = var(URL_VAR).unwrap_or_else(|| RELEASES.to_string());
        #[cfg(debug_assertions)]
        if let Some(key) = var(KEY_VAR) {
            return Self::with_key(&base, &key);
        }
        Self::with_key(&base, PUBLIC_KEY)
    }

    /// `key` is the content of a minisign `.pub` file, or just its key line.
    pub fn with_key(base: &str, key: &str) -> Result<Self> {
        let key = key
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))
            .ok_or_else(|| anyhow!("no update signing key"))?;
        let key = PublicKey::from_base64(key).map_err(|e| anyhow!("update signing key: {e}"))?;
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            key,
        })
    }

    /// The page of a release, with its notes.
    pub fn page(&self, version: &Version) -> String {
        format!("{}/tag/v{version}", self.base)
    }

    pub(crate) fn asset_url(&self, version: &Version, name: &str) -> String {
        format!("{}/download/v{version}/{name}", self.base)
    }

    /// The newest release that is not a pre-release. GitHub redirects
    /// `releases/latest` to its page: the version is read from there, which
    /// needs neither the API nor its rate limit.
    pub fn latest_version(&self) -> Result<Version> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .max_redirects(0)
            .user_agent(crate::USER_AGENT)
            .build()
            .into();
        let url = format!("{}/latest", self.base);
        let resp = agent
            .get(&url)
            .call()
            .with_context(|| format!("checking {url}"))?;
        let location = resp
            .headers()
            .get("location")
            .and_then(|l| l.to_str().ok())
            .ok_or_else(|| anyhow!("{url} did not point to a release"))?;
        version_from_location(location)
    }

    /// The signed file list of `version`.
    pub fn release(&self, version: &Version) -> Result<Release> {
        let sums = self.fetch_small(version, "SHA256SUMS")?;
        let sig = self.fetch_small(version, "SHA256SUMS.minisig")?;
        let sig = String::from_utf8(sig).context("SHA256SUMS.minisig is not text")?;
        Release::verify(version.clone(), &sums, &sig, &self.key)
    }

    fn fetch_small(&self, version: &Version, name: &str) -> Result<Vec<u8>> {
        let url = self.asset_url(version, name);
        let mut body = Vec::new();
        crate::agent(TIMEOUT)
            .get(&url)
            .call()
            .with_context(|| format!("downloading {url}"))?
            .body_mut()
            .with_config()
            .limit(MAX_SUMS)
            .reader()
            .read_to_end(&mut body)
            .with_context(|| format!("downloading {url}"))?;
        Ok(body)
    }
}

/// `https://github.com/o/r/releases/tag/v1.2.3` → 1.2.3.
fn version_from_location(location: &str) -> Result<Version> {
    let tag = location
        .trim_end_matches('/')
        .rsplit_once("/tag/")
        .map(|(_, tag)| tag)
        .ok_or_else(|| anyhow!("unexpected release address {location}"))?;
    let v = tag.strip_prefix('v').unwrap_or(tag);
    Version::parse(v).with_context(|| format!("unexpected release tag {tag}"))
}

/// Whether `latest` is worth updating to from `current`.
pub fn is_newer(current: &str, latest: &Version) -> bool {
    Version::parse(current).is_ok_and(|current| *latest > current)
}

/// A release whose file list was checked against the signing key.
#[derive(Debug, Clone)]
pub struct Release {
    pub version: Version,
    sums: HashMap<String, String>,
}

impl Release {
    /// Check the signature of `sums` and parse it.
    pub fn verify(version: Version, sums: &[u8], sig: &str, key: &PublicKey) -> Result<Self> {
        let sig = Signature::decode(sig).map_err(|e| anyhow!("SHA256SUMS.minisig: {e}"))?;
        key.verify(sums, &sig, false)
            .map_err(|e| anyhow!("SHA256SUMS is not signed by TeXoria's release key: {e}"))?;
        let expected = format!("texoria {version}");
        if sig.trusted_comment() != expected {
            bail!(
                "SHA256SUMS is signed for \"{}\", not \"{expected}\"",
                sig.trusted_comment()
            );
        }
        let sums = std::str::from_utf8(sums).context("SHA256SUMS is not text")?;
        Ok(Self {
            version,
            sums: parse_sums(sums)?,
        })
    }

    /// The SHA-256 (lowercase hex) of the release file `name`.
    pub fn sha256(&self, name: &str) -> Option<&str> {
        self.sums.get(name).map(String::as_str)
    }
}

/// `sha256sum` output: `<hex>  <name>`, or `<hex> *<name>` for binary mode.
/// A name listed with two different hashes is an error.
fn parse_sums(text: &str) -> Result<HashMap<String, String>> {
    let mut sums = HashMap::new();
    let entries = text.lines().filter_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim_start();
        let name = name.strip_prefix('*').unwrap_or(name);
        let valid = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
        (valid && !name.is_empty()).then(|| (name.to_string(), hash.to_ascii_lowercase()))
    });
    for (name, hash) in entries {
        if sums.get(&name).is_some_and(|h| *h != hash) {
            bail!("SHA256SUMS lists {name} with two different hashes");
        }
        sums.insert(name, hash);
    }
    Ok(sums)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fresh key pair: the minisign `.pub` text and a signing function.
    pub(crate) fn keys() -> (String, impl Fn(&[u8], &str) -> String) {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let public = kp.pk.to_box().unwrap().into_string();
        let sign = move |data: &[u8], comment: &str| {
            minisign::sign(Some(&kp.pk), &kp.sk, data, Some(comment), None)
                .unwrap()
                .into_string()
        };
        (public, sign)
    }

    const SUMS: &str = "\
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  texoria-1.0.0-macos-aarch64.dmg
FEDCBA9876543210FEDCBA9876543210FEDCBA9876543210FEDCBA9876543210 *texoria-1.0.0-windows-x86_64-setup.exe
nothex  ignored
";

    fn source(public: &str) -> Source {
        Source::with_key(RELEASES, public).unwrap()
    }

    #[test]
    fn verifies_and_parses() {
        let (public, sign) = keys();
        let sig = sign(SUMS.as_bytes(), "texoria 1.0.0");
        let v = Version::new(1, 0, 0);
        let r = Release::verify(v, SUMS.as_bytes(), &sig, &source(&public).key).unwrap();
        assert_eq!(
            r.sha256("texoria-1.0.0-macos-aarch64.dmg"),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
        assert_eq!(
            r.sha256("texoria-1.0.0-windows-x86_64-setup.exe"),
            Some("fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210")
        );
        assert_eq!(r.sha256("ignored"), None);
    }

    #[test]
    fn rejects_tampered_list() {
        let (public, sign) = keys();
        let sig = sign(SUMS.as_bytes(), "texoria 1.0.0");
        let tampered = SUMS.replace("0123", "9999");
        let v = Version::new(1, 0, 0);
        assert!(Release::verify(v, tampered.as_bytes(), &sig, &source(&public).key).is_err());
    }

    #[test]
    fn rejects_other_key() {
        let (_, sign) = keys();
        let (other, _) = keys();
        let sig = sign(SUMS.as_bytes(), "texoria 1.0.0");
        let v = Version::new(1, 0, 0);
        assert!(Release::verify(v, SUMS.as_bytes(), &sig, &source(&other).key).is_err());
    }

    #[test]
    fn rejects_list_of_another_version() {
        let (public, sign) = keys();
        let sig = sign(SUMS.as_bytes(), "texoria 0.9.0");
        let v = Version::new(1, 0, 0);
        let err = Release::verify(v, SUMS.as_bytes(), &sig, &source(&public).key).unwrap_err();
        assert!(err.to_string().contains("texoria 0.9.0"), "{err}");
    }

    /// A signed list naming one file with two hashes is not ours to pick
    /// from (`install.sh` would refuse it too).
    #[test]
    fn rejects_conflicting_hashes() {
        let (public, sign) = keys();
        let a = "0".repeat(64);
        let b = "1".repeat(64);
        let v = Version::new(1, 0, 0);
        let key = source(&public).key;
        let sums = format!("{a}  x.dmg\n{b}  y.dmg\n{b} *x.dmg\n");
        let sig = sign(sums.as_bytes(), "texoria 1.0.0");
        let err = Release::verify(v.clone(), sums.as_bytes(), &sig, &key).unwrap_err();
        assert!(err.to_string().contains("x.dmg"), "{err}");
        // The same hash twice is harmless.
        let sums = format!("{a}  x.dmg\n{}  x.dmg\n", a.to_uppercase());
        let sig = sign(sums.as_bytes(), "texoria 1.0.0");
        let r = Release::verify(v, sums.as_bytes(), &sig, &key).unwrap();
        assert_eq!(r.sha256("x.dmg"), Some(a.as_str()));
    }

    #[test]
    fn embedded_key_is_valid() {
        Source::with_key(RELEASES, PUBLIC_KEY).unwrap();
    }

    #[test]
    fn version_from_redirect() {
        let v = version_from_location("https://github.com/Herbrant/TeXoria/releases/tag/v0.3.0");
        assert_eq!(v.unwrap(), Version::new(0, 3, 0));
        assert!(version_from_location("https://github.com/Herbrant/TeXoria/releases").is_err());
        assert!(version_from_location("https://x/releases/tag/nightly").is_err());
    }

    #[test]
    fn newer_versions() {
        assert!(is_newer("0.2.0", &Version::new(0, 3, 0)));
        assert!(is_newer("0.2.0-rc.1", &Version::new(0, 2, 0)));
        assert!(!is_newer("0.2.0", &Version::new(0, 2, 0)));
        assert!(!is_newer("0.3.0", &Version::new(0, 2, 9)));
    }

    #[test]
    fn urls() {
        let (public, _) = keys();
        let s = Source::with_key("http://localhost:8000/releases/", &public).unwrap();
        let v = Version::new(1, 2, 3);
        assert_eq!(s.page(&v), "http://localhost:8000/releases/tag/v1.2.3");
        assert_eq!(
            s.asset_url(&v, "SHA256SUMS"),
            "http://localhost:8000/releases/download/v1.2.3/SHA256SUMS"
        );
    }

    #[test]
    fn source_from_the_environment() {
        let (public, sign) = keys();
        let none = Source::from_env(|_| None).unwrap();
        assert_eq!(none.base, RELEASES);
        let vars = |name: &str| match name {
            URL_VAR => Some("http://127.0.0.1:8000/r/".to_string()),
            "TEXORIA_UPDATE_KEY" => Some(public.clone()),
            _ => None,
        };
        let s = Source::from_env(vars).unwrap();
        // Any address, even in release builds: the signature is what counts.
        assert_eq!(s.base, "http://127.0.0.1:8000/r");
        let sig = sign(SUMS.as_bytes(), "texoria 1.0.0");
        let verified = Release::verify(Version::new(1, 0, 0), SUMS.as_bytes(), &sig, &s.key);
        // Another key only in debug builds.
        assert_eq!(verified.is_ok(), cfg!(debug_assertions));
        let embedded = Source::from_env(|n| (n == URL_VAR).then(|| "x".into())).unwrap();
        assert!(
            Release::verify(Version::new(1, 0, 0), SUMS.as_bytes(), &sig, &embedded.key).is_err()
        );
        assert!(Source::new().is_ok());
    }

    #[test]
    fn signing_keys() {
        let (public, _) = keys();
        let line = public.lines().nth(1).unwrap();
        assert!(Source::with_key(RELEASES, line).is_ok());
        assert!(Source::with_key(RELEASES, &format!("\n  {line}  \n")).is_ok());
        for bad in [
            "",
            "\n\n",
            "untrusted comment: minisign public key",
            "not a key",
        ] {
            let err = Source::with_key(RELEASES, bad).err().unwrap();
            assert!(err.to_string().contains("signing key"), "{bad:?}: {err}");
        }
    }

    #[test]
    fn rejects_broken_signatures() {
        let (public, sign) = keys();
        let key = source(&public).key;
        let v = Version::new(1, 0, 0);
        let good = sign(SUMS.as_bytes(), "texoria 1.0.0");
        let mut lines: Vec<&str> = good.lines().collect();
        lines.truncate(2);
        let truncated = lines.join("\n");
        for sig in [
            "",
            "untrusted comment: x\n",
            "garbage\nnot base64!\n",
            &truncated,
        ] {
            let err = Release::verify(v.clone(), SUMS.as_bytes(), sig, &key).unwrap_err();
            assert!(err.to_string().contains("SHA256SUMS.minisig"), "{err}");
        }
        // Legacy signatures (`Ed`, of the data itself rather than of its
        // hash) are not accepted: the signature line's second character
        // holds the algorithm.
        let mut legacy: Vec<String> = good.lines().map(str::to_string).collect();
        assert!(legacy[1].starts_with("RU"));
        legacy[1].replace_range(1..2, "W");
        let legacy = legacy.join("\n") + "\n";
        let err = Release::verify(v.clone(), SUMS.as_bytes(), &legacy, &key).unwrap_err();
        assert!(err.to_string().contains("not signed"), "{err}");
        // A trusted comment edited after signing.
        let edited = good.replace("texoria 1.0.0", "texoria 1.0.1");
        let v1 = Version::new(1, 0, 1);
        assert!(Release::verify(v1, SUMS.as_bytes(), &edited, &key).is_err());
    }

    #[test]
    fn signed_list_must_be_text() {
        let (public, sign) = keys();
        let sums = b"\xff\xfe not text";
        let sig = sign(sums, "texoria 1.0.0");
        let err =
            Release::verify(Version::new(1, 0, 0), sums, &sig, &source(&public).key).unwrap_err();
        assert!(err.to_string().contains("not text"), "{err}");
    }

    #[test]
    fn sums_formats() {
        let a = "a".repeat(64);
        let text = format!(
            "{a}  name with spaces.tar.gz\n{a}\tTAB.dmg\n{a} *bin.exe\n{a}  \n{a}\n\
             {}  short\n{}x  long\nzz{}  nothex\n\n",
            &a[..63],
            a,
            &a[..62]
        );
        let sums = parse_sums(&text).unwrap();
        let mut names: Vec<_> = sums.keys().map(String::as_str).collect();
        names.sort();
        assert_eq!(names, ["TAB.dmg", "bin.exe", "name with spaces.tar.gz"]);
    }

    #[test]
    fn redirect_addresses() {
        for (location, want) in [
            ("https://github.com/o/r/releases/tag/v1.2.3/", Some("1.2.3")),
            ("/o/r/releases/tag/1.2.3", Some("1.2.3")),
            ("https://x/releases/tag/v1.2.0-rc.1", Some("1.2.0-rc.1")),
            ("https://x/releases/tag/", None),
            ("https://x/releases/tag/v1.2", None),
            ("", None),
        ] {
            let got = version_from_location(location).ok().map(|v| v.to_string());
            assert_eq!(got.as_deref(), want, "{location}");
        }
        assert!(!is_newer("not a version", &Version::new(9, 0, 0)));
    }

    fn server(
        routes: Vec<(&str, test_support::http::Response)>,
    ) -> (test_support::http::Server, Source) {
        let server = test_support::http::serve_paths(routes);
        let (public, _) = keys();
        let source = Source::with_key(&format!("{}/releases", server.url()), &public).unwrap();
        (server, source)
    }

    #[test]
    fn latest_version_needs_a_redirect_to_a_tag() {
        use test_support::http::Response;

        let (_s, source) = server(vec![("/releases/latest", Response::ok("a page"))]);
        let err = source.latest_version().unwrap_err();
        assert!(
            err.to_string().contains("did not point to a release"),
            "{err}"
        );

        let (_s, source) = server(vec![(
            "/releases/latest",
            Response::redirect("https://x/releases"),
        )]);
        assert!(source.latest_version().is_err());

        let (_s, source) = server(vec![]);
        let err = source.latest_version().unwrap_err();
        assert!(format!("{err:#}").contains("checking"), "{err:#}");

        let (_s, source) = server(vec![(
            "/releases/latest",
            Response::redirect("/o/r/releases/tag/v2.0.0"),
        )]);
        assert_eq!(source.latest_version().unwrap(), Version::new(2, 0, 0));
    }

    #[test]
    fn release_files_are_small_text() {
        use test_support::http::Response;

        let (public, sign) = keys();
        let sig = sign(SUMS.as_bytes(), "texoria 1.0.0");
        let v = Version::new(1, 0, 0);
        let base = "/releases/download/v1.0.0";
        let routes = |sums: Response, minisig: Response| {
            let server = test_support::http::serve_paths(vec![
                (&format!("{base}/SHA256SUMS"), sums),
                (&format!("{base}/SHA256SUMS.minisig"), minisig),
            ]);
            let source = Source::with_key(&format!("{}/releases", server.url()), &public).unwrap();
            (server, source)
        };

        let (_s, source) = routes(Response::ok(SUMS), Response::ok(sig.clone()));
        let r = source.release(&v).unwrap();
        assert!(r.sha256("texoria-1.0.0-macos-aarch64.dmg").is_some());

        let (_s, source) = routes(Response::ok(SUMS), Response::ok(b"\xff\xfe".to_vec()));
        let err = source.release(&v).unwrap_err();
        assert!(
            err.to_string().contains("SHA256SUMS.minisig is not text"),
            "{err}"
        );

        // More than a list of files.
        let big = vec![b'x'; (MAX_SUMS + 1) as usize];
        let (_s, source) = routes(Response::ok(big), Response::ok(sig.clone()));
        assert!(source.release(&v).is_err());

        let (_s, source) = routes(Response::not_found(), Response::ok(sig));
        let err = source.release(&v).unwrap_err();
        assert!(format!("{err:#}").contains("SHA256SUMS"), "{err:#}");
    }

    mod props {
        use proptest::prelude::*;

        use super::*;

        proptest! {
            #[test]
            fn sums_are_lowercase_hashes(text in "\\PC{0,200}", lines in prop::collection::vec(
                ("[0-9a-fA-F]{62,66}", "[ \t*]{1,3}", "\\PC{0,12}"), 0..6)) {
                let mut all = text.clone();
                for (h, sep, name) in &lines {
                    all.push_str(&format!("\n{h}{sep}{name}"));
                }
                if let Ok(sums) = parse_sums(&all) {
                    for (name, hash) in &sums {
                        prop_assert!(!name.is_empty());
                        prop_assert_eq!(hash.len(), 64);
                        prop_assert!(hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
                    }
                }
            }

            #[test]
            fn listed_files_are_found(files in prop::collection::btree_map("[a-z0-9.-]{1,20}", "[0-9a-f]{64}", 0..8)) {
                let text: String = files.iter().map(|(n, h)| format!("{}  {n}\n", h.to_uppercase())).collect();
                let sums = parse_sums(&text).unwrap();
                prop_assert_eq!(sums.len(), files.len());
                for (n, h) in &files {
                    prop_assert_eq!(sums.get(n), Some(h));
                }
            }

            #[test]
            fn locations_never_panic(location in "\\PC{0,60}") {
                let _ = version_from_location(&location);
            }

            #[test]
            fn tags_are_read_back(major in 0u64..1000, minor in 0u64..1000, patch in 0u64..1000, v in any::<bool>()) {
                let version = Version::new(major, minor, patch);
                let prefix = if v { "v" } else { "" };
                let location = format!("https://github.com/o/r/releases/tag/{prefix}{version}");
                prop_assert_eq!(version_from_location(&location).unwrap(), version);
            }

            #[test]
            fn newer_is_antisymmetric(
                a in (0u64..5, 0u64..5, 0u64..5, prop::option::of("(rc|beta)\\.[0-9]")),
                b in (0u64..5, 0u64..5, 0u64..5, prop::option::of("(rc|beta)\\.[0-9]")),
            ) {
                let parse = |(x, y, z, pre): (u64, u64, u64, Option<String>)| {
                    let mut s = format!("{x}.{y}.{z}");
                    if let Some(p) = pre {
                        s.push('-');
                        s.push_str(&p);
                    }
                    Version::parse(&s).unwrap()
                };
                let (a, b) = (parse(a), parse(b));
                let (ab, ba) = (is_newer(&a.to_string(), &b), is_newer(&b.to_string(), &a));
                prop_assert!(!(ab && ba));
                prop_assert_eq!(ab || ba, a != b);
                prop_assert!(!is_newer(&a.to_string(), &a));
            }
        }
    }
}
