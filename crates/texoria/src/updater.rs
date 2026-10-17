//! Updates of TeXoria itself (see `tex_update`): the check, at startup and
//! from the menu, the download, and the install that the workspace finishes
//! by restarting into the new version.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::*;
use tex_update::{InstallKind, Relaunch, Source, Update, Version};

/// Between two automatic checks.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateState {
    Idle,
    Checking,
    /// A newer release exists.
    Available(Version),
    Downloading {
        version: Version,
        done: u64,
        total: Option<u64>,
    },
    /// Downloaded and verified: installed on restart.
    Ready(Version),
    Installing(Version),
}

pub enum UpdaterEvent {
    /// Something to tell the user. `version` links its release notes.
    Notify {
        error: bool,
        title: SharedString,
        message: SharedString,
        version: Option<Version>,
    },
    /// A check went through and found nothing new.
    UpToDate,
    /// The new version is in place: start it and quit.
    Installed(Relaunch),
}

impl EventEmitter<UpdaterEvent> for Updater {}

pub struct Updater {
    kind: InstallKind,
    state: UpdateState,
    update: Option<Update>,
    cancel: Arc<AtomicBool>,
    task: Task<()>,
}

impl Updater {
    pub fn new(_: &mut Context<Self>) -> Self {
        let kind = InstallKind::detect();
        log::info!("installation: {kind:?}");
        Self {
            kind,
            state: UpdateState::Idle,
            update: None,
            cancel: Arc::new(AtomicBool::new(false)),
            task: Task::ready(()),
        }
    }

    pub fn kind(&self) -> &InstallKind {
        &self.kind
    }

    pub fn state(&self) -> &UpdateState {
        &self.state
    }

    /// Whether this copy should look for releases at all (not a source build).
    pub fn enabled(&self) -> bool {
        self.kind != InstallKind::Dev
    }

    /// Whether an automatic check is due, `last` being the last one that found
    /// nothing new.
    pub fn check_due(last: Option<u64>) -> bool {
        let now = now();
        last.is_none_or(|t| now.saturating_sub(t) >= CHECK_INTERVAL.as_secs() || t > now)
    }

    /// Look for a newer release. A `manual` check tells the outcome, whatever
    /// it is; an automatic one only shows the status bar item.
    pub fn check(&mut self, manual: bool, cx: &mut Context<Self>) {
        match &self.state {
            UpdateState::Idle => {}
            UpdateState::Checking => return,
            UpdateState::Available(v)
            | UpdateState::Downloading { version: v, .. }
            | UpdateState::Ready(v)
            | UpdateState::Installing(v) => {
                if manual {
                    let v = v.clone();
                    self.notify_available(v, cx);
                }
                return;
            }
        }
        if !self.enabled() {
            if manual {
                cx.emit(UpdaterEvent::Notify {
                    error: false,
                    title: "Updates".into(),
                    message: "This copy of TeXoria was built from source: update it with git."
                        .into(),
                    version: None,
                });
            }
            return;
        }
        self.state = UpdateState::Checking;
        cx.notify();
        let latest = cx.background_spawn(async move { Source::new()?.latest_version() });
        self.task = cx.spawn(async move |this, cx| {
            let latest = latest.await;
            this.update(cx, |this, cx| {
                this.state = UpdateState::Idle;
                match latest {
                    Ok(v) if tex_update::is_newer(tex_update::CURRENT, &v) => {
                        log::info!("TeXoria {v} is available");
                        this.state = UpdateState::Available(v.clone());
                        if manual {
                            this.notify_available(v, cx);
                        }
                    }
                    Ok(_) => {
                        cx.emit(UpdaterEvent::UpToDate);
                        if manual {
                            cx.emit(UpdaterEvent::Notify {
                                error: false,
                                title: "TeXoria is up to date".into(),
                                message: format!("Version {} is the latest.", tex_update::CURRENT)
                                    .into(),
                                version: None,
                            });
                        }
                    }
                    Err(e) => {
                        log::warn!("checking for updates: {e:#}");
                        if manual {
                            cx.emit(UpdaterEvent::Notify {
                                error: true,
                                title: "Could not check for updates".into(),
                                message: format!("{e:#}").into(),
                                version: None,
                            });
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        });
    }

    /// Tell about `version`: what to do for this kind of installation.
    pub fn notify_available(&self, version: Version, cx: &mut Context<Self>) {
        let message = match self.kind.instructions() {
            Some(how) => how.to_string(),
            None => "Click “Update” in the status bar to download it; TeXoria then restarts \
                     into the new version."
                .to_string(),
        };
        cx.emit(UpdaterEvent::Notify {
            error: false,
            title: format!("TeXoria {version} is available").into(),
            message: message.into(),
            version: Some(version),
        });
    }

    /// Download the available release and check it.
    pub fn download(&mut self, cx: &mut Context<Self>) {
        let UpdateState::Available(version) = &self.state else {
            return;
        };
        let version = version.clone();
        if !self.kind.self_updating() {
            self.notify_available(version, cx);
            return;
        }
        self.state = UpdateState::Downloading {
            version: version.clone(),
            done: 0,
            total: None,
        };
        cx.notify();
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let kind = self.kind.clone();
        let (tx, rx) = smol::channel::unbounded::<(u64, Option<u64>)>();
        let v = version.clone();
        let work = cx.background_spawn(async move {
            let source = Source::new()?;
            let release = source.release(&v)?;
            let mut last = 0;
            tex_update::download(&source, &release, &kind, &cancel, |done, total| {
                // A step per MB is enough for the status bar.
                if done - last >= 1 << 20 || Some(done) == total {
                    last = done;
                    let _ = tx.try_send((done, total));
                }
            })
        });
        self.task = cx.spawn(async move |this, cx| {
            while let Ok((d, t)) = rx.recv().await {
                let progress = this.update(cx, |this, cx| {
                    if let UpdateState::Downloading { done, total, .. } = &mut this.state {
                        (*done, *total) = (d, t);
                        cx.notify();
                    }
                });
                if progress.is_err() {
                    return;
                }
            }
            let result = work.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(update) => {
                        this.state = UpdateState::Ready(update.version.clone());
                        this.update = Some(update);
                    }
                    Err(e) => {
                        this.state = UpdateState::Available(version);
                        if !this.cancel.load(Ordering::Relaxed) {
                            log::warn!("downloading the update: {e:#}");
                            cx.emit(UpdaterEvent::Notify {
                                error: true,
                                title: "Could not download the update".into(),
                                message: format!("{e:#}").into(),
                                version: None,
                            });
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        });
    }

    pub fn cancel_download(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Put the downloaded release in place; [`UpdaterEvent::Installed`] follows.
    /// The workspace saves everything first.
    pub fn install(&mut self, cx: &mut Context<Self>) {
        let Some(update) = self.update.take() else {
            return;
        };
        let version = update.version.clone();
        self.state = UpdateState::Installing(version.clone());
        cx.notify();
        let work = cx.background_spawn(async move { update.install() });
        self.task = cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(relaunch) => cx.emit(UpdaterEvent::Installed(relaunch)),
                    Err(e) => {
                        log::warn!("installing the update: {e:#}");
                        // The download is gone: start over from it.
                        this.state = UpdateState::Available(version);
                        cx.emit(UpdaterEvent::Notify {
                            error: true,
                            title: "Could not install the update".into(),
                            message: format!("{e:#}").into(),
                            version: None,
                        });
                    }
                }
                cx.notify();
            })
            .ok();
        });
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The page of a release, with its notes.
pub fn release_page(version: &Version) -> String {
    Source::new()
        .map(|s| s.page(version))
        .unwrap_or_else(|_| format!("{}/tag/v{version}", tex_update::RELEASES))
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui_kit exports its own `test` attribute.
    use super::{CHECK_INTERVAL, Updater, now};

    #[test]
    fn checks_once_a_day() {
        assert!(Updater::check_due(None));
        assert!(!Updater::check_due(Some(now() - 60)));
        assert!(Updater::check_due(Some(now() - CHECK_INTERVAL.as_secs())));
        // A clock that went back.
        assert!(Updater::check_due(Some(now() + 3600)));
    }
}
