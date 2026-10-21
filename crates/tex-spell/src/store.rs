//! Where dictionaries come from: the system's Hunspell directories first, then
//! a local cache filled on demand from wooorm/dictionaries at a pinned commit,
//! with every file checked against its pinned SHA-256.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

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
    /// Where downloads come from, and what they must hash to (tests replace
    /// them with a local server and its files).
    base: String,
    hashes: fn(&str) -> Option<(&'static str, &'static str)>,
    max_size: u64,
}

impl Store {
    pub fn new(cache_dir: &Path) -> Self {
        Self::with_dirs(
            cache_dir,
            system_dirs(
                std::env::var_os("DICPATH"),
                std::env::var_os("HOME").map(PathBuf::from),
            ),
        )
    }

    pub fn with_dirs(cache_dir: &Path, system: Vec<PathBuf>) -> Self {
        Self {
            cache: cache_dir.join(&COMMIT[..12]),
            system,
            base: BASE.to_string(),
            hashes: catalog::hashes,
            max_size: MAX_SIZE,
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
            (self.hashes)(code).ok_or_else(|| anyhow!("no dictionary for {code}"))?;
        let files = self.cached(code);
        let dir = files.aff.parent().expect("cache file has a parent");
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let agent = crate::agent(TIMEOUT);
        // Both files are verified before either is in place, so the cache never
        // holds half a dictionary.
        let mut temps = Vec::new();
        for (name, sha) in [("index.aff", aff_sha), ("index.dic", dic_sha)] {
            let url = format!("{}/{COMMIT}/dictionaries/{code}/{name}", self.base);
            log::info!("downloading {url}");
            let mut resp = agent
                .get(&url)
                .call()
                .with_context(|| format!("downloading {url}"))?;
            let reader = resp.body_mut().with_config().limit(self.max_size).reader();
            let tmp = tempfile::NamedTempFile::new_in(dir)?;
            let never = AtomicBool::new(false);
            tex_sys::download::copy_verified(reader, tmp.as_file(), sha, &never, |_| {})
                .with_context(|| format!("downloading {url}"))?;
            temps.push(tmp);
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
#[cfg_attr(not(unix), allow(unused_variables))]
fn system_dirs(dicpath: Option<std::ffi::OsString>, home: Option<PathBuf>) -> Vec<PathBuf> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut dirs: Vec<PathBuf> = dicpath
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    #[cfg(unix)]
    {
        if let Some(home) = home {
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
    use test_support::http::{Response, Server, serve, serve_paths};

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

    const AFF: &str = "SET UTF-8\nTRY esianrtolcdugmphbyfvkwz\n\nSFX S Y 1\nSFX S 0 s .\n";
    const DIC: &str = "2\nciao/S\nmondo\n";

    fn pinned(code: &str) -> Option<(&'static str, &'static str)> {
        (code == "it").then_some((
            "31878ab6a4c657d92168c40e6754abe514117ae8cbe4c828cc3ee40067aac6e3",
            "fbfd2b585cefe1e426f39cbfdb041949df618c402243a2a8d54fa43247af4ec0",
        ))
    }

    fn path(name: &str) -> String {
        format!("/{COMMIT}/dictionaries/it/{name}")
    }

    /// A store downloading from `server`, and its cache folder.
    fn store(server: &Server) -> (Store, tempfile::TempDir) {
        let cache = tempfile::tempdir().unwrap();
        let mut store = Store::with_dirs(cache.path(), Vec::new());
        store.base = server.url().to_string();
        store.hashes = pinned;
        (store, cache)
    }

    /// Everything in the cache folder: nothing is left behind by a failure.
    fn cache_files(store: &Store) -> Vec<String> {
        let mut out: Vec<String> = fs::read_dir(store.cache.join("it"))
            .into_iter()
            .flatten()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        out.sort();
        out
    }

    #[test]
    fn downloads_pinned_files_and_loads_them() {
        let server = serve_paths(vec![
            (&path("index.aff"), Response::ok(AFF)),
            (&path("index.dic"), Response::ok(DIC)),
        ]);
        let (store, _cache) = store(&server);
        assert_eq!(store.local("it"), None);
        let speller = store.load("it").unwrap();
        assert!(speller.check("ciaos") && speller.check("mondo") && !speller.check("mondi"));
        assert_eq!(store.local("it"), Some(store.cached("it")));
        assert_eq!(cache_files(&store), ["index.aff", "index.dic"]);
        let paths: Vec<_> = server.requests().into_iter().map(|r| r.path).collect();
        assert_eq!(paths, [path("index.aff"), path("index.dic")]);
        // Loaded from the cache from now on.
        store.load("it").unwrap();
        assert_eq!(server.requests().len(), 2);
    }

    #[test]
    fn failed_downloads_leave_nothing_behind() {
        let tampered = serve_paths(vec![
            (&path("index.aff"), Response::ok(AFF)),
            (&path("index.dic"), Response::ok("2\nciao/S\nmondi\n")),
        ]);
        let missing = serve_paths(vec![(&path("index.aff"), Response::ok(AFF))]);
        let huge = serve_paths(vec![
            (&path("index.aff"), Response::ok(AFF)),
            (&path("index.dic"), Response::ok(DIC)),
        ]);
        for (server, max_size, error) in [
            (&tampered, MAX_SIZE, "SHA-256 mismatch"),
            (&missing, MAX_SIZE, "404"),
            (&huge, 16, "downloading"),
        ] {
            let (mut store, _cache) = store(server);
            store.max_size = max_size;
            let e = store.download("it").unwrap_err();
            assert!(format!("{e:#}").contains(error), "{e:#}");
            assert!(cache_files(&store).is_empty(), "{:?}", cache_files(&store));
            assert_eq!(store.local("it"), None);
            assert!(store.load("it").is_err());
        }
    }

    #[test]
    fn only_catalog_languages_are_downloaded() {
        let server = serve(|_| Response::ok(""));
        let (mut store, _cache) = store(&server);
        store.hashes = catalog::hashes;
        let e = store.download("tlh-Piqad").unwrap_err();
        assert!(e.to_string().contains("no dictionary for tlh-Piqad"), "{e}");
        assert!(server.requests().is_empty());
        assert!(!store.cache.exists());
    }

    #[test]
    fn half_a_dictionary_in_place_is_not_used() {
        let server = serve_paths(vec![
            (&path("index.aff"), Response::ok(AFF)),
            (&path("index.dic"), Response::ok(DIC)),
        ]);
        let (store, _cache) = store(&server);
        // The second file cannot be put in place.
        let files = store.cached("it");
        fs::create_dir_all(files.dic.join("in-the-way")).unwrap();
        assert!(store.download("it").is_err());
        // The `.aff` is there, but alone it is not a dictionary...
        assert_eq!(store.local("it"), None);
        // ...and the next download replaces it.
        fs::remove_dir_all(&files.dic).unwrap();
        assert_eq!(store.download("it").unwrap(), files);
        assert!(store.load("it").unwrap().check("mondo"));
    }

    #[test]
    fn load_reports_broken_dictionaries() {
        let system = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let store = Store::with_dirs(cache.path(), vec![system.path().to_path_buf()]);
        let install = |aff: &[u8], dic: &[u8]| {
            fs::write(system.path().join("it_IT.aff"), aff).unwrap();
            fs::write(system.path().join("it_IT.dic"), dic).unwrap();
        };
        install(AFF.as_bytes(), DIC.as_bytes());
        assert!(store.load("it").unwrap().check("ciaos"));
        // Not UTF-8 and no charset.
        install(b"TRY abc\n", b"1\ncitt\xe0\n");
        let e = store.load("it").err().unwrap();
        assert!(e.to_string().contains("neither UTF-8"), "{e}");
        install(b"SET KOI-9\n\xff\n", b"1\nx\n");
        let e = store.load("it").err().unwrap();
        assert!(
            e.to_string()
                .contains("unsupported dictionary charset KOI-9"),
            "{e}"
        );
        // Parsed but refused.
        install(AFF.as_bytes(), b"");
        let e = store.load("it").err().unwrap();
        assert!(e.to_string().contains("parsing the it dictionary"), "{e}");
        // A BOM and a legacy charset are fine.
        install(b"SET ISO8859-1\n", b"1\ncitt\xe0\n");
        assert!(store.load("it").unwrap().check("città"));
        install(b"\xEF\xBB\xBFSET UTF-8\n", "\u{feff}1\ncittà\n".as_bytes());
        assert!(store.load("it").unwrap().check("città"));
    }

    #[test]
    fn system_dirs_start_with_dicpath() {
        let dicpath = std::env::join_paths(["/a", "/b"]).unwrap();
        let dirs = system_dirs(Some(dicpath), Some(PathBuf::from("/home/me")));
        assert_eq!(dirs[..2], [PathBuf::from("/a"), PathBuf::from("/b")]);
        if cfg!(unix) {
            assert_eq!(dirs[2], Path::new("/home/me/.local/share/hunspell"));
            assert!(dirs.contains(&PathBuf::from("/usr/share/hunspell")));
            let homeless = system_dirs(None, None);
            assert!(homeless.iter().all(|d| !d.starts_with("/home/me")));
            assert_eq!(
                homeless[0],
                Path::new(if cfg!(target_os = "macos") {
                    "/Library/Spelling"
                } else {
                    "/usr/share/hunspell"
                })
            );
        } else {
            assert_eq!(dirs.len(), 2);
            assert!(system_dirs(None, None).is_empty());
        }
        assert!(dirs.contains(&PathBuf::from("/a")));
    }

    #[test]
    fn new_reads_the_environment() {
        let cache = tempfile::tempdir().unwrap();
        let store = Store::new(cache.path());
        assert_eq!(store.cache, cache.path().join(&COMMIT[..12]));
        assert_eq!(store.base, BASE);
        assert_eq!(store.max_size, MAX_SIZE);
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
