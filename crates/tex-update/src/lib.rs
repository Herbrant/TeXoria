//! Updates for copies of TeXoria that no package manager looks after: the
//! `.dmg`, the Windows installer, the AppImage and `install.sh`'s tarball.
//!
//! [`Source::latest_version`] finds the newest release, [`Source::release`]
//! fetches its `SHA256SUMS` and checks the release workflow's minisign
//! signature, [`download`] gets the package matching the [`InstallKind`] and
//! checks its hash, and [`Update::install`] puts it in place. [`Lock`] keeps
//! two running copies from updating at the same time.

mod apply;
mod download;
mod install;
mod lock;
mod release;

use std::time::Duration;

pub use apply::Relaunch;
pub use download::{Update, download};
pub use install::{InstallKind, Manager, Probe};
pub use lock::Lock;
pub use release::{RELEASES, Release, Source, URL_VAR, is_newer};
pub use semver::Version;

/// The running version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

pub(crate) const USER_AGENT: &str = concat!("TeXoria/", env!("CARGO_PKG_VERSION"));

pub(crate) fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .user_agent(USER_AGENT)
        .build()
        .into()
}
