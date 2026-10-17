//! The project as it was before the agent's turn: what it changed since,
//! whatever way it wrote the files (through TeXoria or on its own), and what
//! to put back to undo it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tex_project::files::{self, FileNode};

/// Larger files are only compared by size and date, and cannot be restored.
const MAX_FILE: u64 = 8 << 20;
/// Past this much, the remaining files are only compared by size and date.
const MAX_TOTAL: u64 = 200 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Snapshot {
    Bytes(Vec<u8>),
    Stamp(u64, Option<SystemTime>),
}

#[derive(Debug, Clone)]
pub struct Checkpoint {
    dir: PathBuf,
    /// By path relative to `dir`.
    files: BTreeMap<PathBuf, Snapshot>,
    /// Git's configuration and hooks (absolute paths), which run programs.
    /// `.git` is not part of the review, so changes there are told apart.
    git: BTreeMap<PathBuf, Option<Vec<u8>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Relative to the checkpoint's directory.
    pub path: PathBuf,
    pub kind: ChangeKind,
    /// The contents before, if kept (`None` for an added file, or one too
    /// large to keep).
    pub before: Option<Vec<u8>>,
    /// The contents now (`None` for a deleted file).
    pub after: Option<Vec<u8>>,
}

impl Change {
    /// Whether undoing it is possible.
    pub fn restorable(&self) -> bool {
        self.kind == ChangeKind::Added || self.before.is_some()
    }
}

impl Checkpoint {
    /// Record the files of `dir` (not `.git` nor `.texoria`). Blocking.
    pub fn take(dir: &Path) -> Checkpoint {
        let mut files = BTreeMap::new();
        let mut total = 0u64;
        for path in walk(dir) {
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let rel = path.strip_prefix(dir).unwrap_or(&path).to_path_buf();
            let len = meta.len();
            let snapshot = if len <= MAX_FILE && total + len <= MAX_TOTAL {
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        total += len;
                        Snapshot::Bytes(bytes)
                    }
                    Err(_) => continue,
                }
            } else {
                Snapshot::Stamp(len, meta.modified().ok())
            };
            files.insert(rel, snapshot);
        }
        let git = git_files(dir)
            .into_iter()
            .map(|p| {
                let bytes = std::fs::read(&p).ok();
                (p, bytes)
            })
            .collect();
        Checkpoint {
            dir: dir.to_path_buf(),
            files,
            git,
        }
    }

    /// Git configuration files and hooks that differ now (created, changed
    /// or deleted): they can run programs on the next Git command. Blocking.
    pub fn git_changes(&self) -> Vec<PathBuf> {
        let mut paths: BTreeSet<PathBuf> = git_files(&self.dir).into_iter().collect();
        paths.extend(self.git.keys().cloned());
        paths
            .into_iter()
            .filter(|p| self.git.get(p).cloned().flatten() != std::fs::read(p).ok())
            .collect()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// What differs now, by path. Blocking.
    pub fn changes(&self) -> Vec<Change> {
        let now: BTreeSet<PathBuf> = walk(&self.dir)
            .into_iter()
            .map(|p| p.strip_prefix(&self.dir).unwrap_or(&p).to_path_buf())
            .collect();
        let mut out = Vec::new();
        for (rel, before) in &self.files {
            let abs = self.dir.join(rel);
            if !now.contains(rel) {
                out.push(Change {
                    path: rel.clone(),
                    kind: ChangeKind::Deleted,
                    before: bytes(before),
                    after: None,
                });
                continue;
            }
            let changed = match before {
                Snapshot::Bytes(old) => match std::fs::read(&abs) {
                    Ok(new) if new != *old => Some(Some(new)),
                    _ => None,
                },
                Snapshot::Stamp(len, modified) => std::fs::metadata(&abs)
                    .ok()
                    .filter(|m| m.len() != *len || m.modified().ok() != *modified)
                    .map(|m| {
                        if m.len() <= MAX_FILE {
                            std::fs::read(&abs).ok()
                        } else {
                            None
                        }
                    }),
            };
            if let Some(after) = changed {
                out.push(Change {
                    path: rel.clone(),
                    kind: ChangeKind::Modified,
                    before: bytes(before),
                    after,
                });
            }
        }
        for rel in now.difference(&self.files.keys().cloned().collect()) {
            let abs = self.dir.join(rel);
            let after = std::fs::metadata(&abs)
                .ok()
                .filter(|m| m.len() <= MAX_FILE)
                .and_then(|_| std::fs::read(&abs).ok());
            out.push(Change {
                path: rel.clone(),
                kind: ChangeKind::Added,
                before: None,
                after,
            });
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    /// The contents of `rel` when the checkpoint was taken.
    pub fn original(&self, rel: &Path) -> Option<&[u8]> {
        match self.files.get(rel)? {
            Snapshot::Bytes(b) => Some(b),
            Snapshot::Stamp(..) => None,
        }
    }

    /// Whether `rel` existed when the checkpoint was taken.
    pub fn had(&self, rel: &Path) -> bool {
        self.files.contains_key(rel)
    }

    /// Accept the current state of `rel`: it no longer counts as changed.
    pub fn keep(&mut self, rel: &Path) {
        let abs = self.dir.join(rel);
        match std::fs::read(&abs) {
            Ok(bytes) => {
                self.files.insert(rel.to_path_buf(), Snapshot::Bytes(bytes));
            }
            Err(_) => {
                self.files.remove(rel);
            }
        }
    }
}

/// The configuration and hooks of the repository `dir` is in (not the
/// `.sample` hooks git ships, which never run).
fn git_files(dir: &Path) -> Vec<PathBuf> {
    let Some(git) = dir.ancestors().map(|d| d.join(".git")).find(|g| g.is_dir()) else {
        return Vec::new();
    };
    let mut out = vec![git.join("config")];
    let hooks = std::fs::read_dir(git.join("hooks")).into_iter().flatten();
    out.extend(
        hooks
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_none_or(|e| e != "sample")),
    );
    out
}

fn bytes(s: &Snapshot) -> Option<Vec<u8>> {
    match s {
        Snapshot::Bytes(b) => Some(b.clone()),
        Snapshot::Stamp(..) => None,
    }
}

/// Every file under `dir`, as the Files view lists them.
fn walk(dir: &Path) -> Vec<PathBuf> {
    fn collect(nodes: Vec<FileNode>, out: &mut Vec<PathBuf>) {
        for n in nodes {
            match n.children {
                Some(children) => collect(children, out),
                None => out.push(n.path),
            }
        }
    }
    let mut out = Vec::new();
    collect(files::scan(dir), &mut out);
    // Links are listed but may point outside the project.
    out.retain(|p| {
        std::fs::symlink_metadata(p)
            .map(|m| m.file_type().is_file())
            .unwrap_or(false)
    });
    out
}

/// The changes as a Git-style patch, for the diff viewer.
pub fn patch(changes: &[Change]) -> String {
    let mut out = String::new();
    for c in changes {
        let path = c.path.to_string_lossy().replace('\\', "/");
        out.push_str(&format!("diff --git a/{path} b/{path}\n"));
        match c.kind {
            ChangeKind::Added => out.push_str("new file mode 100644\n"),
            ChangeKind::Deleted => out.push_str("deleted file mode 100644\n"),
            ChangeKind::Modified => {}
        }
        let old_name = match c.kind {
            ChangeKind::Added => "/dev/null".to_string(),
            _ => format!("a/{path}"),
        };
        let new_name = match c.kind {
            ChangeKind::Deleted => "/dev/null".to_string(),
            _ => format!("b/{path}"),
        };
        let text = |b: &Option<Vec<u8>>| match b {
            None => Some(String::new()),
            Some(b) if b[..b.len().min(8000)].contains(&0) => None,
            Some(b) => String::from_utf8(b.clone()).ok(),
        };
        let known = |b: &Option<Vec<u8>>, exists: bool| !exists || b.is_some();
        let old_exists = c.kind != ChangeKind::Added;
        let new_exists = c.kind != ChangeKind::Deleted;
        match (text(&c.before), text(&c.after)) {
            (Some(old), Some(new))
                if known(&c.before, old_exists) && known(&c.after, new_exists) =>
            {
                if old == new {
                    continue;
                }
                out.push_str(&format!("--- {old_name}\n+++ {new_name}\n"));
                let diff = similar::TextDiff::from_lines(&old, &new);
                for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
                    out.push_str(&hunk.to_string());
                }
            }
            _ => out.push_str(&format!("Binary files {old_name} and {new_name} differ\n")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_what_changed_and_what_to_restore() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("main.tex"), "\\section{A}\nOld text.\n").unwrap();
        std::fs::create_dir(d.join("ch")).unwrap();
        std::fs::write(d.join("ch/gone.tex"), "bye\n").unwrap();
        std::fs::write(d.join("same.tex"), "same\n").unwrap();
        std::fs::create_dir_all(d.join(".texoria/build")).unwrap();
        std::fs::write(d.join(".texoria/build/main.log"), "log").unwrap();

        let cp = Checkpoint::take(d);
        assert!(cp.changes().is_empty());

        std::fs::write(d.join("main.tex"), "\\section{A}\nNew text.\n").unwrap();
        std::fs::remove_file(d.join("ch/gone.tex")).unwrap();
        std::fs::write(d.join("ch/new.tex"), "hello\n").unwrap();
        // Build output is not the agent's work.
        std::fs::write(d.join(".texoria/build/main.log"), "other log").unwrap();

        let changes = cp.changes();
        let kinds: Vec<_> = changes
            .iter()
            .map(|c| (c.path.to_string_lossy().replace('\\', "/"), c.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                ("ch/gone.tex".to_string(), ChangeKind::Deleted),
                ("ch/new.tex".to_string(), ChangeKind::Added),
                ("main.tex".to_string(), ChangeKind::Modified),
            ]
        );
        assert!(changes.iter().all(Change::restorable));
        assert_eq!(
            cp.original(Path::new("main.tex")),
            Some(&b"\\section{A}\nOld text.\n"[..])
        );
        assert!(!cp.had(Path::new("ch/new.tex")));

        let p = patch(&changes);
        assert!(p.contains("diff --git a/main.tex b/main.tex\n--- a/main.tex\n+++ b/main.tex\n"));
        assert!(p.contains("-Old text.\n+New text.\n"));
        assert!(p.contains("new file mode 100644\n--- /dev/null\n+++ b/ch/new.tex\n"));
        assert!(p.contains("deleted file mode 100644\n--- a/ch/gone.tex\n+++ /dev/null\n"));

        let mut cp = cp;
        cp.keep(Path::new("main.tex"));
        cp.keep(Path::new("ch/gone.tex"));
        cp.keep(Path::new("ch/new.tex"));
        assert!(cp.changes().is_empty());
    }

    #[test]
    fn tells_git_configuration_and_hooks_apart() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir_all(d.join(".git/hooks")).unwrap();
        std::fs::write(d.join(".git/config"), "[core]\n").unwrap();
        std::fs::write(d.join(".git/hooks/pre-commit.sample"), "#!/bin/sh\n").unwrap();
        std::fs::create_dir(d.join("paper")).unwrap();
        let cp = Checkpoint::take(&d.join("paper"));
        assert!(cp.git_changes().is_empty());

        std::fs::write(d.join(".git/hooks/pre-commit.sample"), "changed\n").unwrap();
        assert!(cp.git_changes().is_empty());
        std::fs::write(d.join(".git/config"), "[core]\n\tfsmonitor = x\n").unwrap();
        std::fs::write(d.join(".git/hooks/post-checkout"), "#!/bin/sh\n").unwrap();
        assert_eq!(
            cp.git_changes(),
            [d.join(".git/config"), d.join(".git/hooks/post-checkout")]
        );
        // Not part of the files to review or undo.
        assert!(cp.changes().is_empty());
    }

    #[test]
    fn binary_files_are_not_diffed() {
        let c = Change {
            path: PathBuf::from("fig.png"),
            kind: ChangeKind::Modified,
            before: Some(b"\x89PNG\0a".to_vec()),
            after: Some(b"\x89PNG\0b".to_vec()),
        };
        assert!(patch(&[c]).contains("Binary files a/fig.png and b/fig.png differ"));
    }
}
