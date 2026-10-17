//! The project directory as a tree, for the file browser, and the checks
//! behind creating and renaming its entries.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// Version control metadata, build output and OS clutter: never listed, and
/// nothing can be created in them.
const HIDDEN: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".texoria",
    ".DS_Store",
    "Thumbs.db",
    "desktop.ini",
];

/// Guards against opening a huge directory (a home folder) by mistake.
const MAX_DEPTH: usize = 16;
const MAX_ENTRIES: usize = 10_000;

/// How much of a file is read to tell text from binary.
const SNIFF_BYTES: usize = 8 << 10;
/// Larger files are never opened in the editor.
pub const MAX_TEXT_BYTES: u64 = 20 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileNode {
    pub path: PathBuf,
    pub name: String,
    /// `None` for a file; a directory's entries (possibly none) otherwise.
    pub children: Option<Vec<FileNode>>,
}

impl FileNode {
    pub fn is_dir(&self) -> bool {
        self.children.is_some()
    }
}

/// The entries of `dir`, recursively: directories first, then by name
/// (case-insensitive). Symbolic links show what they point to, but a linked
/// directory is not entered (it may loop).
pub fn scan(dir: &Path) -> Vec<FileNode> {
    let mut budget = MAX_ENTRIES;
    scan_dir(dir, 0, &mut budget)
}

fn scan_dir(dir: &Path, depth: usize, budget: &mut usize) -> Vec<FileNode> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if *budget == 0 {
            break;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_hidden(&name) {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        let link = kind.is_symlink();
        let is_dir = if link {
            fs::metadata(&path).is_ok_and(|m| m.is_dir())
        } else {
            kind.is_dir()
        };
        *budget -= 1;
        let children = is_dir.then(|| {
            if link || depth + 1 >= MAX_DEPTH {
                Vec::new()
            } else {
                scan_dir(&path, depth + 1, budget)
            }
        });
        out.push(FileNode {
            path,
            name,
            children,
        });
    }
    out.sort_by(|a, b| {
        b.is_dir()
            .cmp(&a.is_dir())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

fn is_hidden(name: &str) -> bool {
    HIDDEN.iter().any(|h| h.eq_ignore_ascii_case(name))
}

/// Whether `path` is a regular file the editor can show: its beginning is
/// UTF-8 without NUL bytes, and it is not huge.
pub fn looks_like_text(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() || meta.len() > MAX_TEXT_BYTES {
        return false;
    }
    let mut head = Vec::with_capacity(SNIFF_BYTES);
    let read = File::open(path).and_then(|f| f.take(SNIFF_BYTES as u64).read_to_end(&mut head));
    if read.is_err() || head.contains(&0) {
        return false;
    }
    match std::str::from_utf8(&head) {
        Ok(_) => true,
        // A multi-byte character cut by the sniffing window.
        Err(e) => e.error_len().is_none() && head.len() == SNIFF_BYTES,
    }
}

/// Documents and pictures the system may open on a double click. Anything
/// else could be a program (`.app`, `.command`, `.exe`, `.bat`, `.desktop`
/// …), and files that came with a clone are not quarantined, so the system
/// would start it without a warning.
const SAFE_TO_OPEN: &[&str] = &[
    "pdf", "ps", "eps", "png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp", "heic", "svg",
    "csv", "tsv", "ods", "xlsx", "odt", "docx", "odp", "pptx", "rtf", "mp3", "wav", "mp4", "mov",
];

/// Whether the system app may open `path` directly: a regular file (a link
/// counts as what it points to) of a [`SAFE_TO_OPEN`] type.
pub fn safe_to_open(path: &Path) -> bool {
    let Ok(target) = fs::canonicalize(path) else {
        return false;
    };
    let safe_name = |p: &Path| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| SAFE_TO_OPEN.iter().any(|s| s.eq_ignore_ascii_case(e)))
    };
    target.is_file() && safe_name(path) && safe_name(&target)
}

/// The path of a new file or directory called `name` inside `dir`. `name`
/// may have subdirectories (`chapters/intro.tex`) but must stay inside `dir`.
pub fn new_entry_path(dir: &Path, name: &str) -> Result<PathBuf, &'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a name.");
    }
    if name.contains('\\') {
        return Err("Use / to separate folders.");
    }
    let mut out = dir.to_path_buf();
    for c in Path::new(name).components() {
        match c {
            Component::Normal(part) => {
                if is_hidden(&part.to_string_lossy()) {
                    return Err("That name is reserved.");
                }
                out.push(part);
            }
            Component::CurDir => {}
            _ => return Err("The name must stay inside the folder."),
        }
    }
    if out == dir {
        return Err("Enter a name.");
    }
    Ok(out)
}

/// `path` renamed to `name`, in the same directory.
pub fn rename_target(path: &Path, name: &str) -> Result<PathBuf, &'static str> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        return Err("Enter a name.");
    }
    if name.contains(['/', '\\']) {
        return Err("A name cannot contain / or \\.");
    }
    if is_hidden(name) {
        return Err("That name is reserved.");
    }
    Ok(path.with_file_name(name))
}

/// Whether `to` names the same file as `from`: a rename that only changes
/// the case on a case-insensitive disk.
pub fn is_same_file(from: &Path, to: &Path) -> bool {
    let (Ok(a), Ok(b)) = (fs::symlink_metadata(from), fs::symlink_metadata(to)) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        a.dev() == b.dev() && a.ino() == b.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = (a, b);
        from.to_string_lossy().to_lowercase() == to.to_string_lossy().to_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(nodes: &[FileNode]) -> Vec<&str> {
        nodes.iter().map(|n| n.name.as_str()).collect()
    }

    #[test]
    fn only_documents_open_with_the_system() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for name in [
            "fig.PDF",
            "run.command",
            "setup.exe",
            "fig.pdf.bat",
            "noext",
        ] {
            fs::write(d.join(name), "x").unwrap();
        }
        fs::create_dir(d.join("Tool.app")).unwrap();
        assert!(safe_to_open(&d.join("fig.PDF")));
        for name in [
            "run.command",
            "setup.exe",
            "fig.pdf.bat",
            "noext",
            "Tool.app",
            "gone.pdf",
        ] {
            assert!(!safe_to_open(&d.join(name)), "{name}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.join("run.command"), d.join("link.pdf")).unwrap();
            assert!(!safe_to_open(&d.join("link.pdf")));
        }
    }

    #[test]
    fn scans_directories_first_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        fs::create_dir_all(d.join("chapters/old")).unwrap();
        fs::create_dir(d.join("figures")).unwrap();
        fs::create_dir_all(d.join(".git/objects")).unwrap();
        fs::create_dir_all(d.join(".texoria/build")).unwrap();
        fs::write(d.join("main.tex"), "").unwrap();
        fs::write(d.join("Appendix.tex"), "").unwrap();
        fs::write(d.join(".latexmkrc"), "").unwrap();
        fs::write(d.join(".DS_Store"), "").unwrap();
        fs::write(d.join("chapters/intro.tex"), "").unwrap();

        let tree = scan(d);
        assert_eq!(
            names(&tree),
            [
                "chapters",
                "figures",
                ".latexmkrc",
                "Appendix.tex",
                "main.tex"
            ]
        );
        let chapters = tree[0].children.as_ref().unwrap();
        assert_eq!(names(chapters), ["old", "intro.tex"]);
        // An empty directory is still a directory.
        assert_eq!(chapters[0].children, Some(Vec::new()));
        assert_eq!(tree[1].children, Some(Vec::new()));
        assert_eq!(tree[4].children, None);
        assert_eq!(tree[4].path, d.join("main.tex"));
    }

    #[test]
    fn scan_has_a_depth_limit() {
        let dir = tempfile::tempdir().unwrap();
        let deep: PathBuf = (0..MAX_DEPTH + 4).map(|i| format!("d{i}")).collect();
        fs::create_dir_all(dir.path().join(deep)).unwrap();
        let mut depth = 0;
        let mut level = scan(dir.path());
        while let Some(node) = level.first() {
            depth += 1;
            level = node.children.clone().unwrap();
        }
        assert_eq!(depth, MAX_DEPTH);
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_linked_directories() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("a.tex"), "").unwrap();
        std::os::unix::fs::symlink(dir.path(), sub.join("loop")).unwrap();
        let tree = scan(dir.path());
        let sub = tree[0].children.as_ref().unwrap();
        assert_eq!(names(sub), ["loop", "a.tex"]);
        assert_eq!(sub[0].children, Some(Vec::new()));
    }

    #[test]
    fn tells_text_from_binary() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        fs::write(d.join("a.tex"), "\\section{Intro} àèì").unwrap();
        fs::write(d.join("empty.bib"), "").unwrap();
        fs::write(d.join("b.pdf"), b"%PDF-1.5\n\0\xff\xfe").unwrap();
        fs::write(d.join("latin1.tex"), b"caf\xe9").unwrap();
        // A two-byte character straddling the sniffing window.
        let mut long = "a".repeat(SNIFF_BYTES - 1);
        long.push('é');
        fs::write(d.join("long.tex"), long).unwrap();

        assert!(looks_like_text(&d.join("a.tex")));
        assert!(looks_like_text(&d.join("empty.bib")));
        assert!(looks_like_text(&d.join("long.tex")));
        assert!(!looks_like_text(&d.join("b.pdf")));
        assert!(!looks_like_text(&d.join("latin1.tex")));
        assert!(!looks_like_text(d));
        assert!(!looks_like_text(&d.join("missing.tex")));
    }

    #[test]
    fn validates_new_entry_names() {
        let dir = Path::new("/p");
        assert_eq!(
            new_entry_path(dir, " intro.tex "),
            Ok(dir.join("intro.tex"))
        );
        assert_eq!(
            new_entry_path(dir, "chapters/intro.tex"),
            Ok(dir.join("chapters").join("intro.tex"))
        );
        assert_eq!(
            new_entry_path(dir, ".latexmkrc"),
            Ok(dir.join(".latexmkrc"))
        );
        assert!(new_entry_path(dir, "").is_err());
        assert!(new_entry_path(dir, "./").is_err());
        assert!(new_entry_path(dir, "../x.tex").is_err());
        assert!(new_entry_path(dir, "a/../../x.tex").is_err());
        assert!(new_entry_path(dir, "/etc/x").is_err());
        assert!(new_entry_path(dir, "a\\b.tex").is_err());
        assert!(new_entry_path(dir, ".git/hooks/pre-commit").is_err());
        assert!(new_entry_path(dir, "sub/.TEXORIA").is_err());
    }

    #[test]
    fn validates_renames() {
        let file = Path::new("/p/ch/a.tex");
        assert_eq!(
            rename_target(file, "b.tex"),
            Ok(PathBuf::from("/p/ch/b.tex"))
        );
        assert!(rename_target(file, " ").is_err());
        assert!(rename_target(file, "..").is_err());
        assert!(rename_target(file, "x/b.tex").is_err());
        assert!(rename_target(file, "x\\b.tex").is_err());
        assert!(rename_target(file, ".git").is_err());
    }

    #[test]
    fn same_file() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.tex"), dir.path().join("b.tex"));
        fs::write(&a, "").unwrap();
        assert!(is_same_file(&a, &a));
        assert!(!is_same_file(&a, &b));
        fs::write(&b, "").unwrap();
        assert!(!is_same_file(&a, &b));
    }
}
