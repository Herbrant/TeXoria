//! The languages TeXoria can check: one per Hunspell dictionary of
//! [wooorm/dictionaries](https://github.com/wooorm/dictionaries), keyed by its BCP-47 code.

use std::sync::LazyLock;

use whatlang::Lang as W;

pub struct Language {
    /// BCP-47 code, also the dictionary's directory in wooorm/dictionaries.
    pub code: &'static str,
    /// Native name, shown in the language picker.
    pub name: &'static str,
    /// What the text detector calls this language. Regional variants share it.
    pub detect: Option<W>,
    /// babel/polyglossia names (and polyglossia `variant=` values).
    pub aliases: &'static [&'static str],
}

macro_rules! langs {
    ($($code:literal, $name:literal, $detect:expr, [$($alias:literal),*];)*) => {
        pub static LANGUAGES: &[Language] = &[
            $(Language { code: $code, name: $name, detect: $detect, aliases: &[$($alias),*] },)*
        ];
    };
}

langs! {
    "bg", "Български", Some(W::Bul), ["bulgarian"];
    "br", "Brezhoneg", None, ["breton"];
    "ca", "Català", Some(W::Cat), ["catalan"];
    "ca-valencia", "Català (valencià)", None, ["valencian", "valencia"];
    "cs", "Čeština", Some(W::Ces), ["czech"];
    "cy", "Cymraeg", Some(W::Cym), ["welsh"];
    "da", "Dansk", Some(W::Dan), ["danish"];
    "de", "Deutsch", Some(W::Deu), ["german", "ngerman", "germanb"];
    "de-AT", "Deutsch (Österreich)", Some(W::Deu), ["austrian", "naustrian"];
    "de-CH", "Deutsch (Schweiz)", Some(W::Deu), ["swissgerman", "nswissgerman"];
    "el", "Ελληνικά", Some(W::Ell), ["greek"];
    "el-polyton", "Ελληνικά (πολυτονικά)", None, ["polutonikogreek", "polytonicgreek"];
    "en", "English (US)", Some(W::Eng), ["english", "american", "usenglish"];
    "en-AU", "English (Australia)", Some(W::Eng), ["australian"];
    "en-CA", "English (Canada)", Some(W::Eng), ["canadian"];
    "en-GB", "English (UK)", Some(W::Eng), ["british", "ukenglish", "newzealand"];
    "en-ZA", "English (South Africa)", Some(W::Eng), [];
    "eo", "Esperanto", Some(W::Epo), ["esperanto"];
    "es", "Español", Some(W::Spa), ["spanish", "spain"];
    "es-AR", "Español (Argentina)", Some(W::Spa), ["argentina"];
    "es-BO", "Español (Bolivia)", Some(W::Spa), ["bolivia"];
    "es-CL", "Español (Chile)", Some(W::Spa), ["chile"];
    "es-CO", "Español (Colombia)", Some(W::Spa), ["colombia"];
    "es-CR", "Español (Costa Rica)", Some(W::Spa), ["costarica"];
    "es-CU", "Español (Cuba)", Some(W::Spa), ["cuba"];
    "es-DO", "Español (República Dominicana)", Some(W::Spa), [];
    "es-EC", "Español (Ecuador)", Some(W::Spa), ["ecuador"];
    "es-GT", "Español (Guatemala)", Some(W::Spa), ["guatemala"];
    "es-HN", "Español (Honduras)", Some(W::Spa), ["honduras"];
    "es-MX", "Español (México)", Some(W::Spa), ["mexican", "mexico"];
    "es-NI", "Español (Nicaragua)", Some(W::Spa), ["nicaragua"];
    "es-PA", "Español (Panamá)", Some(W::Spa), ["panama"];
    "es-PE", "Español (Perú)", Some(W::Spa), ["peru"];
    "es-PH", "Español (Filipinas)", Some(W::Spa), [];
    "es-PR", "Español (Puerto Rico)", Some(W::Spa), [];
    "es-PY", "Español (Paraguay)", Some(W::Spa), ["paraguay"];
    "es-SV", "Español (El Salvador)", Some(W::Spa), [];
    "es-US", "Español (Estados Unidos)", Some(W::Spa), [];
    "es-UY", "Español (Uruguay)", Some(W::Spa), ["uruguay"];
    "es-VE", "Español (Venezuela)", Some(W::Spa), ["venezuela"];
    "et", "Eesti", Some(W::Est), ["estonian"];
    "eu", "Euskara", None, ["basque"];
    "fa", "فارسی", Some(W::Pes), ["persian", "farsi"];
    "fo", "Føroyskt", None, ["faroese"];
    "fr", "Français", Some(W::Fra), ["french", "francais", "frenchb", "acadian", "canadien"];
    "fur", "Furlan", None, ["friulan", "friulian"];
    "fy", "Frysk", None, ["frisian", "westfrisian"];
    "ga", "Gaeilge", None, ["irish"];
    "gd", "Gàidhlig", None, ["scottish", "scottishgaelic", "gaelic"];
    "gl", "Galego", None, ["galician"];
    "he", "עברית", Some(W::Heb), ["hebrew"];
    "hr", "Hrvatski", Some(W::Hrv), ["croatian"];
    "hu", "Magyar", Some(W::Hun), ["hungarian", "magyar"];
    "hy", "Հայերեն", Some(W::Hye), ["armenian"];
    "hyw", "Արեւմտահայերէն", None, ["westernarmenian"];
    "ia", "Interlingua", None, ["interlingua"];
    "ie", "Interlingue", None, ["interlingue", "occidental"];
    "is", "Íslenska", None, ["icelandic"];
    "it", "Italiano", Some(W::Ita), ["italian"];
    "ka", "ქართული", Some(W::Kat), ["georgian"];
    "ko", "한국어", Some(W::Kor), ["korean"];
    "la", "Latina", Some(W::Lat), ["latin", "classiclatin", "medievallatin", "ecclesiasticlatin"];
    "lb", "Lëtzebuergesch", None, ["luxembourgish", "luxemburgish"];
    "lt", "Lietuvių", Some(W::Lit), ["lithuanian"];
    "ltg", "Latgalīšu", None, ["latgalian"];
    "lv", "Latviešu", Some(W::Lav), ["latvian"];
    "mk", "Македонски", Some(W::Mkd), ["macedonian"];
    "mn", "Монгол", None, ["mongolian"];
    "nb", "Norsk bokmål", Some(W::Nob), ["norsk", "norwegian", "bokmal"];
    "nds", "Plattdüütsch", None, ["lowgerman"];
    "ne", "नेपाली", Some(W::Nep), ["nepali"];
    "nl", "Nederlands", Some(W::Nld), ["dutch", "flemish"];
    "nn", "Norsk nynorsk", None, ["nynorsk"];
    "oc", "Occitan", None, ["occitan"];
    "pl", "Polski", Some(W::Pol), ["polish"];
    "pt", "Português (Brasil)", Some(W::Por), ["brazil", "brazilian"];
    "pt-PT", "Português (Portugal)", Some(W::Por), ["portuguese", "portuges"];
    "ro", "Română", Some(W::Ron), ["romanian"];
    "ru", "Русский", Some(W::Rus), ["russian"];
    "rw", "Kinyarwanda", None, ["kinyarwanda"];
    "sk", "Slovenčina", Some(W::Slk), ["slovak"];
    "sl", "Slovenščina", Some(W::Slv), ["slovene", "slovenian"];
    "sr", "Српски", Some(W::Srp), ["serbianc"];
    "sr-Latn", "Srpski (latinica)", None, ["serbian"];
    "sv", "Svenska", Some(W::Swe), ["swedish"];
    "sv-FI", "Svenska (Finland)", Some(W::Swe), [];
    "tk", "Türkmençe", Some(W::Tuk), ["turkmen"];
    "tlh", "Klingon (pIqaD)", None, [];
    "tlh-Latn", "tlhIngan Hol", None, ["klingon"];
    "tr", "Türkçe", Some(W::Tur), ["turkish"];
    "uk", "Українська", Some(W::Ukr), ["ukrainian", "ukraineb"];
    "vi", "Tiếng Việt", Some(W::Vie), ["vietnamese", "vietnam"];
}

/// SHA-256 of `index.aff` and `index.dic` at the pinned commit, generated by
/// `scripts/gen-dictionaries.sh`.
const HASHES: &str = include_str!("../data/dictionaries.tsv");

static PINNED: LazyLock<Vec<(&'static str, &'static str, &'static str)>> = LazyLock::new(|| {
    HASHES
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let mut f = l.split('\t');
            Some((f.next()?, f.next()?, f.next()?))
        })
        .collect()
});

pub fn get(code: &str) -> Option<&'static Language> {
    LANGUAGES.iter().find(|l| l.code == code)
}

pub fn name(code: &str) -> &str {
    get(code).map_or(code, |l| l.name)
}

/// Pinned SHA-256 of `(index.aff, index.dic)`.
pub fn hashes(code: &str) -> Option<(&'static str, &'static str)> {
    PINNED
        .iter()
        .find(|(c, ..)| *c == code)
        .map(|&(_, aff, dic)| (aff, dic))
}

/// The language a babel/polyglossia name stands for.
pub fn alias(name: &str) -> Option<&'static str> {
    let lower = name.trim().to_ascii_lowercase();
    LANGUAGES
        .iter()
        .find(|l| l.aliases.contains(&lower.as_str()))
        .map(|l| l.code)
}

/// Resolve a language as written by a user: a code (`it`, `en-GB`), a locale
/// (`it_IT`, `en_GB.UTF-8`) or a babel/polyglossia name (`ngerman`).
pub fn resolve(name: &str) -> Option<&'static str> {
    let name = name.trim();
    let name = name.split('.').next().unwrap_or(name);
    if name.is_empty() {
        return None;
    }
    if let Some(code) = alias(name) {
        return Some(code);
    }
    // Locales: `xx`, `xx_YY`, `xx-YY`, `sr_Latn_RS`.
    let mut parts = name.split(['_', '-']);
    let base = parts.next()?.to_ascii_lowercase();
    let rest: Vec<&str> = parts.collect();
    let by_code = |code: &str| {
        LANGUAGES
            .iter()
            .find(|l| l.code.eq_ignore_ascii_case(code))
            .map(|l| l.code)
    };
    for part in &rest {
        if let Some(c) = by_code(&format!("{base}-{part}")) {
            return Some(c);
        }
    }
    by_code(&base)
}

/// The dictionary for a detected language: the variant for `region` (an ISO
/// 3166 code, e.g. from the system locale) if there is one, else the main one.
pub fn for_detected(lang: W, region: Option<&str>) -> Option<&'static str> {
    let variants: Vec<_> = LANGUAGES
        .iter()
        .filter(|l| l.detect == Some(lang))
        .collect();
    region
        .and_then(|r| {
            variants.iter().find(|l| {
                l.code
                    .split_once('-')
                    .is_some_and(|(_, v)| v.eq_ignore_ascii_case(r))
            })
        })
        .or_else(|| variants.iter().find(|l| !l.code.contains('-')))
        .or(variants.first())
        .map(|l| l.code)
}

/// Languages the text detector may answer with.
pub fn detectable() -> Vec<W> {
    let mut out: Vec<W> = LANGUAGES.iter().filter_map(|l| l.detect).collect();
    out.sort_by_key(|l| l.code());
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn resolving_never_panics(text in "\\PC{0,16}", region in "\\PC{0,3}") {
            if let Some(code) = resolve(&text) {
                prop_assert!(get(code).is_some());
            }
            if let Some(code) = alias(&text) {
                prop_assert_eq!(resolve(&text), Some(code));
            }
            prop_assert_eq!(name(&text), get(&text).map_or(text.as_str(), |l| l.name));
            for lang in detectable() {
                let code = for_detected(lang, Some(&region)).unwrap();
                prop_assert_eq!(get(code).unwrap().detect, Some(lang));
            }
        }
    }

    #[test]
    fn names_and_detectable_languages() {
        assert_eq!(name("it"), "Italiano");
        assert_eq!(name("xx-unknown"), "xx-unknown");
        assert_eq!(hashes("xx"), None);
        let langs = detectable();
        assert!(langs.contains(&W::Eng) && langs.contains(&W::Ita));
        let mut sorted = langs.clone();
        sorted.dedup();
        assert_eq!(sorted, langs, "no duplicates");
        assert!(langs.windows(2).all(|w| w[0].code() < w[1].code()));
    }

    #[test]
    fn every_language_is_pinned_and_vice_versa() {
        for l in LANGUAGES {
            assert!(hashes(l.code).is_some(), "{} has no pinned hash", l.code);
        }
        for (code, aff, dic) in PINNED.iter() {
            assert!(get(code).is_some(), "{code} is not in the catalog");
            assert_eq!(aff.len(), 64);
            assert_eq!(dic.len(), 64);
        }
    }

    #[test]
    fn aliases_are_unique_and_lowercase() {
        let mut seen = std::collections::HashSet::new();
        for l in LANGUAGES {
            for a in l.aliases {
                assert_eq!(*a, a.to_ascii_lowercase());
                assert!(seen.insert(*a), "duplicate alias {a}");
            }
        }
    }

    #[test]
    fn resolves_names_and_locales() {
        assert_eq!(resolve("it"), Some("it"));
        assert_eq!(resolve("it_IT"), Some("it"));
        assert_eq!(resolve("en_GB"), Some("en-GB"));
        assert_eq!(resolve("en-US"), Some("en"));
        assert_eq!(resolve("en_US.UTF-8"), Some("en"));
        assert_eq!(resolve("de_CH"), Some("de-CH"));
        assert_eq!(resolve("ngerman"), Some("de"));
        assert_eq!(resolve("British"), Some("en-GB"));
        assert_eq!(resolve("brazil"), Some("pt"));
        assert_eq!(resolve("pt_PT"), Some("pt-PT"));
        assert_eq!(resolve("sr_Latn_RS"), Some("sr-Latn"));
        assert_eq!(resolve("a4paper"), None);
        assert_eq!(resolve(""), None);
    }

    #[test]
    fn picks_the_regional_variant() {
        assert_eq!(for_detected(W::Eng, None), Some("en"));
        assert_eq!(for_detected(W::Eng, Some("GB")), Some("en-GB"));
        assert_eq!(for_detected(W::Eng, Some("IT")), Some("en"));
        assert_eq!(for_detected(W::Por, Some("PT")), Some("pt-PT"));
        assert_eq!(for_detected(W::Ita, Some("CH")), Some("it"));
        assert_eq!(for_detected(W::Jpn, None), None);
    }
}
