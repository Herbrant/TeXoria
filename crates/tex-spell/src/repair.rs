//! Hunspell reads published dictionaries leniently (it skips lines it cannot
//! parse and reads numeric flags with `atoi`); spellbook refuses them. These
//! fixes make a few dictionaries of wooorm/dictionaries (and of distributions)
//! load as they do in Hunspell.

/// Numeric flags are `u16`s and spellbook rejects 0, which Hunspell accepts.
const ZERO_FLAG: &str = "65000";

#[derive(Clone, Copy, PartialEq)]
enum FlagType {
    Short,
    Long,
    Numeric,
}

fn flag_type(aff: &str) -> FlagType {
    let mut flags = FlagType::Short;
    for line in aff.lines() {
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next()) {
            (Some("FLAG"), Some("long")) => flags = FlagType::Long,
            (Some("FLAG"), Some("num")) => flags = FlagType::Numeric,
            _ => {}
        }
    }
    flags
}

/// Fix what can be fixed up front: numeric flags as Hunspell reads them
/// (`17X` is 17), and `.dic` lines with an odd number of long-flag chars.
pub fn prepare(aff: &str, dic: &str) -> (String, String) {
    match flag_type(aff) {
        FlagType::Short => (aff.to_string(), dic.to_string()),
        FlagType::Long => (
            aff.to_string(),
            map_dic(dic, |f| (f.chars().count() % 2 == 0).then(|| f.to_string())),
        ),
        FlagType::Numeric => (numeric_aff(aff), map_dic(dic, |f| Some(numeric_list(f)))),
    }
}

/// Apply `fix` to the flags of every `.dic` line (`word/flags morph`); a line
/// whose flags cannot be fixed is dropped.
fn map_dic(dic: &str, fix: impl Fn(&str) -> Option<String>) -> String {
    let mut lines = dic.lines();
    let mut out = String::with_capacity(dic.len());
    if let Some(count) = lines.next() {
        out.push_str(count);
        out.push('\n');
    }
    for line in lines {
        let Some(slash) = flag_slash(line) else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        let rest = &line[slash + 1..];
        let end = rest.find([' ', '\t']).unwrap_or(rest.len());
        if let Some(flags) = fix(&rest[..end]) {
            out.push_str(&line[..=slash]);
            out.push_str(&flags);
            out.push_str(&rest[end..]);
            out.push('\n');
        }
    }
    out
}

/// The `/` before a word's flags (`\/` is a slash in the word).
fn flag_slash(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    (1..bytes.len()).find(|&i| bytes[i] == b'/' && bytes[i - 1] != b'\\')
}

/// One numeric flag as Hunspell reads it: its leading digits.
fn numeric(flag: &str) -> Option<String> {
    let digits: String = flag
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    match digits.parse::<u16>().ok()? {
        0 => Some(ZERO_FLAG.to_string()),
        n => Some(n.to_string()),
    }
}

fn numeric_list(flags: &str) -> String {
    flags
        .split(',')
        .filter_map(numeric)
        .collect::<Vec<_>>()
        .join(",")
}

/// Directives whose argument is one flag.
const FLAG_DIRECTIVES: &[&str] = &[
    "NEEDAFFIX",
    "PSEUDOROOT",
    "FORBIDDENWORD",
    "KEEPCASE",
    "NOSUGGEST",
    "SUBSTANDARD",
    "CIRCUMFIX",
    "ONLYINCOMPOUND",
    "COMPOUNDFLAG",
    "COMPOUNDBEGIN",
    "COMPOUNDMIDDLE",
    "COMPOUNDLAST",
    "COMPOUNDEND",
    "COMPOUNDPERMITFLAG",
    "COMPOUNDFORBIDFLAG",
    "COMPOUNDROOT",
    "FORCEUCASE",
    "WARN",
    "LEMMA_PRESENT",
];

fn numeric_aff(aff: &str) -> String {
    let mut out = String::with_capacity(aff.len());
    for line in aff.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let fixed = match fields.as_slice() {
            [kw @ ("SFX" | "PFX"), flag, rest @ ..] => {
                let mut f = vec![kw.to_string(), numeric(flag).unwrap_or(flag.to_string())];
                for (i, field) in rest.iter().enumerate() {
                    // Rows: strip, affix[/flags], condition, morphology.
                    if i == 1
                        && let Some((affix, flags)) = field.split_once('/')
                    {
                        f.push(format!("{affix}/{}", numeric_list(flags)));
                    } else {
                        f.push(field.to_string());
                    }
                }
                Some(f.join(" "))
            }
            [kw, flag] if FLAG_DIRECTIVES.contains(kw) => {
                numeric(flag).map(|n| format!("{kw} {n}"))
            }
            _ => None,
        };
        out.push_str(fixed.as_deref().unwrap_or(line));
        out.push('\n');
    }
    out
}

/// Tables of the `.aff`: `KW count` (or `SFX flag Y count`) followed by
/// `count` rows.
const TABLES: &[&str] = &[
    "REP",
    "MAP",
    "BREAK",
    "ICONV",
    "OCONV",
    "PHONE",
    "AF",
    "AM",
    "COMPOUNDRULE",
    "CHECKCOMPOUNDPATTERN",
];

/// Where a table starts: its header line (0-based) and the index of its count.
fn header(line: &str) -> Option<usize> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    match fields.as_slice() {
        ["SFX" | "PFX", _, "Y" | "N", n, ..] if n.parse::<usize>().is_ok() => Some(3),
        [kw, n] if TABLES.contains(kw) && n.parse::<usize>().is_ok() => Some(1),
        _ => None,
    }
}

/// Drop the 1-based `line` of `text`. In an `.aff`, the table it belongs to
/// gets one row less, and a table header is dropped with its rows.
pub fn drop_line(text: &str, line: usize, is_aff: bool) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let Some(ix) = line.checked_sub(1).filter(|&i| i < lines.len()) else {
        return text.to_string();
    };
    if is_aff {
        if let Some(count_ix) = header(&lines[ix]) {
            let n: usize = lines[ix]
                .split_whitespace()
                .nth(count_ix)
                .unwrap()
                .parse()
                .unwrap();
            let end = (ix + 1 + n).min(lines.len());
            lines.drain(ix..end);
            return lines.join("\n");
        }
        // The nearest table above whose rows reach this line.
        if let Some((h, count_ix)) = (0..ix).rev().find_map(|h| {
            let c = header(&lines[h])?;
            let n: usize = lines[h].split_whitespace().nth(c)?.parse().ok()?;
            (h + n >= ix).then_some((h, c))
        }) {
            let mut fields: Vec<String> = lines[h].split_whitespace().map(str::to_string).collect();
            let n: usize = fields[count_ix].parse().unwrap_or(1);
            fields[count_ix] = n.saturating_sub(1).to_string();
            lines[h] = fields.join(" ");
        }
    }
    lines.remove(ix);
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_numeric_flags_like_hunspell() {
        let aff = "FLAG num\nSFX 0 N 1\nSFX 0 0 de/17X,3 .\nNEEDAFFIX 0\nKEEPCASE 12\n";
        let (aff, dic) = prepare(aff, "2\nev/0,3\nkitap/17X tr:x\n");
        assert_eq!(
            aff,
            "FLAG num\nSFX 65000 N 1\nSFX 65000 0 de/17,3 .\nNEEDAFFIX 65000\nKEEPCASE 12\n"
        );
        assert_eq!(dic, "2\nev/65000,3\nkitap/17 tr:x\n");
    }

    #[test]
    fn drops_odd_long_flags() {
        let (_, dic) = prepare("FLAG long\n", "2\nhello/zz\nworld/m01 po:v\na\\/b/zz\n");
        assert_eq!(dic, "2\nhello/zz\na\\/b/zz\n");
    }

    #[test]
    fn drops_table_rows_and_headers() {
        let aff = "SET UTF-8\nSFX k Y 3\nSFX k a b c\nSFK k typo\nSFX k d e f\nREP 1\nREP a b";
        assert_eq!(
            drop_line(aff, 4, true),
            "SET UTF-8\nSFX k Y 2\nSFX k a b c\nSFX k d e f\nREP 1\nREP a b"
        );
        assert_eq!(drop_line(aff, 2, true), "SET UTF-8\nREP 1\nREP a b");
        assert_eq!(drop_line(aff, 1, true), &aff[aff.find('\n').unwrap() + 1..]);
        assert_eq!(drop_line("1\nbad/x\n", 2, false), "1");
    }
}
