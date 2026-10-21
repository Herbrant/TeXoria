//! Updates of TeXoria itself.

use super::*;

impl Workspace {
    /// Look for a new release now if one is due, then every hour whether one
    /// is (the app may stay open for days).
    pub(super) fn schedule_update_checks(&mut self, cx: &mut Context<Self>) {
        if !self.updater.read(cx).enabled() {
            return;
        }
        self._update_checks = cx.spawn(async move |this, cx| {
            loop {
                let due = this.update(cx, |this, cx| {
                    let s = &this.settings;
                    if s.check_updates && Updater::check_due(s.last_update_check) {
                        this.updater.update(cx, |u, cx| u.check(false, cx));
                    }
                });
                if due.is_err() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(60 * 60))
                    .await;
            }
        });
    }

    pub(super) fn on_updater_event(
        &mut self,
        _: &Entity<Updater>,
        ev: &UpdaterEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            UpdaterEvent::Notify {
                error,
                title,
                message,
                version,
            } => {
                let mut n = if *error {
                    Notification::error(message.clone())
                } else {
                    Notification::info(message.clone())
                }
                .title(title.clone());
                if let Some(v) = version {
                    let url = updater::release_page(v);
                    n = n.action(move |_, _, _| {
                        let url = url.clone();
                        Button::new("release-notes")
                            .label("Release Notes")
                            .on_click(move |_, _, cx| cx.open_url(&url))
                    });
                }
                window.push_notification(n, cx);
            }
            UpdaterEvent::UpToDate => {
                self.settings.last_update_check = Some(updater::now());
                self.settings.save();
            }
            UpdaterEvent::Installed(relaunch) => {
                // The new version opens the same project.
                let args = self.root.iter().map(|p| p.clone().into_os_string());
                match relaunch.spawn(args) {
                    Ok(()) => cx.quit(),
                    Err(e) => {
                        let (title, message) = if relaunch.installs() {
                            ("Could not install the update", format!("{e:#}"))
                        } else {
                            (
                                "Restart TeXoria",
                                format!(
                                    "The new version is installed, but could not be started: \
                                     {e:#}. Quit and open TeXoria again."
                                ),
                            )
                        };
                        self.updater
                            .update(cx, |u, cx| u.relaunch_failed(relaunch, cx));
                        window.push_notification(Notification::error(message).title(title), cx);
                    }
                }
            }
        }
    }

    /// Save everything, install the downloaded release and restart into it.
    pub(super) fn restart_to_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent_busy {
            window.push_notification(
                Notification::info("Wait for the agent to finish, or stop it, before updating."),
                cx,
            );
            return;
        }
        if !self.save_all(cx) {
            return;
        }
        self.cancel_build();
        self.updater.update(cx, |u, cx| u.install(cx));
    }

    /// The status bar item of an update, if there is one.
    pub(super) fn render_update(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let updater = self.updater.read(cx);
        let self_updating = updater.kind().self_updating();
        let (label, tooltip): (SharedString, SharedString) = match updater.state() {
            UpdateState::Idle | UpdateState::Checking => return None,
            UpdateState::Available(v) if self_updating => (
                format!("Update to {v}").into(),
                "Download the new version; TeXoria restarts into it when you are ready".into(),
            ),
            UpdateState::Available(v) => (
                format!("TeXoria {v} available").into(),
                "How to update".into(),
            ),
            UpdateState::Downloading {
                version,
                done,
                total,
            } => {
                let progress = match total {
                    Some(t) if *t > 0 => format!(" {}%", done * 100 / t),
                    _ => format!(" {} MB", done >> 20),
                };
                (
                    format!("Downloading {version}…{progress}").into(),
                    "Click to cancel".into(),
                )
            }
            UpdateState::Ready(v) => (
                "Restart to Update".into(),
                format!("Save everything and restart into TeXoria {v}").into(),
            ),
            UpdateState::Installing(v) => (format!("Installing {v}…").into(), "".into()),
            UpdateState::Elsewhere(v) => (
                "Updating in another window".into(),
                format!("Another TeXoria window is downloading or installing {v}").into(),
            ),
        };
        let icon = match updater.state() {
            UpdateState::Downloading { .. } | UpdateState::Installing(_) => Glyph::Spinner,
            _ => Glyph::Fetch,
        };
        let state = updater.state().clone();
        Some(
            Button::new("update")
                .ghost()
                .icon(icon)
                .label(label)
                .when(!tooltip.is_empty(), |b| b.tooltip(tooltip))
                .on_click(cx.listener(move |this, _, window, cx| match &state {
                    UpdateState::Available(_) => this.updater.update(cx, |u, cx| u.download(cx)),
                    UpdateState::Downloading { .. } => {
                        this.updater.update(cx, |u, cx| u.cancel_download(cx))
                    }
                    UpdateState::Ready(_) => this.restart_to_update(window, cx),
                    UpdateState::Elsewhere(v) => {
                        let v = v.clone();
                        this.updater.update(cx, |u, cx| u.notify_available(v, cx))
                    }
                    _ => {}
                }))
                .into_any_element(),
        )
    }
}
