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
        let base = std::env::var(URL_VAR).unwrap_or_else(|_| RELEASES.to_string());
        #[cfg(debug_assertions)]
        if let Ok(key) = std::env::var(KEY_VAR) {
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
}
