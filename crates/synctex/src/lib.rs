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

/// Upper bound on the (decompressed) file: real ones are a few MB, and a
/// `.synctex.gz` may come with an untrusted project (a gzip bomb).
const MAX_BYTES: u64 = 256 << 20;
/// Largest gap tolerated between consecutive page numbers.
const MAX_PAGE_GAP: usize = 10_000;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a SyncTeX file")]
    BadHeader,
    #[error("SyncTeX file too large")]
    TooLarge,
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
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.starts_with(&[0x1f, 0x8b]) {
            let mut out = Vec::new();
            GzDecoder::new(&bytes[..])
                .take(MAX_BYTES + 1)
                .read_to_end(&mut out)?;
            bytes = out;
        }
        if bytes.len() as u64 > MAX_BYTES {
            return Err(Error::TooLarge);
        }
        let text = String::from_utf8_lossy(&bytes);
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
                    unit = number(v).unwrap_or(1.0);
                } else if let Some(v) = line.strip_prefix("Magnification:") {
                    magnification = number(v).unwrap_or(1000.0);
                } else if let Some(v) = line.strip_prefix("X Offset:") {
                    x_offset = number(v).unwrap_or(0.0);
                } else if let Some(v) = line.strip_prefix("Y Offset:") {
                    y_offset = number(v).unwrap_or(0.0);
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
                    // A page left open ends where the next one starts.
                    if let Some(page) = current.take() {
                        st.pages.push(page);
                    }
                    let next = st.pages.len() + 1;
                    let n = line[1..]
                        .trim()
                        .parse::<usize>()
                        .ok()
                        .filter(|n| n.saturating_sub(next) <= MAX_PAGE_GAP)
                        .unwrap_or(next);
                    // Pages are 1-based and normally sequential; tolerate
                    // (bounded) gaps.
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
        // The input whose path ends with the most components of the wanted
        // one: the same path, or else at least the same file name (symlinks,
        // differing absolute prefixes). The lowest tag on a tie.
        let common_tail = |p: &Path| {
            p.components()
                .rev()
                .zip(wanted.components().rev())
                .take_while(|(a, b)| a == b)
                .count()
        };
        self.inputs
            .iter()
            .map(|(t, p)| (common_tail(p), *t))
            .filter(|(n, _)| *n > 0)
            .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
            .map(|(_, t)| t)
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
    // Saturate: a wrapped-around number would be some real line.
    let line = link.next()?.parse::<i64>().ok()?.clamp(0, u32::MAX.into()) as u32;
    let mut pos = parts.next()?.split(',');
    let x = number(pos.next()?)?;
    let y = number(pos.next()?)?;
    let (w, h, d) = match parts.next() {
        Some(size) => {
            let mut it = size.split(',').map(|v| number(v).unwrap_or(0.0));
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

/// A finite number: `"nan"` and `"inf"` parse as `f32` too.
fn number(s: &str) -> Option<f32> {
    s.trim().parse::<f32>().ok().filter(|v| v.is_finite())
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
    fn huge_page_number_is_bounded() {
        let text = SAMPLE.replace("{2\n", "{4000000000\n");
        let st = SyncTex::parse(&text, Path::new("/proj")).unwrap();
        assert_eq!(st.page_count(), 2);
    }

    #[test]
    fn rejects_gzip_bomb() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.synctex.gz");
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        enc.write_all(SAMPLE.as_bytes()).unwrap();
        let zeros = vec![0u8; 1 << 20];
        for _ in 0..=(MAX_BYTES >> 20) {
            enc.write_all(&zeros).unwrap();
        }
        std::fs::write(&path, enc.finish().unwrap()).unwrap();
        assert!(matches!(SyncTex::open(&path), Err(Error::TooLarge)));
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

    #[test]
    fn reads_plain_files_relative_to_their_folder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.synctex");
        std::fs::write(&path, doc("Input:3:./sub/../intro.tex\n", PAGE_OF_INTRO)).unwrap();
        let st = SyncTex::open(&path).unwrap();
        assert_eq!(st.page_count(), 1);
        let intro = normalize(dir.path(), "intro.tex");
        assert_eq!(st.forward(&intro, 4).unwrap().page, 0);
    }

    #[test]
    fn open_errors() {
        let dir = tempfile::tempdir().unwrap();
        let missing = SyncTex::open(&dir.path().join("none.synctex"));
        assert!(matches!(missing, Err(Error::Io(_))), "{missing:?}");
        let bad = dir.path().join("bad.synctex");
        std::fs::write(&bad, "not synctex\n").unwrap();
        assert!(matches!(SyncTex::open(&bad), Err(Error::BadHeader)));
        // A truncated gzip stream.
        let gz = dir.path().join("cut.synctex.gz");
        std::fs::write(&gz, [0x1f, 0x8b, 8, 0, 0, 0]).unwrap();
        assert!(matches!(SyncTex::open(&gz), Err(Error::Io(_))));
        assert!(Error::BadHeader.to_string().contains("SyncTeX"));
    }

    #[test]
    fn bad_headers() {
        for text in ["", "\n", "Content:\n", "synctex version:1\n"] {
            assert!(
                matches!(SyncTex::parse(text, Path::new("/")), Err(Error::BadHeader)),
                "{text:?}"
            );
        }
    }

    /// A SyncTeX file with `main.tex` (tag 1) and `chap.tex` (tag 2) in `/proj`.
    fn doc(preamble: &str, content: &str) -> String {
        format!(
            "SyncTeX Version:1\nInput:1:/proj/main.tex\nInput:2:/proj/chap.tex\n{preamble}\
             Output:pdf\nContent:\n{content}Postamble:\nCount:1\n"
        )
    }

    const PAGE_OF_INTRO: &str = "{1\n(3,4:65536,655360:6553600,65536,0\n)\n}1\n";

    #[test]
    fn units_magnification_and_offsets() {
        let content = "{1\n(1,1:100,200:1000,10,0\n)\n}1\n";
        let at = |preamble: &str| {
            let st = SyncTex::parse(&doc(preamble, content), Path::new("/proj")).unwrap();
            st.forward(Path::new("/proj/main.tex"), 1).unwrap().rects[0]
        };
        let plain = at("");
        assert!((plain.x - sp(100.0)).abs() < 1e-4);
        assert!((plain.width - sp(1000.0)).abs() < 1e-4);
        let scaled = at("Unit:2\nMagnification:500\n");
        // 2 × 0.5: the same.
        assert!((scaled.x - plain.x).abs() < 1e-4, "{scaled:?}");
        let doubled = at("Unit:4\nMagnification:500\n");
        assert!((doubled.width - 2.0 * plain.width).abs() < 1e-4);
        let moved = at("X Offset:50\nY Offset:-100\n");
        assert!((moved.x - sp(150.0)).abs() < 1e-4);
        assert!((moved.y - sp(100.0 - 10.0)).abs() < 1e-4);
        // Unreadable values fall back to the defaults.
        let junk = at("Unit:x\nMagnification:\nX Offset:?\nY Offset:abc\n");
        assert_eq!(junk, plain);
        let infinite = at("Unit:inf\nMagnification:NaN\nX Offset:-inf\nY Offset:1e40\n");
        assert_eq!(infinite, plain);
    }

    #[test]
    fn inputs_inside_the_content() {
        let content =
            format!("Input:3:/proj/late.tex\nInput:x:/ignored.tex\nInput:nocolon\n{PAGE_OF_INTRO}");
        let st = SyncTex::parse(&doc("", &content), Path::new("/proj")).unwrap();
        let loc = st.forward(Path::new("/proj/late.tex"), 4).unwrap();
        assert_eq!(loc.page, 0);
        assert_eq!(st.inputs.len(), 3);
    }

    #[test]
    fn page_gaps_are_filled() {
        let content = format!("{{1\n}}1\n{}", PAGE_OF_INTRO.replace("{1", "{4"));
        let st =
            SyncTex::parse(&doc("Input:3:/proj/intro.tex\n", &content), Path::new("/")).unwrap();
        assert_eq!(st.page_count(), 4);
        assert_eq!(st.forward(Path::new("/proj/intro.tex"), 4).unwrap().page, 3);
        // A page number going backwards is the next page.
        let content = format!("{PAGE_OF_INTRO}{PAGE_OF_INTRO}");
        let st = SyncTex::parse(&doc("", &content), Path::new("/")).unwrap();
        assert_eq!(st.page_count(), 2);
        // An unreadable one too.
        let st = SyncTex::parse(&doc("", "{1\n}1\n{x\n}x\n"), Path::new("/")).unwrap();
        assert_eq!(st.page_count(), 2);
    }

    #[test]
    fn page_without_closing_brace_before_the_next() {
        let content = "{1\n(1,3:0,655360:655360,65536,0\n{2\n(1,9:0,655360:655360,65536,0\n}2\n";
        let st = SyncTex::parse(&doc("", content), Path::new("/")).unwrap();
        assert_eq!(st.page_count(), 2);
        assert_eq!(st.forward(Path::new("/proj/main.tex"), 3).unwrap().page, 0);
        assert_eq!(st.forward(Path::new("/proj/main.tex"), 9).unwrap().page, 1);
    }

    #[test]
    fn last_page_without_closing_brace() {
        let text = "SyncTeX Version:1\nInput:3:/proj/intro.tex\nContent:\n{1\n(3,4:0,10:5,1,0\n";
        let st = SyncTex::parse(text, Path::new("/")).unwrap();
        assert_eq!(st.page_count(), 1);
        assert!(st.forward(Path::new("/proj/intro.tex"), 4).is_some());
    }

    #[test]
    fn records_outside_pages_and_junk_are_ignored() {
        let content =
            "(1,1:0,0:5,5,0\n\nzzz\n!42\n{1\n(1,x:0,0\n(1,1\n(1,1:0\nh\n(1,2:0,10:5,1,0\n}1\n";
        let st = SyncTex::parse(&doc("", content), Path::new("/")).unwrap();
        assert_eq!(st.page_count(), 1);
        assert_eq!(st.pages[0].len(), 1);
        assert_eq!(st.pages[0][0].line, 2);
    }

    #[test]
    fn forward_misses() {
        let st = SyncTex::parse(SAMPLE, Path::new("/proj")).unwrap();
        assert_eq!(st.forward(Path::new("/elsewhere/unknown.tex"), 1), None);
        // Known file, but no record of it.
        let st = SyncTex::parse(&doc("", "{1\n}1\n"), Path::new("/")).unwrap();
        assert_eq!(st.forward(Path::new("/proj/main.tex"), 1), None);
    }

    #[test]
    fn forward_matches_by_file_name() {
        // Another absolute prefix (a symlink, a moved project): same name.
        let st = SyncTex::parse(SAMPLE, Path::new("/proj")).unwrap();
        assert_eq!(
            st.forward(Path::new("/other/place/chap.tex"), 3)
                .unwrap()
                .page,
            0
        );
    }

    #[test]
    fn file_name_fallback_prefers_the_closest_path() {
        // Two `intro.tex`, the project seen under another prefix: the one
        // whose folders match the most wins, every time.
        let text = doc(
            "Input:3:/proj/appendix/intro.tex\nInput:4:/proj/chapters/intro.tex\n\
             Input:5:/proj/intro.tex\n",
            "{1\n(3,1:0,655360:655360,65536,0\n}1\n{2\n(4,1:0,655360:655360,65536,0\n}2\n\
             {3\n(5,1:0,655360:655360,65536,0\n}3\n",
        );
        for _ in 0..50 {
            // A new map each time: a new iteration order.
            let st = SyncTex::parse(&text, Path::new("/")).unwrap();
            let page = |p: &str| st.forward(Path::new(p), 1).unwrap().page;
            assert_eq!(page("/mnt/proj/chapters/intro.tex"), 1);
            assert_eq!(page("/mnt/proj/appendix/intro.tex"), 0);
            // No folder in common: the lowest tag.
            assert_eq!(page("/x/intro.tex"), 0);
        }
    }

    #[test]
    fn forward_before_the_first_record_and_after_the_last() {
        let st = SyncTex::parse(SAMPLE, Path::new("/proj")).unwrap();
        // Nothing at or after line 50: the closest line before (20).
        assert_eq!(st.forward(Path::new("/proj/main.tex"), 50).unwrap().page, 1);
        // Line 0 is the first line with records (10, on page 1).
        assert_eq!(st.forward(Path::new("/proj/main.tex"), 0).unwrap().page, 0);
    }

    #[test]
    fn forward_with_only_point_records_gives_a_marker() {
        let content = "{1\ng1,7:655360,1310720\nk1,7:0,0\n}1\n";
        let st = SyncTex::parse(&doc("", content), Path::new("/")).unwrap();
        let loc = st.forward(Path::new("/proj/main.tex"), 7).unwrap();
        assert_eq!(loc.rects.len(), 1);
        let r = loc.rects[0];
        assert!((r.x - sp(655360.0)).abs() < 1e-3);
        assert!((r.y - (sp(1310720.0) - 8.0)).abs() < 1e-3);
        assert_eq!((r.width, r.height), (2.0, 10.0));
    }

    #[test]
    fn forward_merges_boxes_on_a_baseline() {
        let content = "{1\n(1,3:0,655360:655360,65536,0\n(1,3:1310720,655360:655360,65536,0\n\
                       (1,3:0,1966080:655360,65536,0\n(1,3:0,0:0,0,0\n}1\n";
        let st = SyncTex::parse(&doc("", content), Path::new("/")).unwrap();
        let loc = st.forward(Path::new("/proj/main.tex"), 3).unwrap();
        // Two lines; the first one spans both boxes; the empty box is skipped.
        assert_eq!(loc.rects.len(), 2, "{loc:?}");
        assert!((loc.rects[0].width - sp(1310720.0 + 655360.0)).abs() < 1e-3);
    }

    #[test]
    fn huge_line_numbers_saturate() {
        // 2^32 + 1 must not become line 1.
        let content = "{1\n(1,4294967297:0,655360:655360,65536,0\n}1\n\
                       {2\n(1,5:0,655360:655360,65536,0\n(1,-3:0,0:5,5,0\n}2\n";
        let st = SyncTex::parse(&doc("", content), Path::new("/")).unwrap();
        assert_eq!(st.pages[0][0].line, u32::MAX);
        assert_eq!(st.forward(Path::new("/proj/main.tex"), 1).unwrap().page, 1);
        // Negative ones are 0, which forward and inverse skip.
        assert_eq!(st.pages[1][1].line, 0);
    }

    #[test]
    fn non_finite_numbers_are_rejected() {
        let content = "{1\n(1,3:-nan,655360:655360,65536,0\n(1,4:inf,0:1,1,0\n\
                       (1,5:0,655360:nan,inf,-inf\n(1,6:0,1e40:1,1,0\n}1\n";
        let st = SyncTex::parse(&doc("", content), Path::new("/")).unwrap();
        let finite = |r: &Rect| [r.x, r.y, r.width, r.height].iter().all(|v| v.is_finite());
        assert!(
            st.pages[0].iter().all(|r| finite(&r.rect)),
            "{:?}",
            st.pages[0]
        );
        for line in 3..=6 {
            if let Some(loc) = st.forward(Path::new("/proj/main.tex"), line) {
                assert!(loc.rects.iter().all(finite), "{line}: {loc:?}");
            }
        }
    }

    #[test]
    fn negative_widths_are_flipped() {
        let content = "{1\n(1,3:655360,655360:-327680,65536,0\n}1\n";
        let st = SyncTex::parse(&doc("", content), Path::new("/")).unwrap();
        let r = st.forward(Path::new("/proj/main.tex"), 3).unwrap().rects[0];
        assert!((r.x - sp(327680.0)).abs() < 1e-3, "{r:?}");
        assert!((r.width - sp(327680.0)).abs() < 1e-3, "{r:?}");
    }

    #[test]
    fn inverse_falls_back_to_the_nearest_record() {
        let st = SyncTex::parse(SAMPLE, Path::new("/proj")).unwrap();
        // Far below everything on page 1: the lowest line box (chap.tex).
        let loc = st.inverse(0, sp(4736286.0), 2000.0).unwrap();
        assert_eq!(loc.file, PathBuf::from("/proj/chap.tex"));
        assert_eq!(st.inverse(5, 0.0, 0.0), None);
        // A record of an unknown file.
        let content = "{1\n(9,1:0,655360:655360,65536,0\n}1\n";
        let st = SyncTex::parse(&doc("", content), Path::new("/")).unwrap();
        assert_eq!(st.inverse(0, 1.0, 5.0), None);
        // A page without records.
        let st = SyncTex::parse(&doc("", "{1\n}1\n"), Path::new("/")).unwrap();
        assert_eq!(st.inverse(0, 1.0, 5.0), None);
    }

    #[test]
    fn normalize_paths() {
        assert_eq!(
            normalize(Path::new("/p"), "./a/./b/../c.tex"),
            PathBuf::from("/p/a/c.tex")
        );
        assert_eq!(
            normalize(Path::new("/p/q"), "/abs.tex"),
            PathBuf::from("/abs.tex")
        );
        assert_eq!(
            normalize(Path::new("/"), "../../x.tex"),
            PathBuf::from("/x.tex")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_inputs() {
        let text = "SyncTeX Version:1\nInput:1:C:\\p\\.\\main.tex\nContent:\n\
                    {1\n(1,4:0,655360:655360,65536,0\n}1\n";
        let st = SyncTex::parse(text, Path::new("D:\\build")).unwrap();
        assert_eq!(st.inputs[&1], PathBuf::from("C:\\p\\main.tex"));
        assert_eq!(st.forward(Path::new("C:/p/main.tex"), 4).unwrap().page, 0);
        let back = st.inverse(0, 1.0, 9.0).unwrap();
        assert_eq!(back.file, PathBuf::from("C:\\p\\main.tex"));
    }

    mod props {
        use super::*;
        use proptest::prelude::*;

        fn rect() -> impl Strategy<Value = Rect> {
            (-1e4f32..1e4, -1e4f32..1e4, 0f32..1e3, 0f32..1e3).prop_map(|(x, y, width, height)| {
                Rect {
                    x,
                    y,
                    width,
                    height,
                }
            })
        }

        proptest! {
            #[test]
            fn union_contains_both(a in rect(), b in rect()) {
                let u = a.union(&b);
                // `x + width` may round below the far edge by an ulp or so.
                let eps = 1e-2;
                for r in [a, b] {
                    prop_assert!(u.contains(r.x, r.y));
                    prop_assert!(u.x + u.width + eps >= r.x + r.width);
                    prop_assert!(u.y + u.height + eps >= r.y + r.height);
                }
            }

            #[test]
            fn inside_is_at_distance_zero(r in rect(), fx in 0f32..=1.0, fy in 0f32..=1.0) {
                let (x, y) = (r.x + r.width * fx, r.y + r.height * fy);
                if r.contains(x, y) {
                    prop_assert_eq!(r.distance2(x, y), 0.0);
                }
            }

            #[test]
            fn distance_is_never_negative(r in rect(), x in -2e4f32..2e4, y in -2e4f32..2e4) {
                let d = r.distance2(x, y);
                prop_assert!(d >= 0.0);
                prop_assert_eq!(d == 0.0, r.contains(x, y));
            }

            #[test]
            fn normalize_is_idempotent(path in "(/?)((\\.|\\.\\.|[a-z]{1,3})/){0,6}[a-z]{1,3}\\.tex") {
                // Absolute on every platform.
                let base = if cfg!(windows) { "C:\\base\\dir" } else { "/base/dir" };
                let once = normalize(Path::new(base), &path);
                prop_assert!(once.is_absolute());
                prop_assert!(once.components().all(|c| !matches!(c, Component::CurDir | Component::ParentDir)));
                prop_assert_eq!(normalize(Path::new("/"), &once.to_string_lossy()), once.clone());
            }

            #[test]
            fn parse_never_panics(body in "\\PC*") {
                let text = format!("SyncTeX Version:1\n{body}");
                let st = SyncTex::parse(&text, Path::new("/")).unwrap();
                let _ = st.forward(Path::new("/a.tex"), 1);
                let _ = st.inverse(0, 1.0, 1.0);
            }

            #[test]
            fn parse_never_panics_on_records(
                lines in prop::collection::vec(
                    prop_oneof![
                        Just("{1".to_string()),
                        Just("}1".to_string()),
                        Just("Input:1:/a.tex".to_string()),
                        "[({hvkg$x\\[][0-9,:-]{0,30}",
                    ],
                    0..60,
                )
            ) {
                let text = format!("SyncTeX Version:1\nContent:\n{}", lines.join("\n"));
                let st = SyncTex::parse(&text, Path::new("/")).unwrap();
                for line in 0..5 {
                    if let Some(loc) = st.forward(Path::new("/a.tex"), line) {
                        prop_assert!(loc.page < st.page_count());
                        prop_assert!(!loc.rects.is_empty());
                    }
                }
            }

            /// One line box per source line, a few per page, without overlaps:
            /// forward then inverse gives the line back.
            #[test]
            fn forward_inverse_round_trip(per_page in prop::collection::vec(1usize..6, 1..5)) {
                let mut content = String::new();
                let mut line = 0;
                for (p, n) in per_page.iter().enumerate() {
                    content += &format!("{{{}\n", p + 1);
                    for i in 0..*n {
                        line += 1;
                        let y = (i as i64 + 1) * 20 * 65536;
                        content += &format!("(1,{line}:655360,{y}:6553600,655360,131072\n)\n");
                    }
                    content += &format!("}}{}\n", p + 1);
                }
                let st = SyncTex::parse(&doc("", &content), Path::new("/")).unwrap();
                prop_assert_eq!(st.page_count(), per_page.len());
                for l in 1..=line {
                    let loc = st.forward(Path::new("/proj/main.tex"), l).unwrap();
                    prop_assert_eq!(loc.rects.len(), 1);
                    let r = loc.rects[0];
                    let back = st.inverse(loc.page, r.x + r.width / 2.0, r.y + r.height / 2.0).unwrap();
                    prop_assert_eq!(back.line, l);
                    prop_assert_eq!(back.file, PathBuf::from("/proj/main.tex"));
                }
            }
        }
    }
}
