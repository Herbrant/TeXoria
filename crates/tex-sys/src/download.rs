//! Downloads checked against the SHA-256 they must have.

use std::fs::File;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

/// What TeXoria tells the web servers it talks to (Crossref asks clients to say who they are).
pub const USER_AGENT: &str = concat!(
    "TeXoria/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/Herbrant/TeXoria)"
);

/// Copy `reader` into `out` and flush it to disk, failing unless what was read has the
/// SHA-256 `expected` (in lowercase hexadecimal), or once `cancel` is set. `progress` gets the
/// number of bytes copied so far.
pub fn copy_verified(
    mut reader: impl Read,
    mut out: &File,
    expected: &str,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> io::Result<()> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 << 10];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::other("canceled"));
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])?;
        done += n as u64;
        progress(done);
    }
    out.sync_all()?;
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        return Err(io::Error::other(format!(
            "SHA-256 mismatch: expected {expected}, got {actual}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "abc"'s SHA-256.
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn copies_what_has_the_expected_hash() {
        let file = tempfile::tempfile().unwrap();
        let mut seen = Vec::new();
        copy_verified(&b"abc"[..], &file, ABC, &AtomicBool::new(false), |n| {
            seen.push(n)
        })
        .unwrap();
        assert_eq!(seen, [3]);
        let mut copied = String::new();
        let mut file = file;
        std::io::Seek::rewind(&mut file).unwrap();
        file.read_to_string(&mut copied).unwrap();
        assert_eq!(copied, "abc");
    }

    #[test]
    fn refuses_another_hash_or_a_canceled_copy() {
        let file = tempfile::tempfile().unwrap();
        let err = copy_verified(&b"abd"[..], &file, ABC, &AtomicBool::new(false), |_| {});
        let err = err.unwrap_err().to_string();
        assert!(
            err.starts_with(&format!("SHA-256 mismatch: expected {ABC}, got ")),
            "{err}"
        );
        let err = copy_verified(&b"abc"[..], &file, ABC, &AtomicBool::new(true), |_| {});
        assert_eq!(err.unwrap_err().to_string(), "canceled");
    }

    #[test]
    fn says_who_it_is() {
        assert!(USER_AGENT.starts_with(concat!("TeXoria/", env!("CARGO_PKG_VERSION"), " (")));
    }
}
