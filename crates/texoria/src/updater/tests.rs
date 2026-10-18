use std::cell::RefCell;

use std::rc::Rc;

use gpui_kit::TestAppContext;
use test_support::http::{Response, Server, serve_paths};
use tex_update::Manager;

use super::*;
// `super::*` brings gpui's `test` attribute, which would shadow the standard one, and
// gpui-kit's `gpui` alias, which the `#[gpui::test]` macro's paths must not find twice.
use core::prelude::v1::test;
use gpui_kit::gpui;

#[test]
fn checks_once_a_day() {
    assert!(Updater::check_due(None));
    assert!(!Updater::check_due(Some(now() - 60)));
    assert!(Updater::check_due(Some(now() - CHECK_INTERVAL.as_secs())));
    // A clock that went back.
    assert!(Updater::check_due(Some(now() + 3600)));
}

#[test]
fn release_pages() {
    let page = release_page(&Version::new(1, 2, 3));
    assert!(page.ends_with("/tag/v1.2.3"), "{page}");
}

/// `tests/fixtures/update`: a release 99.0.0 signed with a test key (the secret half was
/// thrown away), whose packages are a Linux tarball and a stand-in Windows installer.
fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(test_support::fixtures().join("update").join(name)).unwrap()
}

const V: &str = "99.0.0";

fn version() -> Version {
    Version::parse(V).unwrap()
}

fn tarball() -> String {
    format!("texoria-{V}-linux-{}.tar.gz", std::env::consts::ARCH)
}

fn installer() -> String {
    format!("texoria-{V}-windows-{}-setup.exe", std::env::consts::ARCH)
}

/// A release server; `sums` replaces the signed `SHA256SUMS`, `delay` slows the packages.
fn server(latest: &str, sums: Option<&str>, delay: Duration) -> Server {
    let sums = sums.map_or_else(|| fixture("SHA256SUMS"), |s| s.as_bytes().to_vec());
    let at = |name: &str| format!("/download/v{V}/{name}");
    serve_paths(vec![
        (
            "/latest",
            Response::redirect(&format!("https://example.invalid/releases/tag/v{latest}")),
        ),
        (&at("SHA256SUMS"), Response::ok(sums)),
        (
            &at("SHA256SUMS.minisig"),
            Response::ok(fixture("SHA256SUMS.minisig")),
        ),
        (
            &at(&tarball()),
            Response::ok(fixture(&format!("texoria-{V}-linux.tar.gz"))).delay(delay),
        ),
        (
            &at(&installer()),
            Response::ok(fixture(&format!("texoria-{V}-windows-setup.exe"))),
        ),
    ])
}

fn source_of(server: &Server) -> SourceFn {
    let url = server.url().to_string();
    let key = String::from_utf8(fixture("release-key.pub")).unwrap();
    Arc::new(move || Source::with_key(&url, &key))
}

/// An installation by `install.sh` in a temporary prefix.
fn tarball_install() -> (tempfile::TempDir, InstallKind) {
    let prefix = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(prefix.path().join("bin")).unwrap();
    std::fs::create_dir_all(prefix.path().join("lib/texoria")).unwrap();
    std::fs::write(prefix.path().join("bin/texoria"), "old").unwrap();
    std::fs::write(prefix.path().join("lib/texoria/libpdfium.so"), "old").unwrap();
    let kind = InstallKind::Tarball {
        prefix: prefix.path().to_path_buf(),
    };
    (prefix, kind)
}

/// What the updater said, and the states it went through.
#[derive(Default)]
struct Seen {
    events: Vec<String>,
    states: Vec<UpdateState>,
    relaunch: Option<Relaunch>,
}

struct Fixture {
    updater: Entity<Updater>,
    seen: Rc<RefCell<Seen>>,
    lock: PathBuf,
    _lock_dir: tempfile::TempDir,
}

impl Fixture {
    fn new(cx: &mut TestAppContext, kind: InstallKind, source: SourceFn) -> Self {
        let lock_dir = tempfile::tempdir().unwrap();
        let lock = lock_dir.path().join("update.lock");
        let path = lock.clone();
        let updater = cx.new(|_| Updater::with(kind, source, Some(path)));
        let seen: Rc<RefCell<Seen>> = Rc::default();
        let (s1, s2) = (seen.clone(), seen.clone());
        cx.update(|cx| {
            cx.subscribe(&updater, move |_, ev: &UpdaterEvent, _| {
                let mut seen = s1.borrow_mut();
                let line = match ev {
                    UpdaterEvent::Notify {
                        error,
                        title,
                        message,
                        version,
                    } => format!(
                        "{}{title}: {message}{}",
                        if *error { "error: " } else { "" },
                        version
                            .as_ref()
                            .map_or(String::new(), |v| format!(" [{v}]"))
                    ),
                    UpdaterEvent::UpToDate => "up to date".into(),
                    UpdaterEvent::Installed(relaunch) => {
                        seen.relaunch = Some(relaunch.clone());
                        "installed".into()
                    }
                };
                seen.events.push(line);
            })
            .detach();
            cx.observe(&updater, move |u, cx| {
                let state = u.read(cx).state().clone();
                let mut seen = s2.borrow_mut();
                if seen.states.last() != Some(&state) {
                    seen.states.push(state);
                }
            })
            .detach();
        });
        Fixture {
            updater,
            seen,
            lock,
            _lock_dir: lock_dir,
        }
    }

    fn update<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut Updater, &mut Context<Updater>) -> R,
    ) -> R {
        let r = self.updater.update(cx, f);
        cx.run_until_parked();
        r
    }

    fn state(&self, cx: &mut TestAppContext) -> UpdateState {
        self.updater.read_with(cx, |u, _| u.state().clone())
    }

    fn events(&self) -> Vec<String> {
        std::mem::take(&mut self.seen.borrow_mut().events)
    }
}

/// Whether the lock at `path` is free, or soon is: a process another test is starting holds
/// a copy of every open file until it runs its program (a fork shares the lock).
fn released(path: &std::path::Path) -> bool {
    let start = std::time::Instant::now();
    while Lock::busy_at(path) {
        if start.elapsed() > Duration::from_secs(5) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

fn available() -> UpdateState {
    UpdateState::Available(version())
}

#[gpui::test]
fn a_source_build_never_checks(cx: &mut TestAppContext) {
    let f = Fixture::new(
        cx,
        InstallKind::Dev,
        Arc::new(|| panic!("no release source")),
    );
    f.updater.read_with(cx, |u, _| {
        assert!(!u.enabled());
        assert_eq!(u.kind(), &InstallKind::Dev);
    });
    f.update(cx, |u, cx| u.check(false, cx));
    assert!(f.events().is_empty());
    f.update(cx, |u, cx| u.check(true, cx));
    assert_eq!(
        f.events(),
        ["Updates: This copy of TeXoria was built from source: update it with git."]
    );
    assert_eq!(f.state(cx), UpdateState::Idle);
}

#[gpui::test]
fn finds_a_newer_release(cx: &mut TestAppContext) {
    let server = server(V, None, Duration::ZERO);
    let (_prefix, kind) = tarball_install();
    let f = Fixture::new(cx, kind, source_of(&server));
    // An automatic check only changes the status bar.
    f.update(cx, |u, cx| u.check(false, cx));
    assert_eq!(f.state(cx), available());
    assert!(f.events().is_empty());
    assert_eq!(f.seen.borrow().states[0], UpdateState::Checking);
    // Asked again: told what to do, without asking the server.
    f.update(cx, |u, cx| u.check(true, cx));
    let events = f.events();
    assert_eq!(events.len(), 1);
    assert!(
        events[0].starts_with(&format!("TeXoria {V} is available: Click “Update”")),
        "{events:?}"
    );
    assert!(events[0].ends_with(&format!("[{V}]")));
    assert_eq!(server.requests().len(), 1);

    // A manual check from scratch tells it too.
    let f = Fixture::new(cx, tarball_install().1, source_of(&server));
    f.update(cx, |u, cx| {
        u.check(true, cx);
        // Already checking: one request only.
        u.check(true, cx);
    });
    assert_eq!(f.events().len(), 1);
    assert_eq!(server.requests().len(), 2);
}

#[gpui::test]
fn tells_when_up_to_date_or_unreachable(cx: &mut TestAppContext) {
    let server = server(tex_update::CURRENT, None, Duration::ZERO);
    let f = Fixture::new(cx, tarball_install().1, source_of(&server));
    f.update(cx, |u, cx| u.check(false, cx));
    assert_eq!(f.events(), ["up to date"]);
    f.update(cx, |u, cx| u.check(true, cx));
    assert_eq!(
        f.events(),
        [
            "up to date".to_string(),
            format!(
                "TeXoria is up to date: Version {} is the latest.",
                tex_update::CURRENT
            )
        ]
    );
    assert_eq!(f.state(cx), UpdateState::Idle);

    // No release address: only a manual check says so.
    let failing: SourceFn = Arc::new(|| Err(anyhow::anyhow!("no network here")));
    let f = Fixture::new(cx, tarball_install().1, failing);
    f.update(cx, |u, cx| u.check(false, cx));
    assert!(f.events().is_empty());
    f.update(cx, |u, cx| u.check(true, cx));
    assert_eq!(
        f.events(),
        ["error: Could not check for updates: no network here"]
    );
    assert_eq!(f.state(cx), UpdateState::Idle);
}

#[gpui::test]
fn package_managers_update_their_own(cx: &mut TestAppContext) {
    let server = server(V, None, Duration::ZERO);
    let kind = InstallKind::Managed(Manager::Homebrew);
    let f = Fixture::new(cx, kind, source_of(&server));
    f.update(cx, |u, cx| u.check(true, cx));
    assert_eq!(
        f.events(),
        [format!(
            "TeXoria {V} is available: Update it with: brew upgrade --cask texoria [{V}]"
        )]
    );
    // "Update" repeats it, and downloads nothing.
    f.update(cx, |u, cx| u.download(cx));
    assert_eq!(f.events().len(), 1);
    assert_eq!(f.state(cx), available());
    assert_eq!(server.requests().len(), 1);
}

#[gpui::test]
fn downloads_and_installs(cx: &mut TestAppContext) {
    let server = server(V, None, Duration::ZERO);
    let (prefix, kind) = tarball_install();
    let f = Fixture::new(cx, kind, source_of(&server));
    // Nothing to download or install yet.
    f.update(cx, |u, cx| {
        u.download(cx);
        u.install(cx);
        u.cancel_download(cx);
    });
    assert_eq!(f.state(cx), UpdateState::Idle);

    f.update(cx, |u, cx| u.check(false, cx));
    f.update(cx, |u, cx| u.download(cx));
    assert_eq!(f.state(cx), UpdateState::Ready(version()));
    assert!(f.events().is_empty(), "{:?}", f.events());
    let size = fixture(&format!("texoria-{V}-linux.tar.gz")).len() as u64;
    assert!(
        f.seen.borrow().states.contains(&UpdateState::Downloading {
            version: version(),
            done: size,
            total: Some(size),
        }),
        "{:?}",
        f.seen.borrow().states
    );
    // The other copies leave it alone until it is installed.
    assert!(Lock::busy_at(&f.lock));

    f.update(cx, |u, cx| u.install(cx));
    assert_eq!(f.events(), ["installed"]);
    assert_eq!(f.state(cx), UpdateState::Installing(version()));
    let script = std::fs::read_to_string(prefix.path().join("bin/texoria")).unwrap();
    assert!(script.contains("texoria 99.0.0"), "{script}");
    // Its relaunch would not start: the new version is in place for the next one.
    let relaunch = f.seen.borrow_mut().relaunch.take().unwrap();
    assert!(!relaunch.installs());
    f.update(cx, |u, cx| u.relaunch_failed(&relaunch, cx));
    assert_eq!(f.state(cx), UpdateState::Idle);
    assert!(released(&f.lock));
    // Only once installing.
    f.update(cx, |u, cx| u.relaunch_failed(&relaunch, cx));
    assert_eq!(f.state(cx), UpdateState::Idle);
}

#[gpui::test]
fn a_failed_install_starts_over(cx: &mut TestAppContext) {
    let server = server(V, None, Duration::ZERO);
    let (prefix, kind) = tarball_install();
    let f = Fixture::new(cx, kind, source_of(&server));
    f.update(cx, |u, cx| u.check(false, cx));
    f.update(cx, |u, cx| u.download(cx));
    assert_eq!(f.state(cx), UpdateState::Ready(version()));
    // The program can no longer be replaced.
    std::fs::remove_dir_all(prefix.path().join("bin")).unwrap();
    f.update(cx, |u, cx| u.install(cx));
    let events = f.events();
    assert_eq!(events.len(), 1);
    assert!(
        events[0].starts_with("error: Could not install the update: "),
        "{events:?}"
    );
    assert_eq!(f.state(cx), available());
    assert!(released(&f.lock));
}

#[gpui::test]
fn refuses_a_release_that_is_not_signed(cx: &mut TestAppContext) {
    let forged = format!("{}  {}\n", "0".repeat(64), tarball());
    let server = server(V, Some(&forged), Duration::ZERO);
    let (prefix, kind) = tarball_install();
    let f = Fixture::new(cx, kind, source_of(&server));
    f.update(cx, |u, cx| u.check(false, cx));
    f.update(cx, |u, cx| u.download(cx));
    let events = f.events();
    assert_eq!(events.len(), 1);
    assert!(
        events[0].starts_with("error: Could not download the update: "),
        "{events:?}"
    );
    assert!(events[0].contains("not signed"), "{events:?}");
    assert_eq!(f.state(cx), available());
    assert!(released(&f.lock));
    // Nothing was downloaded, and the old version is untouched.
    assert!(
        !server
            .requests()
            .iter()
            .any(|r| r.path.ends_with(".tar.gz"))
    );
    assert_eq!(
        std::fs::read_to_string(prefix.path().join("bin/texoria")).unwrap(),
        "old"
    );
}

#[gpui::test]
fn a_cancelled_download_says_nothing(cx: &mut TestAppContext) {
    let server = server(V, None, Duration::from_millis(200));
    let (prefix, kind) = tarball_install();
    let f = Fixture::new(cx, kind, source_of(&server));
    f.update(cx, |u, cx| u.check(false, cx));
    f.updater.update(cx, |u, cx| {
        u.download(cx);
        assert!(matches!(u.state(), UpdateState::Downloading { .. }));
        u.cancel_download(cx);
        assert_eq!(u.state(), &available());
    });
    cx.run_until_parked();
    assert_eq!(f.state(cx), available());
    assert!(f.events().is_empty());
    assert!(released(&f.lock));
    // No download left behind.
    let left: Vec<_> = std::fs::read_dir(prefix.path())
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".texoria-update-")
        })
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[gpui::test]
fn waits_for_the_copy_that_is_updating(cx: &mut TestAppContext) {
    let server = server(V, None, Duration::ZERO);
    let (_prefix, kind) = tarball_install();
    let f = Fixture::new(cx, kind, source_of(&server));
    // Another copy holds the lock when the release is found.
    let held = Lock::try_acquire_at(&f.lock).unwrap().unwrap();
    f.update(cx, |u, cx| u.check(false, cx));
    assert_eq!(f.state(cx), UpdateState::Elsewhere(version()));
    f.update(cx, |u, cx| u.check(true, cx));
    let events = f.events();
    assert!(events[0].contains("Another TeXoria window"), "{events:?}");
    // Still busy at the next look.
    cx.executor().advance_clock(WATCH_INTERVAL);
    cx.run_until_parked();
    assert_eq!(f.state(cx), UpdateState::Elsewhere(version()));
    // Done there: available here at the next look.
    drop(held);
    assert!(released(&f.lock));
    cx.executor().advance_clock(WATCH_INTERVAL);
    cx.run_until_parked();
    assert_eq!(f.state(cx), available());

    // The other copy starts while this one was about to download.
    let held = Lock::try_acquire_at(&f.lock).unwrap().unwrap();
    f.update(cx, |u, cx| u.download(cx));
    assert_eq!(f.state(cx), UpdateState::Elsewhere(version()));
    let events = f.events();
    assert!(events[0].contains("Another TeXoria window"), "{events:?}");
    assert!(
        !server
            .requests()
            .iter()
            .any(|r| r.path.contains("/download/"))
    );
    drop(held);
    assert!(released(&f.lock));
    cx.executor().advance_clock(WATCH_INTERVAL);
    cx.run_until_parked();
    assert_eq!(f.state(cx), available());
}

/// The kept download folders of the stand-in Windows installer, in the temporary folder.
fn remove_kept_installers() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for e in entries.flatten() {
        let dir = e.path();
        if e.file_name()
            .to_string_lossy()
            .starts_with(".texoria-update-")
            && dir.join(installer()).is_file()
        {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

#[gpui::test]
fn the_windows_installer_installs_when_it_runs(cx: &mut TestAppContext) {
    let server = server(V, None, Duration::ZERO);
    let dir = tempfile::tempdir().unwrap();
    let kind = InstallKind::Windows {
        dir: dir.path().to_path_buf(),
    };
    let f = Fixture::new(cx, kind, source_of(&server));
    f.update(cx, |u, cx| u.check(false, cx));
    f.update(cx, |u, cx| u.download(cx));
    assert_eq!(f.state(cx), UpdateState::Ready(version()));
    f.update(cx, |u, cx| u.install(cx));
    assert_eq!(f.events(), ["installed"]);
    let relaunch = f.seen.borrow_mut().relaunch.take().unwrap();
    remove_kept_installers();
    // The installer could not start: nothing is installed, start over.
    assert!(relaunch.installs());
    f.update(cx, |u, cx| u.relaunch_failed(&relaunch, cx));
    assert_eq!(f.state(cx), available());
    assert!(released(&f.lock));
}
