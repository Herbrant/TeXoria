//! Minimal SyncTeX reader: forward (source → PDF) and inverse (PDF → source)
//! search over `.synctex` / `.synctex.gz` files.
//!
//! All public coordinates are PDF points (1/72 in) with the origin at the
//! top-left corner of the page and `y` growing downwards, which is what a
//! viewer naturally works with.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a SyncTeX file")]
    BadHeader,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }

    fn union(&self, o: &Rect) -> Rect {
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = (self.x + self.width).max(o.x + o.width);
        let y1 = (self.y + self.height).max(o.y + o.height);
        Rect {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        }
    }

    fn distance2(&self, x: f32, y: f32) -> f32 {
        let dx = (self.x - x).max(0.0).max(x - (self.x + self.width));
        let dy = (self.y - y).max(0.0).max(y - (self.y + self.height));
        dx * dx + dy * dy
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `(` / `h`: horizontal boxes — the records that map to lines of text.
    HBox,
    /// `[` / `v`
    VBox,
    /// `k`, `g`, `$`, `x`: point-like records.
    Point,
}

#[derive(Debug, Clone, Copy)]
struct Record {
    kind: Kind,
    tag: u32,
    line: u32,
    rect: Rect,
}

/// A position in the output document.
#[derive(Debug, Clone, PartialEq)]
pub struct PdfLocation {
    /// 0-based page index.
    pub page: usize,
    /// Highlight rectangles on that page (one per matching line box).
    pub rects: Vec<Rect>,
}

/// A position in a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLocation {
    pub file: PathBuf,
    /// 1-based line.
    pub line: u32,
}

#[derive(Debug, Default)]
pub struct SyncTex {
    inputs: HashMap<u32, PathBuf>,
    pages: Vec<Vec<Record>>,
}

impl SyncTex {
    /// Load a `.synctex.gz` or plain `.synctex` file. Relative input paths are
    /// resolved against the directory of the synctex file.
    pub fn open(path: &Path) -> Result<Self, Error> {
        let bytes = std::fs::read(path)?;
        let text = if bytes.starts_with(&[0x1f, 0x8b]) {
            let mut s = String::new();
            GzDecoder::new(&bytes[..]).read_to_string(&mut s)?;
            s
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        };
        let base = path.parent().unwrap_or(Path::new("."));
        Self::parse(&text, base)
    }

    pub fn parse(text: &str, base: &Path) -> Result<Self, Error> {
        let mut lines = text.lines();
        if !lines
            .next()
            .is_some_and(|l| l.starts_with("SyncTeX Version:"))
        {
            return Err(Error::BadHeader);
        }

        let mut st = SyncTex::default();
        let mut unit = 1.0f32;
        let mut magnification = 1000.0f32;
        let mut x_offset = 0.0f32;
        let mut y_offset = 0.0f32;
        let mut current: Option<Vec<Record>> = None;
        // sp → TeX pt → PDF bp
        let mut scale = 1.0f32;

        let mut in_content = false;
        for line in lines {
            if !in_content {
                if let Some(rest) = line.strip_prefix("Input:") {
                    if let Some((tag, path)) = rest.split_once(':')
                        && let Ok(tag) = tag.parse()
                    {
                        st.inputs.insert(tag, normalize(base, path));
                    }
                } else if let Some(v) = line.strip_prefix("Unit:") {
                    unit = v.trim().parse().unwrap_or(1.0);
                } else if let Some(v) = line.strip_prefix("Magnification:") {
                    magnification = v.trim().parse().unwrap_or(1000.0);
                } else if let Some(v) = line.strip_prefix("X Offset:") {
                    x_offset = v.trim().parse().unwrap_or(0.0);
                } else if let Some(v) = line.strip_prefix("Y Offset:") {
                    y_offset = v.trim().parse().unwrap_or(0.0);
                } else if line.starts_with("Content:") {
                    in_content = true;
                    scale = unit * (magnification / 1000.0) / 65536.0 * (72.0 / 72.27);
                }
                continue;
            }

            // Inputs may also appear inside the content section.
            if let Some(rest) = line.strip_prefix("Input:") {
                if let Some((tag, path)) = rest.split_once(':')
                    && let Ok(tag) = tag.parse()
                {
                    st.inputs.insert(tag, normalize(base, path));
                }
                continue;
            }
            if line.starts_with("Postamble:") {
                break;
            }

            let Some(first) = line.bytes().next() else {
                continue;
            };
            match first {
                b'{' => {
                    let n: usize = line[1..].trim().parse().unwrap_or(st.pages.len() + 1);
                    // Pages are 1-based and normally sequential; tolerate gaps.
                    while st.pages.len() + 1 < n {
                        st.pages.push(Vec::new());
                    }
                    current = Some(Vec::new());
                }
                b'}' => {
                    if let Some(page) = current.take() {
                        st.pages.push(page);
                    }
                }
                b'(' | b'h' | b'[' | b'v' | b'k' | b'g' | b'$' | b'x' => {
                    let Some(page) = current.as_mut() else {
                        continue;
                    };
                    let kind = match first {
                        b'(' | b'h' => Kind::HBox,
                        b'[' | b'v' => Kind::VBox,
                        _ => Kind::Point,
                    };
                    if let Some(rec) = parse_record(&line[1..], kind, scale, x_offset, y_offset) {
                        page.push(rec);
                    }
                }
                _ => {}
            }
        }
        if let Some(page) = current.take() {
            st.pages.push(page);
        }
        Ok(st)
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    fn tag_for(&self, file: &Path) -> Option<u32> {
        let wanted = normalize(Path::new("/"), &file.to_string_lossy());
        // Prefer an exact match, then fall back to comparing file names
        // (handles symlinks / differing absolute prefixes).
        self.inputs
            .iter()
            .find(|(_, p)| **p == wanted)
            .or_else(|| {
                self.inputs
                    .iter()
                    .find(|(_, p)| p.file_name() == wanted.file_name())
            })
            .map(|(t, _)| *t)
    }

    /// Source → PDF. Picks the exact line if present, otherwise the closest
    /// following line, otherwise the closest preceding one.
    pub fn forward(&self, file: &Path, line: u32) -> Option<PdfLocation> {
        let tag = self.tag_for(file)?;
        let mut best: Option<u32> = None;
        let mut best_before: Option<u32> = None;
        for rec in self
            .pages
            .iter()
            .flatten()
            .filter(|r| r.tag == tag && r.line > 0)
        {
            if rec.line >= line {
                best = Some(best.map_or(rec.line, |b| b.min(rec.line)));
            } else {
                best_before = Some(best_before.map_or(rec.line, |b| b.max(rec.line)));
            }
        }
        let target = best.or(best_before)?;

        let page = self
            .pages
            .iter()
            .position(|p| p.iter().any(|r| r.tag == tag && r.line == target))?;
        let mut rects: Vec<Rect> = Vec::new();
        for rec in self.pages[page]
            .iter()
            .filter(|r| r.tag == tag && r.line == target && r.kind == Kind::HBox)
            .filter(|r| r.rect.width > 0.0)
        {
            // Merge boxes that sit on the same baseline into one highlight.
            match rects.iter_mut().find(|q| (q.y - rec.rect.y).abs() < 1.0) {
                Some(q) => *q = q.union(&rec.rect),
                None => rects.push(rec.rect),
            }
        }
        if rects.is_empty() {
            // Only point records: synthesize a thin marker.
            let r = self.pages[page]
                .iter()
                .find(|r| r.tag == tag && r.line == target)?;
            rects.push(Rect {
                x: r.rect.x,
                y: r.rect.y - 8.0,
                width: 2.0,
                height: 10.0,
            });
        }
        Some(PdfLocation { page, rects })
    }

    /// PDF → source. `x`/`y` are PDF points from the top-left of `page`.
    pub fn inverse(&self, page: usize, x: f32, y: f32) -> Option<SourceLocation> {
        let records = self.pages.get(page)?;
        let candidate = records
            .iter()
            .filter(|r| r.kind == Kind::HBox && r.line > 0 && r.rect.contains(x, y))
            .min_by(|a, b| area(a).total_cmp(&area(b)))
            .or_else(|| {
                records
                    .iter()
                    .filter(|r| r.kind != Kind::VBox && r.line > 0)
                    .min_by(|a, b| a.rect.distance2(x, y).total_cmp(&b.rect.distance2(x, y)))
            })?;
        Some(SourceLocation {
            file: self.inputs.get(&candidate.tag)?.clone(),
            line: candidate.line,
        })
    }
}

fn area(r: &Record) -> f32 {
    r.rect.width * r.rect.height
}

/// Parse `tag,line[,col]:x,y[:W,H,D]` (already stripped of the type char).
fn parse_record(s: &str, kind: Kind, scale: f32, xo: f32, yo: f32) -> Option<Record> {
    let mut parts = s.split(':');
    let mut link = parts.next()?.split(',');
    let tag = link.next()?.parse().ok()?;
    let line = link.next()?.parse::<i64>().ok()?.max(0) as u32;
    let mut pos = parts.next()?.split(',');
    let x: f32 = pos.next()?.parse().ok()?;
    let y: f32 = pos.next()?.parse().ok()?;
    let (w, h, d) = match parts.next() {
        Some(size) => {
            let mut it = size.split(',').map(|v| v.parse::<f32>().unwrap_or(0.0));
            (
                it.next().unwrap_or(0.0),
                it.next().unwrap_or(0.0),
                it.next().unwrap_or(0.0),
            )
        }
        None => (0.0, 0.0, 0.0),
    };
    let x = (x + xo) * scale;
    let y = (y + yo) * scale;
    let (w, h, d) = (w * scale, h * scale, d * scale);
    // Boxes can have negative widths (e.g. after \hskip-); normalize.
    let (x, w) = if w < 0.0 { (x + w, -w) } else { (x, w) };
    Some(Record {
        kind,
        tag,
        line,
        rect: Rect {
            x,
            y: y - h,
            width: w,
            height: h + d,
        },
    })
}

/// Resolve `path` against `base` and drop `.` / `..` components lexically.
fn normalize(base: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
SyncTeX Version:1
Input:1:/proj/./main.tex
Input:2:/proj/chap.tex
Output:pdf
Magnification:1000
Unit:1
X Offset:0
Y Offset:0
Content:
!100
{1
[1,5:4736286,3670016:25000000,40000000,0
(1,10:4736286,6000000:20000000,500000,100000
h1,10:4736286,6000000:3000000,500000,100000
g1,10:9000000,6000000
)
(2,3:4736286,8000000:20000000,500000,100000
)
]
}1
{2
[1,20:4736286,3670016:25000000,40000000,0
(1,20:4736286,5000000:20000000,500000,100000
)
]
}2
Postamble:
Count:10
";

    fn sp(v: f32) -> f32 {
        v / 65536.0 * 72.0 / 72.27
    }

    #[test]
    fn forward_exact_and_nearest() {
        let st = SyncTex::parse(SAMPLE, Path::new("/proj")).unwrap();
        assert_eq!(st.page_count(), 2);

        let loc = st.forward(Path::new("/proj/main.tex"), 10).unwrap();
        assert_eq!(loc.page, 0);
        assert_eq!(loc.rects.len(), 1);
        let r = loc.rects[0];
        assert!((r.x - sp(4736286.0)).abs() < 0.01);
        assert!((r.y - sp(6000000.0 - 500000.0)).abs() < 0.01);

        // Line 15 has no records: next following line (20) is on page 2.
        assert_eq!(st.forward(Path::new("/proj/main.tex"), 15).unwrap().page, 1);
        // Included file.
        assert_eq!(st.forward(Path::new("/proj/chap.tex"), 3).unwrap().page, 0);
    }

    #[test]
    fn inverse_picks_smallest_box() {
        let st = SyncTex::parse(SAMPLE, Path::new("/proj")).unwrap();
        let x = sp(4736286.0 + 100.0);
        let y = sp(6000000.0 - 100.0);
        let loc = st.inverse(0, x, y).unwrap();
        assert_eq!(
            loc,
            SourceLocation {
                file: PathBuf::from("/proj/main.tex"),
                line: 10
            }
        );

        let y2 = sp(8000000.0 - 100.0);
        assert_eq!(
            st.inverse(0, x, y2).unwrap().file,
            PathBuf::from("/proj/chap.tex")
        );
    }

    #[test]
    fn real_pdflatex_output() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/synctex");
        let st = SyncTex::open(&dir.join("sample.synctex.gz")).unwrap();
        assert_eq!(st.page_count(), 2);
        // The fixture was written on Unix: on Windows `/proj/...` has no drive
        // letter, so it resolves onto the fixture's drive.
        let main = normalize(&dir, "/proj/main.tex");

        // A paragraph is attributed to the line where it ends; asking for an
        // earlier line of it must still land on page 1.
        let loc = st.forward(&main, 17).unwrap();
        assert_eq!(loc.page, 0);
        let r = loc.rects[0];
        let back = st
            .inverse(loc.page, r.x + r.width / 2.0, r.y + r.height / 2.0)
            .unwrap();
        assert_eq!(back.file, main);
        assert!((17..=19).contains(&back.line), "{back:?}");

        // `./` components in `Input:` lines are normalized away.
        let methods = normalize(&dir, "/proj/chapters/methods.tex");
        let loc = st.forward(&methods, 3).unwrap();
        let r = loc.rects[0];
        let back = st
            .inverse(loc.page, r.x + 1.0, r.y + r.height / 2.0)
            .unwrap();
        assert_eq!(back.file, methods);
        assert!((3..=4).contains(&back.line), "{back:?}");
    }

    #[test]
    fn reads_gzip() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.synctex.gz");
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(SAMPLE.as_bytes()).unwrap();
        std::fs::write(&path, enc.finish().unwrap()).unwrap();
        assert_eq!(SyncTex::open(&path).unwrap().page_count(), 2);
    }
}
