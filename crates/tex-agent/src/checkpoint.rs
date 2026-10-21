//! The project as it was before the agent's turn: what it changed since,
//! whatever way it wrote the files (through TeXoria or on its own), and what
//! to put back to undo it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tex_project::files;

/// Larger files are only compared by size and date, and cannot be restored.
const MAX_FILE: u64 = 8 << 20;
/// Past this much, the remaining files are only compared by size and date.
const MAX_TOTAL: u64 = 200 << 20;
/// Past this many files, or this deep, the checkpoint is incomplete.
const MAX_FILES: usize = 200_000;
const MAX_DEPTH: usize = 64;
/// File times may be coarser than the clock.
const CLOCK_SLACK: Duration = Duration::from_secs(2);

/// How much a checkpoint keeps (smaller in tests).
#[derive(Debug, Clone, Copy)]
struct Limits {
    file: u64,
    total: u64,
    files: usize,
    depth: usize,
}

const LIMITS: Limits = Limits {
    file: MAX_FILE,
    total: MAX_TOTAL,
    files: MAX_FILES,
    depth: MAX_DEPTH,
};

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
    /// Every file was seen: one missing from `files` did not exist.
    complete: bool,
    taken: SystemTime,
    limits: Limits,
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
    /// For an added file: it surely did not exist before, so undoing may
    /// delete it.
    pub removable: bool,
}

impl Change {
    /// Whether undoing it is possible.
    pub fn restorable(&self) -> bool {
        match self.kind {
            ChangeKind::Added => self.removable,
            _ => self.before.is_some(),
        }
    }
}

impl Checkpoint {
    /// Record the files of `dir` (not `.git` nor `.texoria`). Blocking.
    pub fn take(dir: &Path) -> Checkpoint {
        Self::take_with(dir, LIMITS)
    }

    fn take_with(dir: &Path, limits: Limits) -> Checkpoint {
        let taken = SystemTime::now();
        let mut files = BTreeMap::new();
        let mut total = 0u64;
        let (paths, mut complete) = walk(dir, &limits);
        for path in paths {
            let Ok(meta) = std::fs::metadata(&path) else {
                complete = false;
                continue;
            };
            let rel = path.strip_prefix(dir).unwrap_or(&path).to_path_buf();
            let len = meta.len();
            let stamp = Snapshot::Stamp(len, meta.modified().ok());
            let snapshot = if len <= limits.file && total + len <= limits.total {
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        total += len;
                        Snapshot::Bytes(bytes)
                    }
                    // Still recorded: it existed.
                    Err(_) => stamp,
                }
            } else {
                stamp
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
            complete,
            taken,
            limits,
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
        let now: BTreeSet<PathBuf> = walk(&self.dir, &self.limits)
            .0
            .into_iter()
            .map(|p| p.strip_prefix(&self.dir).unwrap_or(&p).to_path_buf())
            .collect();
        let mut out = Vec::new();
        for (rel, before) in &self.files {
            let abs = self.dir.join(rel);
            // Asked of the file itself, not of the walk, which may stop early.
            let exists = std::fs::symlink_metadata(&abs).is_ok_and(|m| m.file_type().is_file());
            if !exists {
                out.push(Change {
                    path: rel.clone(),
                    kind: ChangeKind::Deleted,
                    before: bytes(before),
                    after: None,
                    removable: false,
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
                        if m.len() <= self.limits.file {
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
                    removable: false,
                });
            }
        }
        for rel in now.difference(&self.files.keys().cloned().collect()) {
            let abs = self.dir.join(rel);
            let meta = std::fs::metadata(&abs).ok();
            let after = meta
                .as_ref()
                .filter(|m| m.len() <= self.limits.file)
                .and_then(|_| std::fs::read(&abs).ok());
            // Deleting a file the checkpoint missed would lose it for good.
            let removable = self.complete && meta.is_some_and(|m| touched_since(&m, self.taken));
            out.push(Change {
                path: rel.clone(),
                kind: ChangeKind::Added,
                before: None,
                after,
                removable,
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
        let snapshot = std::fs::metadata(&abs).ok().and_then(|m| {
            if m.len() > self.limits.file {
                Some(Snapshot::Stamp(m.len(), m.modified().ok()))
            } else {
                std::fs::read(&abs).ok().map(Snapshot::Bytes)
            }
        });
        match snapshot {
            Some(s) => {
                self.files.insert(rel.to_path_buf(), s);
            }
            None => {
                self.files.remove(rel);
            }
        }
    }
}

/// The configuration and hooks of the repository `dir` is in (not the
/// `.sample` hooks git ships, which never run).
fn git_files(dir: &Path) -> Vec<PathBuf> {
    // The nearest `.git`: a folder, or a file naming one (worktrees and
    // submodules), like git looks for it.
    let Some(dot_git) = dir
        .ancestors()
        .map(|d| d.join(".git"))
        .find(|g| std::fs::symlink_metadata(g).is_ok())
    else {
        return Vec::new();
    };
    let top = dot_git.parent().unwrap_or(dir).to_path_buf();
    let mut out = Vec::new();
    let gitdir = if dot_git.is_dir() {
        dot_git
    } else {
        // Pointing it elsewhere changes the repository.
        out.push(dot_git.clone());
        match std::fs::read_to_string(&dot_git)
            .ok()
            .as_deref()
            .and_then(|s| s.trim_end().strip_prefix("gitdir:"))
        {
            Some(path) => lexical(&top.join(path.trim())),
            None => return out,
        }
    };
    // A linked worktree has the configuration and hooks of the main one.
    let common = std::fs::read_to_string(gitdir.join("commondir"))
        .map(|c| lexical(&gitdir.join(c.trim())))
        .unwrap_or_else(|_| gitdir.clone());
    let mut configs = vec![common.join("config")];
    if common != gitdir {
        configs.push(gitdir.join("config.worktree"));
    }
    let mut hooks = vec![common.join("hooks")];
    hooks.extend(configs.iter().filter_map(|c| hooks_path(c, &top)));
    out.extend(configs);
    for dir in hooks {
        let entries = std::fs::read_dir(dir).into_iter().flatten().flatten();
        out.extend(
            entries
                .map(|e| e.path())
                .filter(|p| p.extension().is_none_or(|e| e != "sample")),
        );
    }
    out.sort();
    out.dedup();
    out
}

/// The hooks folder that `core.hooksPath` in `config` sets (relative to the
/// working tree `top`, where hooks run).
fn hooks_path(config: &Path, top: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(config).ok()?;
    let mut core = false;
    let mut found = None;
    for line in text.lines().map(str::trim) {
        if let Some(section) = line.strip_prefix('[') {
            let name = section.split([']', ' ', '"']).next().unwrap_or("");
            core = name.eq_ignore_ascii_case("core");
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if core && key.trim().eq_ignore_ascii_case("hookspath") {
            // The last one wins, as in git.
            found = Some(value.trim().trim_matches('"').to_string());
        }
    }
    let path = found.filter(|p| !p.is_empty())?;
    Some(match path.strip_prefix("~/") {
        Some(rest) => tex_sys::paths::home(&tex_sys::paths::process_env)?.join(rest),
        None => lexical(&top.join(path)),
    })
}

/// `path` without its `.` and `..` (read as going up a name, without links).
fn lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir
                if matches!(out.components().next_back(), Some(Component::Normal(_))) =>
            {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

fn bytes(s: &Snapshot) -> Option<Vec<u8>> {
    match s {
        Snapshot::Bytes(b) => Some(b.clone()),
        Snapshot::Stamp(..) => None,
    }
}

/// Whether the file was created, written or renamed since `since`.
fn touched_since(meta: &std::fs::Metadata, since: SystemTime) -> bool {
    let since = since.checked_sub(CLOCK_SLACK).unwrap_or(since);
    #[cfg(unix)]
    {
        // The status change time follows a rename too, and cannot be set back.
        use std::os::unix::fs::MetadataExt;
        let changed = std::time::UNIX_EPOCH
            + Duration::new(
                meta.ctime().max(0) as u64,
                meta.ctime_nsec().clamp(0, 999_999_999) as u32,
            );
        if changed >= since {
            return true;
        }
    }
    [meta.modified().ok(), meta.created().ok()]
        .into_iter()
        .flatten()
        .any(|t| t >= since)
}

/// The regular files under `dir` (not links, which may point outside the
/// project, nor what the Files view hides), and whether that is all of them.
/// Unlike the Files view's scan, nothing is skipped to save time: a file
/// missed here would look added later, and an undo would delete it.
fn walk(dir: &Path, limits: &Limits) -> (Vec<PathBuf>, bool) {
    fn visit(
        dir: &Path,
        depth: usize,
        limits: &Limits,
        out: &mut Vec<PathBuf>,
        complete: &mut bool,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            *complete = false;
            return;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                *complete = false;
                continue;
            };
            if files::is_hidden(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                *complete = false;
                continue;
            };
            if kind.is_dir() {
                if depth + 1 >= limits.depth {
                    *complete = false;
                } else {
                    visit(&entry.path(), depth + 1, limits, out, complete);
                }
            } else if kind.is_file() {
                if out.len() >= limits.files {
                    *complete = false;
                    return;
                }
                out.push(entry.path());
            }
        }
    }
    let (mut out, mut complete) = (Vec::new(), true);
    visit(dir, 0, limits, &mut out, &mut complete);
    (out, complete)
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
    fn never_offers_to_delete_files_it_did_not_see() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("main.tex"), "x").unwrap();
        let mut cp = Checkpoint::take(d);
        std::fs::write(d.join("new.tex"), "y").unwrap();
        let added = |cp: &Checkpoint| {
            cp.changes()
                .into_iter()
                .find(|c| c.path == Path::new("new.tex"))
                .unwrap()
        };
        assert!(added(&cp).restorable());

        // A walk that stopped early: the file may have been there before.
        cp.complete = false;
        assert!(!added(&cp).restorable());
        // A file older than the checkpoint was there already.
        cp.complete = true;
        cp.taken = SystemTime::now() + Duration::from_secs(3600);
        assert!(!added(&cp).restorable());
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

    const SMALL: Limits = Limits {
        file: 10,
        total: 15,
        files: 100,
        depth: 8,
    };

    fn set_mtime(path: &Path, t: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    fn change<'a>(changes: &'a [Change], path: &str) -> Option<&'a Change> {
        changes.iter().find(|c| c.path == Path::new(path))
    }

    #[test]
    fn large_files_are_compared_by_size_and_date() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("a.tex"), "12345678").unwrap();
        std::fs::write(d.join("b.tex"), "abcdefgh").unwrap();
        std::fs::write(d.join("big.pdf"), "x".repeat(20)).unwrap();
        std::fs::write(d.join("same.pdf"), "y".repeat(20)).unwrap();
        let mut cp = Checkpoint::take_with(d, SMALL);
        assert!(cp.changes().is_empty());
        // Kept whole up to the total, then only stamped.
        let kept: Vec<bool> = ["a.tex", "b.tex", "big.pdf"]
            .iter()
            .map(|f| cp.original(Path::new(f)).is_some())
            .collect();
        assert_eq!(kept.iter().filter(|k| **k).count(), 1);
        assert!(cp.had(Path::new("big.pdf")) && cp.original(Path::new("big.pdf")).is_none());

        let stamped = if kept[0] { "b.tex" } else { "a.tex" };
        // Same size, another date.
        std::fs::write(d.join(stamped), "ABCDEFGH").unwrap();
        set_mtime(
            &d.join(stamped),
            SystemTime::now() + Duration::from_secs(60),
        );
        // Larger than what is kept.
        std::fs::write(d.join("big.pdf"), "x".repeat(30)).unwrap();
        let changes = cp.changes();
        assert_eq!(changes.len(), 2, "{changes:?}");
        let small = change(&changes, stamped).unwrap();
        assert_eq!(
            (small.kind, small.before.as_deref()),
            (ChangeKind::Modified, None)
        );
        assert_eq!(small.after.as_deref(), Some(&b"ABCDEFGH"[..]));
        let big = change(&changes, "big.pdf").unwrap();
        assert_eq!(
            (big.kind, &big.before, &big.after),
            (ChangeKind::Modified, &None, &None)
        );
        assert!(!small.restorable() && !big.restorable());
        let p = patch(&changes);
        assert!(
            p.contains("Binary files a/big.pdf and b/big.pdf differ"),
            "{p}"
        );

        // Accepted: a large file stays a stamp.
        cp.keep(Path::new("big.pdf"));
        cp.keep(Path::new(stamped));
        assert!(cp.changes().is_empty());
        assert!(cp.original(Path::new("big.pdf")).is_none());
        assert!(cp.original(Path::new(stamped)).is_some());
        std::fs::remove_file(d.join("big.pdf")).unwrap();
        cp.keep(Path::new("big.pdf"));
        assert!(!cp.had(Path::new("big.pdf")));
        assert!(cp.changes().is_empty());
    }

    #[test]
    fn added_files_too_large_to_keep() {
        let dir = tempfile::tempdir().unwrap();
        let cp = Checkpoint::take_with(dir.path(), SMALL);
        std::fs::write(dir.path().join("out.pdf"), "x".repeat(30)).unwrap();
        let changes = cp.changes();
        assert_eq!(changes[0].after, None);
        assert!(changes[0].restorable(), "it surely was not there");
        assert!(patch(&changes).contains("Binary files /dev/null and b/out.pdf differ"));
    }

    #[test]
    fn skips_hidden_entries_and_links() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        // What the Files view hides; other dot files are the project's.
        std::fs::create_dir_all(d.join(".hg")).unwrap();
        std::fs::write(d.join(".hg/x"), "x").unwrap();
        std::fs::write(d.join(".latexmkrc"), "x").unwrap();
        std::fs::write(d.join(".DS_Store"), "x").unwrap();
        std::fs::write(d.join("main.tex"), "x").unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "s").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path().join("secret"), d.join("link.tex")).unwrap();
        let cp = Checkpoint::take(d);
        assert!(cp.had(Path::new("main.tex")) && cp.had(Path::new(".latexmkrc")));
        assert_eq!(cp.files.len(), 2, "{:?}", cp.files.keys());

        std::fs::write(d.join(".hg/x"), "changed").unwrap();
        std::fs::write(d.join(".hg/new"), "new").unwrap();
        std::fs::write(d.join("Thumbs.db"), "new").unwrap();
        std::fs::write(outside.path().join("secret"), "changed").unwrap();
        assert!(cp.changes().is_empty());
        // A file replaced by a link is gone from the project.
        #[cfg(unix)]
        {
            std::fs::remove_file(d.join("main.tex")).unwrap();
            std::os::unix::fs::symlink(outside.path().join("secret"), d.join("main.tex")).unwrap();
            let changes = cp.changes();
            assert_eq!(changes.len(), 1);
            assert_eq!(changes[0].kind, ChangeKind::Deleted);
            assert!(changes[0].restorable());
        }
    }

    #[test]
    fn too_many_files_or_too_deep_is_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for f in ["a", "b", "c"] {
            std::fs::write(d.join(f), f).unwrap();
        }
        let few = Limits { files: 2, ..SMALL };
        let cp = Checkpoint::take_with(d, few);
        assert!(!cp.complete);
        assert_eq!(cp.files.len(), 2);

        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir_all(d.join("1/2/3")).unwrap();
        std::fs::write(d.join("1/2/3/deep.tex"), "x").unwrap();
        std::fs::write(d.join("1/shallow.tex"), "x").unwrap();
        let shallow = Limits { depth: 2, ..SMALL };
        let cp = Checkpoint::take_with(d, shallow);
        assert!(!cp.complete);
        assert!(cp.had(Path::new("1/shallow.tex")));
        assert!(!cp.had(Path::new("1/2/3/deep.tex")));
        // A file it could not see may have been there: never deleted.
        std::fs::write(d.join("new.tex"), "x").unwrap();
        let added = cp.changes();
        assert_eq!(added.len(), 1);
        assert!(!added[0].restorable());
        assert!(Checkpoint::take(d).complete);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_files_are_stamped() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("locked.tex");
        std::fs::write(&f, "x").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&f).is_ok() {
            // Running as root: nothing is unreadable.
            return;
        }
        let cp = Checkpoint::take(dir.path());
        assert!(cp.had(Path::new("locked.tex")));
        assert!(cp.original(Path::new("locked.tex")).is_none());
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[test]
    fn new_files_with_an_old_date_count_as_added_now() {
        let dir = tempfile::tempdir().unwrap();
        let cp = Checkpoint::take(dir.path());
        let f = dir.path().join("old.tex");
        std::fs::write(&f, "x").unwrap();
        // As a copy that keeps the original's date would be.
        set_mtime(&f, SystemTime::now() - Duration::from_secs(24 * 3600));
        let changes = cp.changes();
        assert!(changes[0].removable, "created after the checkpoint");
    }

    #[test]
    fn no_repository_no_git_files() {
        let dir = tempfile::tempdir().unwrap();
        if dir.path().ancestors().any(|a| a.join(".git").exists()) {
            return;
        }
        let cp = Checkpoint::take(dir.path());
        assert!(cp.git.is_empty() && cp.git_changes().is_empty());
        assert_eq!(cp.dir(), dir.path());
    }

    #[test]
    fn identical_and_undecodable_contents() {
        let c = |kind, before: Option<&[u8]>, after: Option<&[u8]>| Change {
            path: PathBuf::from("ch").join("a.tex"),
            kind,
            before: before.map(<[u8]>::to_vec),
            after: after.map(<[u8]>::to_vec),
            removable: false,
        };
        assert_eq!(
            patch(&[c(ChangeKind::Modified, Some(b"x"), Some(b"x"))]),
            "diff --git a/ch/a.tex b/ch/a.tex\n"
        );
        let p = patch(&[c(ChangeKind::Modified, Some(b"caf\xe9"), Some(b"cafe"))]);
        assert!(
            p.ends_with("Binary files a/ch/a.tex and b/ch/a.tex differ\n"),
            "{p}"
        );
        let p = patch(&[c(ChangeKind::Deleted, None, None)]);
        assert!(
            p.ends_with("Binary files a/ch/a.tex and /dev/null differ\n"),
            "{p}"
        );
        let p = patch(&[c(ChangeKind::Added, None, Some(b"new\n"))]);
        assert!(
            p.ends_with("--- /dev/null\n+++ b/ch/a.tex\n@@ -0,0 +1 @@\n+new\n"),
            "{p}"
        );
    }

    proptest::proptest! {
        #[test]
        fn patches_never_panic(
            before in proptest::option::of(proptest::collection::vec(proptest::num::u8::ANY, 0..64)),
            after in proptest::option::of(proptest::collection::vec(proptest::num::u8::ANY, 0..64)),
            kind in proptest::sample::select(vec![ChangeKind::Added, ChangeKind::Modified, ChangeKind::Deleted]),
            path in "\\PC{1,12}",
        ) {
            let p = patch(&[Change { path: PathBuf::from(path), kind, before, after, removable: true }]);
            proptest::prop_assert!(p.starts_with("diff --git a/"));
        }
    }

    #[test]
    fn watches_the_repository_a_git_file_names() {
        // A submodule: its `.git` is a file naming a folder of the outer
        // repository's, which is not the submodule's repository.
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path();
        let modules = outer.join(".git/modules/sub");
        std::fs::create_dir_all(modules.join("hooks")).unwrap();
        std::fs::write(outer.join(".git/config"), "[core]\n").unwrap();
        std::fs::write(modules.join("config"), "[core]\n").unwrap();
        std::fs::create_dir(outer.join("sub")).unwrap();
        std::fs::write(outer.join("sub/.git"), "gitdir: ../.git/modules/sub\n").unwrap();
        let cp = Checkpoint::take(&outer.join("sub"));
        assert!(cp.git_changes().is_empty());

        std::fs::write(outer.join(".git/config"), "[core]\n\tfsmonitor = x\n").unwrap();
        assert!(cp.git_changes().is_empty(), "not this project's repository");
        std::fs::write(modules.join("config"), "[core]\n\tfsmonitor = x\n").unwrap();
        std::fs::write(modules.join("hooks/pre-commit"), "#!/bin/sh\n").unwrap();
        assert_eq!(
            cp.git_changes(),
            [modules.join("config"), modules.join("hooks/pre-commit")]
        );
        // Pointing the `.git` file elsewhere is a change too.
        std::fs::write(outer.join("sub/.git"), "gitdir: /tmp/elsewhere\n").unwrap();
        assert!(cp.git_changes().contains(&outer.join("sub/.git")));
    }

    #[test]
    fn worktrees_share_the_main_configuration_and_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main/.git");
        let linked = main.join("worktrees/wt");
        std::fs::create_dir_all(main.join("hooks")).unwrap();
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(main.join("config"), "[core]\n").unwrap();
        std::fs::write(linked.join("commondir"), "../..\n").unwrap();
        let wt = dir.path().join("wt");
        std::fs::create_dir(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", linked.display())).unwrap();
        let cp = Checkpoint::take(&wt);
        assert!(cp.git_changes().is_empty());

        std::fs::write(main.join("hooks/post-checkout"), "#!/bin/sh\n").unwrap();
        std::fs::write(linked.join("config.worktree"), "[core]\n\tfsmonitor = x\n").unwrap();
        let changed: Vec<PathBuf> = cp
            .git_changes()
            .iter()
            .map(|p| p.strip_prefix(dir.path()).unwrap().to_path_buf())
            .collect();
        let rel = |p: &str| PathBuf::from_iter(p.split('/'));
        assert_eq!(
            changed,
            [
                rel("main/.git/hooks/post-checkout"),
                rel("main/.git/worktrees/wt/config.worktree"),
            ]
        );
    }

    #[test]
    fn watches_the_hooks_of_core_hooks_path() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let hooks = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.join(".git/my-hooks")).unwrap();
        // Relative to the working tree, or absolute.
        for (config, folder) in [
            (
                "[core]\n\thooksPath = .git/my-hooks\n",
                d.join(".git/my-hooks"),
            ),
            (
                &*format!("[Core]\n  HooksPath = \"{}\"\n", hooks.path().display()),
                hooks.path().to_path_buf(),
            ),
        ] {
            std::fs::write(d.join(".git/config"), config).unwrap();
            let cp = Checkpoint::take(d);
            assert!(cp.git_changes().is_empty());
            let hook = folder.join("pre-push");
            std::fs::write(&hook, "#!/bin/sh\n").unwrap();
            assert_eq!(cp.git_changes(), std::slice::from_ref(&hook), "{config}");
            std::fs::remove_file(&hook).unwrap();
        }
        // Another section's key is not the hooks path.
        std::fs::write(
            d.join(".git/config"),
            "[alias]\n\thooksPath = .git/my-hooks\n",
        )
        .unwrap();
        let cp = Checkpoint::take(d);
        std::fs::write(d.join(".git/my-hooks/pre-push"), "#!/bin/sh\n").unwrap();
        assert!(cp.git_changes().is_empty());
    }

    #[test]
    fn binary_files_are_not_diffed() {
        let c = Change {
            path: PathBuf::from("fig.png"),
            kind: ChangeKind::Modified,
            before: Some(b"\x89PNG\0a".to_vec()),
            after: Some(b"\x89PNG\0b".to_vec()),
            removable: false,
        };
        assert!(patch(&[c]).contains("Binary files a/fig.png and b/fig.png differ"));
    }
}
