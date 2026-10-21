//! Small helpers for the text TeXoria shows.

/// The `s` of a plural English noun, for `n` of them: `1 file`, `2 files`.
pub fn plural<N: PartialEq + From<u8>>(n: N) -> &'static str {
    if n == N::from(1) { "" } else { "s" }
}

/// `12,345`.
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plurals() {
        assert_eq!(format!("{} file{}", 1, plural(1)), "1 file");
        assert_eq!(format!("{} file{}", 0, plural(0)), "0 files");
        assert_eq!(format!("{} file{}", 2, plural(2)), "2 files");
        assert_eq!(plural(1i64), "");
    }
}
