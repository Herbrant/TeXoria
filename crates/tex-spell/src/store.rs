//! Where dictionaries come from: the system's Hunspell directories first, then
//! a local cache filled on demand from wooorm/dictionaries at a pinned commit,
//! with every file checked against its pinned SHA-256.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};

use crate::catalog;
use crate::checker::Speller;

/// wooorm/dictionaries commit the hashes in `data/dictionaries.tsv` belong to.
/// Bump both with `scripts/gen-dictionaries.sh`.
pub const COMMIT: &str = "8cfea406b505e4d7df52d5a19bce525df98c54ab";
const BASE: &str = "https://raw.githubusercontent.com/wooorm/dictionaries";
/// The largest file (rw/index.aff) is ~38 MB.
const MAX_SIZE: u64 = 128 << 20;
const TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Files {
    pub aff: PathBuf,
    pub dic: PathBuf,
}

pub struct Store {
    /// Downloads, one directory per language, under a directory per commit.
    cache: PathBuf,
    system: Vec<PathBuf>,
}

impl Store {
    pub fn new(cache_dir: &Path) -> Self {
        Self::with_dirs(cache_dir, system_dirs())
    }

    pub fn with_dirs(cache_dir: &Path, system: Vec<PathBuf>) -> Self {
        Self {
            cache: cache_dir.join(&COMMIT[..12]),
            system,
        }
    }

    /// The dictionary for `code` if it is already on this machine.
    pub fn local(&self, code: &str) -> Option<Files> {
        // Joined into paths: `../x` must not reach outside the folders.
        if code.is_empty() || !code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return None;
        }
        let installed = system_names(code).into_iter().find_map(|name| {
            self.system.iter().find_map(|dir| {
                let files = Files {
                    aff: dir.join(format!("{name}.aff")),
                    dic: dir.join(format!("{name}.dic")),
                };
                (files.aff.is_file() && files.dic.is_file()).then_some(files)
            })
        });
        installed.or_else(|| {
            let files = self.cached(code);
            (files.aff.is_file() && files.dic.is_file()).then_some(files)
        })
    }

    fn cached(&self, code: &str) -> Files {
        let dir = self.cache.join(code);
        Files {
            aff: dir.join("index.aff"),
            dic: dir.join("index.dic"),
        }
    }

    /// Download the dictionary for `code` into the cache. Blocking.
    pub fn download(&self, code: &str) -> Result<Files> {
        let (aff_sha, dic_sha) =
            catalog::hashes(code).ok_or_else(|| anyhow!("no dictionary for {code}"))?;
        let files = self.cached(code);
        let dir = files.aff.parent().expect("cache file has a parent");
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .user_agent(concat!("TeXoria/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        // Both files are verified before either is in place, so the cache never
        // holds half a dictionary.
        let mut temps = Vec::new();
        for (name, sha) in [("index.aff", aff_sha), ("index.dic", dic_sha)] {
            let url = format!("{BASE}/{COMMIT}/dictionaries/{code}/{name}");
            log::info!("downloading {url}");
            let mut resp = agent
                .get(&url)
                .call()
                .with_context(|| format!("downloading {url}"))?;
            let reader = resp.body_mut().with_config().limit(MAX_SIZE).reader();
            let tmp = tempfile::NamedTempFile::new_in(dir)?;
            temps.push(
                copy_verified(reader, tmp, sha).with_context(|| format!("downloading {url}"))?,
            );
        }
        let mut temps = temps.into_iter();
        for dest in [&files.aff, &files.dic] {
            let tmp = temps.next().expect("one temp per file");
            tmp.persist(dest).map_err(|e| e.error)?;
        }
        Ok(files)
    }

    /// Load the dictionary for `code`, downloading it if needed. Blocking (a
    /// large dictionary takes a while to parse).
    pub fn load(&self, code: &str) -> Result<Speller> {
        let files = match self.local(code) {
            Some(files) => files,
            None => self.download(code)?,
        };
        let aff =
            fs::read(&files.aff).with_context(|| format!("reading {}", files.aff.display()))?;
        let dic =
            fs::read(&files.dic).with_context(|| format!("reading {}", files.dic.display()))?;
        let aff_text = decode(&aff, &aff)?;
        let dic_text = decode(&dic, &aff)?;
        Speller::new(&aff_text, &dic_text)
            .with_context(|| format!("parsing the {code} dictionary ({})", files.aff.display()))
    }
}

/// Copy `reader` into `tmp`, failing unless its SHA-256 is `expected`.
fn copy_verified(
    mut reader: impl Read,
    mut tmp: tempfile::NamedTempFile,
    expected: &str,
) -> Result<tempfile::NamedTempFile> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 << 10];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        tmp.write_all(&buf[..n])?;
    }
    tmp.as_file().sync_all()?;
    let actual: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual != expected {
        bail!("SHA-256 mismatch: expected {expected}, got {actual}");
    }
    Ok(tmp)
}

/// Hunspell files are UTF-8 or in the charset named by the `.aff`'s `SET`.
fn decode(bytes: &[u8], aff: &[u8]) -> Result<String> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    if let Ok(s) = std::str::from_utf8(bytes) {
        return Ok(s.to_string());
    }
    let label = aff
        .split(|&b| b == b'\n')
        .find_map(|l| l.strip_prefix(b"SET "))
        .map(|l| String::from_utf8_lossy(l).trim().to_string())
        .ok_or_else(|| anyhow!("dictionary is neither UTF-8 nor declares its charset"))?;
    let encoding = encoding_rs::Encoding::for_label(label.as_bytes())
        .or_else(|| encoding_rs::Encoding::for_label(label.replace("ISO", "ISO-").as_bytes()))
        .ok_or_else(|| anyhow!("unsupported dictionary charset {label}"))?;
    Ok(encoding.decode(bytes).0.into_owned())
}

/// Hunspell's usual locations (`DICPATH` first).
fn system_dirs() -> Vec<PathBuf> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut dirs: Vec<PathBuf> = std::env::var_os("DICPATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    #[cfg(unix)]
    {
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            dirs.push(home.join(".local/share/hunspell"));
            #[cfg(target_os = "macos")]
            dirs.push(home.join("Library/Spelling"));
        }
        #[cfg(target_os = "macos")]
        dirs.push("/Library/Spelling".into());
        for d in [
            "/usr/share/hunspell",
            "/usr/local/share/hunspell",
            "/usr/share/myspell",
            "/usr/share/myspell/dicts",
            "/app/share/hunspell",
        ] {
            dirs.push(d.into());
        }
    }
    dirs
}

/// File names a system dictionary for `code` is installed under.
fn system_names(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    match code.split_once('-') {
        Some(("sr", "Latn")) => out.push("sr_Latn_RS".to_string()),
        Some((_, variant)) if variant.len() == 2 => out.push(code.replace('-', "_")),
        Some(_) => {}
        None => {
            let region = match code {
                "en" => "US",
                "pt" => "BR",
                "da" => "DK",
                "sv" => "SE",
                "nb" | "nn" => "NO",
                "cs" => "CZ",
                "el" => "GR",
                "he" => "IL",
                "uk" => "UA",
                "ko" => "KR",
                "vi" => "VN",
                "ca" | "eu" | "gl" => "ES",
                "sl" => "SI",
                "et" => "EE",
                "ka" => "GE",
                "hy" => "AM",
                "fa" => "IR",
                "ne" => "NP",
                "sr" => "RS",
                "ga" => "IE",
                "gd" | "cy" => "GB",
                "br" | "oc" => "FR",
                "fy" => "NL",
                "lb" => "LU",
                "fo" => "FO",
                "is" => "IS",
                "mn" => "MN",
                "rw" => "RW",
                "tk" => "TM",
                _ => "",
            };
            let region = if region.is_empty() {
                code.to_ascii_uppercase()
            } else {
                region.to_string()
            };
            out.push(format!("{code}_{region}"));
            out.push(code.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_downloads() {
        let dir = tempfile::tempdir().unwrap();
        // SHA-256 of "abc".
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let tmp = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        let ok = copy_verified(&b"abc"[..], tmp, abc).unwrap();
        assert_eq!(fs::read(ok.path()).unwrap(), b"abc");
        let tmp = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        assert!(copy_verified(&b"abd"[..], tmp, abc).is_err());
    }

    #[test]
    fn decodes_legacy_charsets() {
        let aff = b"SET ISO8859-15\nTRY abc\n";
        assert_eq!(decode(b"citt\xe0\n", aff).unwrap(), "città\n");
        assert_eq!(decode("città".as_bytes(), aff).unwrap(), "città");
        assert!(decode(b"\xff", b"TRY abc\n").is_err());
    }

    #[test]
    fn finds_system_and_cached_dictionaries() {
        let cache = tempfile::tempdir().unwrap();
        let system = tempfile::tempdir().unwrap();
        let store = Store::with_dirs(cache.path(), vec![system.path().to_path_buf()]);
        assert_eq!(store.local("it"), None);

        fs::write(system.path().join("it_IT.aff"), "").unwrap();
        fs::write(system.path().join("it_IT.dic"), "").unwrap();
        assert_eq!(
            store.local("it").unwrap().aff,
            system.path().join("it_IT.aff")
        );

        let cached = store.cached("de-AT");
        fs::create_dir_all(cached.aff.parent().unwrap()).unwrap();
        fs::write(&cached.aff, "").unwrap();
        assert_eq!(
            store.local("de-AT"),
            None,
            "half a dictionary is not a dictionary"
        );
        fs::write(&cached.dic, "").unwrap();
        assert_eq!(store.local("de-AT"), Some(cached));
    }

    #[test]
    fn codes_stay_inside_the_dictionary_folders() {
        let root = tempfile::tempdir().unwrap();
        let system = root.path().join("system");
        let cache = root.path().join("cache");
        fs::create_dir_all(&system).unwrap();
        let store = Store::with_dirs(&cache, vec![system.clone()]);
        // A dictionary outside both folders...
        fs::write(root.path().join("evil.aff"), "").unwrap();
        fs::write(root.path().join("evil.dic"), "").unwrap();
        let outside = store.cached("../../x");
        fs::create_dir_all(outside.aff.parent().unwrap()).unwrap();
        fs::write(&outside.aff, "").unwrap();
        fs::write(&outside.dic, "").unwrap();
        // ...is not reachable through a code.
        for code in [
            "../evil",
            "../../x",
            "/etc/evil",
            "x/../../../evil",
            "",
            "it\0",
        ] {
            assert_eq!(store.local(code), None, "{code:?}");
        }
        assert!(store.download("../../x").is_err());
    }

    #[test]
    fn names_system_files() {
        assert_eq!(system_names("it"), vec!["it_IT", "it"]);
        assert_eq!(system_names("en"), vec!["en_US", "en"]);
        assert_eq!(system_names("en-GB"), vec!["en_GB"]);
        assert_eq!(system_names("sr-Latn"), vec!["sr_Latn_RS"]);
        assert!(system_names("ca-valencia").is_empty());
    }

    /// Downloads a real dictionary: `cargo test -p tex-spell -- --include-ignored`.
    #[test]
    #[ignore = "needs the network"]
    fn downloads_and_loads_a_dictionary() {
        let cache = tempfile::tempdir().unwrap();
        let store = Store::with_dirs(cache.path(), Vec::new());
        let speller = store.load("it").unwrap();
        assert!(speller.check("perché"));
        assert!(!speller.check("perchè"));
        assert!(store.local("it").is_some());
    }
}
