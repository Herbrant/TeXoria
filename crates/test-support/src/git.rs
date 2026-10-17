//! git with the user's own configuration out of the way.

use std::path::{Path, PathBuf};

/// Environment variables that make git ignore the user's and the system's configuration: a
/// global config in `home` with an identity and `main` as the default branch.
pub fn isolated_env(home: &Path) -> Vec<(&'static str, PathBuf)> {
    let config = home.join("gitconfig");
    std::fs::write(
        &config,
        "[user]\n\tname = Test\n\temail = test@example.com\n[init]\n\tdefaultBranch = main\n",
    )
    .unwrap();
    vec![
        ("GIT_CONFIG_GLOBAL", config),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ("HOME", home.to_path_buf()),
    ]
}
