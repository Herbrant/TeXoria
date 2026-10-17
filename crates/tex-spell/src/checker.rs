//! Checking words against a Hunspell dictionary.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Mutex;

use anyhow::{Result, anyhow};
use spellbook::{Dictionary, ParseDictionaryErrorSource};

use crate::repair;

/// Lines of a broken dictionary skipped before giving up on it.
const MAX_REPAIRS: usize = 300;
/// Forget cached verdicts past this many words.
const CACHE_LIMIT: usize = 100_000;

pub struct Speller {
    dict: Dictionary,
    /// Documents repeat their words: remember each verdict.
    cache: Mutex<HashMap<String, bool>>,
}

impl Speller {
    pub fn new(aff: &str, dic: &str) -> Result<Self> {
        let (mut aff, mut dic) = repair::prepare(aff, dic);
        // Hunspell skips the lines it cannot read; spellbook refuses the file.
        let mut retries = 0;
        let dict = loop {
            let e = match Dictionary::new(&aff, &dic) {
                Ok(dict) => break dict,
                Err(e) => e,
            };
            let Some(line) = e.line_number.filter(|_| retries < MAX_REPAIRS) else {
                return Err(anyhow!("{e}"));
            };
            log::debug!("skipping a dictionary line: {e}");
            retries += 1;
            match e.source {
                ParseDictionaryErrorSource::Aff => aff = repair::drop_line(&aff, line, true),
                ParseDictionaryErrorSource::Dic => dic = repair::drop_line(&dic, line, false),
            }
        };
        Ok(Self {
            dict,
            cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn check(&self, word: &str) -> bool {
        if let Some(&ok) = self.cache.lock().unwrap().get(word) {
            return ok;
        }
        let ok = self.check_uncached(word);
        let mut cache = self.cache.lock().unwrap();
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(word.to_string(), ok);
        ok
    }

    fn check_uncached(&self, word: &str) -> bool {
        if self.dict.check(word) {
            return true;
        }
        let straight = word.replace('\u{2019}', "'");
        if straight != word && self.dict.check(&straight) {
            return true;
        }
        // Elision (`l'anno`, `dell'anno`, `qu'il`): the word after the
        // apostrophe is what matters.
        straight
            .rsplit_once('\'')
            .is_some_and(|(_, rest)| rest.chars().count() > 1 && self.dict.check(rest))
    }

    /// The ranges among `words` (byte ranges of `text`) that are misspelled.
    /// Ranges that are not a slice of `text` (from an older text) are skipped.
    pub fn misspelled(
        &self,
        text: &str,
        words: &[Range<usize>],
        personal: &HashSet<String>,
    ) -> Vec<Range<usize>> {
        words
            .iter()
            .filter(|r| {
                let Some(w) = text.get((*r).clone()) else {
                    return false;
                };
                !personal.contains(w) && !personal.contains(&w.to_lowercase()) && !self.check(w)
            })
            .cloned()
            .collect()
    }

    /// Up to `n` corrections for `word`. Can be slow (tens of milliseconds).
    pub fn suggest(&self, word: &str, n: usize) -> Vec<String> {
        let mut out = Vec::new();
        self.dict.suggest(word, &mut out);
        out.truncate(n);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AFF: &str = "SET UTF-8\nTRY aeiouhlnrtcdgmpsbfvz\n\nSFX S Y 1\nSFX S 0 s .\n";
    const DIC: &str = "5\nhello\nworld/S\nanno\nperché\ncittà\n";

    fn speller() -> Speller {
        Speller::new(AFF, DIC).unwrap()
    }

    #[test]
    fn checks_words() {
        let s = speller();
        assert!(s.check("hello"));
        assert!(s.check("Hello"));
        assert!(s.check("worlds"));
        assert!(s.check("città"));
        assert!(!s.check("helo"));
        // Cached verdicts stay the same.
        assert!(!s.check("helo"));
        assert!(s.check("l'anno"));
        assert!(s.check("dell’anno"));
        assert!(!s.check("l'ano"));
    }

    #[test]
    fn finds_misspelled_ranges() {
        let s = speller();
        let text = "hello wrld città Texoria";
        let words = vec![0..5, 6..10, 11..17, 18..25];
        let personal = HashSet::from(["texoria".to_string()]);
        assert_eq!(s.misspelled(text, &words, &personal), vec![6..10]);
    }

    #[test]
    fn skips_malformed_lines() {
        let aff = "SET UTF-8\nFLAG num\nSFX 10 Y 1\nSFX 10 0 s .\n";
        let dic = "3\nhello/10\n\"A/S\"\nworld/10,x1\ncitt\\/à\n";
        let s = Speller::new(aff, dic).unwrap();
        assert!(s.check("hellos"));
        // Hunspell reads `x1` as no flag.
        assert!(s.check("worlds"));

        let aff = "SET UTF-8\nFLAG long\nSFX zz Y 2\nSFX zz 0 s .\nSFX zz 0 [ .\n";
        let s = Speller::new(aff, "2\nhello/zz\nworld/m01 po:v\n").unwrap();
        assert!(s.check("hellos"));
        assert!(!s.check("world"));
    }

    #[test]
    fn reports_errors_it_cannot_repair() {
        // No line to drop: the count line is missing.
        let e = Speller::new(AFF, "").err().unwrap();
        assert!(e.to_string().contains(".dic"), "{e}");
        // A broken line per repair is fine up to the limit...
        let broken = |n: usize| format!("SET UTF-8\n{}", "KEEPCASE\n".repeat(n));
        assert!(Speller::new(&broken(MAX_REPAIRS), DIC).is_ok());
        // ...past it the dictionary is given up on.
        let e = Speller::new(&broken(MAX_REPAIRS + 1), DIC).err().unwrap();
        assert!(e.to_string().contains("line 2"), "{e}");
    }

    #[test]
    fn forgets_verdicts_past_the_limit() {
        let s = speller();
        for i in 0..CACHE_LIMIT {
            s.check(&format!("w{i}"));
        }
        assert_eq!(s.cache.lock().unwrap().len(), CACHE_LIMIT);
        assert!(s.check("hello"));
        assert_eq!(
            *s.cache.lock().unwrap(),
            HashMap::from([("hello".to_string(), true)])
        );
    }

    #[test]
    fn skips_ranges_outside_the_text() {
        let s = speller();
        let text = "città wrld";
        // Inside `à`, past the end, reversed: not words of this text.
        #[allow(clippy::reversed_empty_ranges)]
        let words = vec![0..6, 3..5, 7..11, 8..20, 9..8];
        assert_eq!(s.misspelled(text, &words, &HashSet::new()), vec![7..11]);
    }

    #[test]
    fn suggests_corrections() {
        let s = speller();
        assert!(s.suggest("helo", 5).contains(&"hello".to_string()));
        assert!(s.suggest("wrld", 1).len() <= 1);
    }
}
