use std::path::PathBuf;

use gpui_kit::{TestAppContext, VisualTestContext, WindowHandle, base::Root};
use tex_spell::Source;

use super::*;
use crate::testing;
use crate::workspace::Workspace;
// `super::*` brings gpui's `test` attribute, which would shadow the standard one, and
// gpui-kit's `gpui` alias, which the `#[gpui::test]` macro's paths must not find twice.
use core::prelude::v1::test;
use gpui_kit::gpui;

fn job(text: &str) -> Job {
    Job {
        text: text.to_string(),
        chosen: None,
        previous: None,
        index: None,
        root: None,
        root_text: None,
        root_index: None,
        region: None,
        fallback: "en",
    }
}

const ITALIAN: &str = "Questo documento descrive il funzionamento del sistema e le scelte \
    progettuali che hanno guidato lo sviluppo del programma negli ultimi anni.";

#[test]
fn detects_and_keeps_the_language() {
    let a = analyze(job(ITALIAN));
    assert_eq!(a.language.code, "it");
    assert_eq!(a.language.source, Source::Text);
    // Every word but the single letter "e".
    assert_eq!(a.words.len(), 20);

    // Once recognized, short edits do not move it back to the fallback.
    let a = analyze(Job {
        previous: Some(a.language),
        ..job("Breve")
    });
    assert_eq!(a.language.code, "it");

    // A declaration wins over the text.
    let a = analyze(Job {
        previous: Some(a.language),
        ..job(&format!("% !TeX spellcheck = de_DE\n{ITALIAN}"))
    });
    assert_eq!((a.language.code, a.language.source), ("de", Source::Magic));
}

#[test]
fn honors_the_users_choice_and_the_root_file() {
    let a = analyze(Job {
        chosen: Some("fr"),
        ..job(ITALIAN)
    });
    assert_eq!((a.language.code, a.language.source), ("fr", Source::User));

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("main.tex");
    std::fs::write(&root, "% !TeX spellcheck = en_GB\n").unwrap();
    let a = analyze(Job {
        root: Some(root.clone()),
        ..job(ITALIAN)
    });
    assert_eq!(a.language.code, "en-GB");
    // The root's text in the editor wins over its file.
    let a = analyze(Job {
        root: Some(root),
        root_text: Some("% !TeX spellcheck = de_DE\n".into()),
        ..job(ITALIAN)
    });
    assert_eq!(a.language.code, "de");
}

#[test]
fn a_short_chapter_takes_the_documents_language() {
    // The sample's chapter: too little text to tell its language, on a machine set to
    // another one than the document's.
    let chapter = "\\section{Methods}\nWe work in $\\R^n$ with the usual norm.\n";
    let root =
        format!("\\documentclass{{article}}\n\\begin{{document}}\n{ITALIAN}\n\\end{{document}}\n");
    let a = analyze(Job {
        root_text: Some(root),
        fallback: "de",
        ..job(chapter)
    });
    assert_eq!((a.language.code, a.language.source), ("it", Source::Text));
    // Without a document to go by, the fallback.
    let a = analyze(Job {
        fallback: "de",
        ..job(chapter)
    });
    assert_eq!(
        (a.language.code, a.language.source),
        ("de", Source::Fallback)
    );
}

#[test]
fn regions_of_locales() {
    assert_eq!(region_of("en-GB").as_deref(), Some("GB"));
    assert_eq!(region_of("pt-br").as_deref(), Some("BR"));
    assert_eq!(region_of("zh-Hant-TW").as_deref(), Some("TW"));
    assert_eq!(region_of("sr_Latn_RS").as_deref(), Some("RS"));
    assert_eq!(region_of("es-419"), None);
    assert_eq!(region_of("it"), None);
    assert_eq!(region_of(""), None);
}

#[test]
fn underlines_are_wavy_and_in_order() {
    let color = gpui_kit::red();
    let u = underlines(vec![4..9, 0..2], color);
    assert_eq!(u.len(), 2);
    assert_eq!(u[0].range, 4..9);
    assert_eq!(u[1].range, 0..2);
    let style = u[0].style.underline.unwrap();
    assert!(style.wavy);
    assert_eq!(style.color, Some(color));
}

/// A tiny English dictionary.
const AFF: &str = "SET UTF-8\nTRY aeiouhlnrtcdgmpsbfvwz\n\nSFX S Y 1\nSFX S 0 s .\n";
const DIC: &str = "4\nhello\nworld/S\ndocument\ntext\n";

fn speller() -> Arc<Speller> {
    Arc::new(Speller::new(AFF, DIC).unwrap())
}

#[test]
fn offline_dictionaries_are_only_the_local_ones() {
    let system = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    std::fs::write(system.path().join("en_US.aff"), AFF).unwrap();
    std::fs::write(system.path().join("en_US.dic"), DIC).unwrap();
    let dicts = Dictionaries::offline(Store::with_dirs(
        cache.path(),
        vec![system.path().to_path_buf()],
    ));
    let en = dicts.load("en").unwrap();
    assert!(en.check("worlds") && !en.check("wrold"));
    // Never downloaded.
    let e = dicts.load("de").err().unwrap();
    assert!(e.to_string().contains("no de dictionary"), "{e:#}");
    assert!(std::fs::read_dir(cache.path()).unwrap().next().is_none());

    // The tests' own spelling never finds a dictionary of the machine.
    assert!(Spell::new(&[]).store.load("en").is_err());
}

#[test]
fn the_personal_dictionary_follows_the_settings() {
    let mut spell = Spell::new(&["TeXoria".into()]);
    assert!(spell.words.contains("TeXoria"));
    spell.set_words(&["a".into(), "b".into()]);
    assert_eq!(spell.words.len(), 2);
    assert!(!spell.words.contains("TeXoria"));
}

fn labels(entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .map(|e| match e {
            Entry::Item {
                label, disabled, ..
            } => format!("{label}{}", if *disabled { " (disabled)" } else { "" }),
            Entry::Separator => "-".into(),
        })
        .collect()
}

#[test]
fn context_menu_corrections() {
    let entries = corrections(3..8, "wrold".into(), vec!["world".into(), "would".into()]);
    assert_eq!(
        labels(&entries),
        ["world", "would", "-", "Add “wrold” to Dictionary", "-"]
    );
    let Entry::Item { action, .. } = &entries[1] else {
        panic!()
    };
    assert!(
        action.as_any().downcast_ref::<ReplaceMisspelling>()
            == Some(&ReplaceMisspelling {
                range: 3..8,
                text: "would".into()
            })
    );
    let Entry::Item { action, .. } = &entries[3] else {
        panic!()
    };
    assert!(
        action.as_any().downcast_ref::<AddToDictionary>()
            == Some(&AddToDictionary {
                word: "wrold".into()
            })
    );

    let entries = corrections(0..3, "qqq".into(), Vec::new());
    assert_eq!(
        labels(&entries),
        [
            "No Suggestions (disabled)",
            "-",
            "Add “qqq” to Dictionary",
            "-"
        ]
    );
}

const DOCUMENT: &str = "% !TeX spellcheck = en\n\\documentclass{article}\n\\begin{document}\n\
    Hello wrold, hello world.\n\\end{document}\n";

struct Open {
    _dir: tempfile::TempDir,
    main: PathBuf,
    window: WindowHandle<Root>,
    workspace: Entity<Workspace>,
}

impl Open {
    fn editor(&self, cx: &mut TestAppContext) -> Entity<EditorState> {
        self.workspace
            .read_with(cx, |ws, _| ws.buffer_parts(&self.main).unwrap().0.clone())
    }

    fn underlines(&self, cx: &mut TestAppContext) -> TextDecorationCollection {
        self.workspace.read_with(cx, |ws, _| {
            ws.buffer_parts(&self.main).unwrap().1.underlines.clone()
        })
    }

    fn misspelled(&self, cx: &mut TestAppContext) -> Vec<String> {
        let (editor, underlines) = (self.editor(cx), self.underlines(cx));
        cx.read(|cx| {
            let text = editor.read(cx).text().to_string();
            underlines
                .get_ranges(cx)
                .into_iter()
                .map(|r| text[r].to_string())
                .collect()
        })
    }

    fn text(&self, cx: &mut TestAppContext) -> String {
        let editor = self.editor(cx);
        cx.read(|cx| editor.read(cx).text().to_string())
    }

    /// Type `text` at the end of the document's body, and wait for the checks.
    fn type_text(&self, cx: &mut TestAppContext, text: &str) {
        let editor = self.editor(cx);
        cx.update_window(self.window.into(), |_, window, cx| {
            editor.update(cx, |state, cx| {
                let end = state.text().to_string().find("\\end{document}").unwrap();
                state.set_selected_range(end..end, cx);
                state.replace(text, window, cx);
            })
        })
        .unwrap();
        settle(cx);
    }

    fn update(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>),
    ) {
        let workspace = self.workspace.clone();
        cx.update_window(self.window.into(), |_, window, cx| {
            workspace.update(cx, |ws, cx| f(ws, window, cx))
        })
        .unwrap();
        settle(cx);
    }
}

/// Past every delay (live preview, spelling), and the background work done.
fn settle(cx: &mut TestAppContext) {
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
}

/// A project whose English dictionary is the tiny one above.
fn open(cx: &mut TestAppContext) -> Open {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.tex"), DOCUMENT).unwrap();
    let main = crate::paths::canonical(&dir.path().join("main.tex"));
    let (window, workspace) = testing::open(cx, Some(main.clone()), testing::startup());
    let open = Open {
        _dir: dir,
        main,
        window,
        workspace,
    };
    // Opening found no dictionary (tests have none).
    assert!(open.misspelled(cx).is_empty());
    open.update(cx, |ws, _, _| {
        let spell = ws.spell_mut();
        spell.region = None;
        spell.fallback = "en";
        spell.dictionaries.clear();
        spell
            .dictionaries
            .insert("en", Dictionary::Ready(speller()));
    });
    // Any edit checks the buffer again.
    open.type_text(cx, "\n");
    open
}

#[gpui::test]
fn underlines_misspelled_words(cx: &mut TestAppContext) {
    let open = open(cx);
    assert_eq!(open.misspelled(cx), ["wrold"]);
    open.type_text(cx, "Anothr text.\n");
    assert_eq!(open.misspelled(cx), ["wrold", "Anothr"]);

    // Off, then on again.
    open.update(cx, |ws, window, cx| {
        ws.update_settings(|s| s.spell_check = false, window, cx)
    });
    assert!(open.misspelled(cx).is_empty());
    open.update(cx, |ws, window, cx| {
        ws.update_settings(|s| s.spell_check = true, window, cx)
    });
    assert_eq!(open.misspelled(cx), ["wrold", "Anothr"]);

    // Not checked at all in this file.
    let main = open.main.clone();
    open.update(cx, |ws, window, cx| {
        ws.update_settings(
            |s| s.set_spell_language(&main, Some(crate::settings::SpellLanguage::OFF.into())),
            window,
            cx,
        )
    });
    assert!(open.misspelled(cx).is_empty());
}

/// The code actions of the misspelling at `offset`.
fn code_actions(open: &Open, cx: &mut TestAppContext, offset: usize) -> Vec<CodeAction> {
    let editor = open.editor(cx);
    let (underlines, speller) = open.workspace.read_with(cx, |ws, _| {
        let spell = ws.buffer_parts(&open.main).unwrap().1;
        (spell.underlines.clone(), spell.speller.clone())
    });
    let provider = SpellActions {
        underlines,
        speller,
    };
    assert_eq!(provider.id(), "spelling");
    let task = cx
        .update_window(open.window.into(), |_, window, cx| {
            provider.code_actions(editor, offset..offset, window, cx)
        })
        .unwrap();
    cx.foreground_executor().block_test(task).unwrap()
}

fn perform(open: &Open, cx: &mut TestAppContext, action: CodeAction) {
    let editor = open.editor(cx);
    let (underlines, speller) = open.workspace.read_with(cx, |ws, _| {
        let spell = ws.buffer_parts(&open.main).unwrap().1;
        (spell.underlines.clone(), spell.speller.clone())
    });
    let provider = SpellActions {
        underlines,
        speller,
    };
    let task = cx
        .update_window(open.window.into(), |_, window, cx| {
            provider.perform_code_action(editor, action, true, window, cx)
        })
        .unwrap();
    cx.foreground_executor().block_test(task).unwrap();
    settle(cx);
}

#[gpui::test]
fn corrects_through_code_actions(cx: &mut TestAppContext) {
    let open = open(cx);
    let at = DOCUMENT.find("wrold").unwrap() + 2;
    let actions = code_actions(&open, cx, at);
    let titles: Vec<&str> = actions.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(titles.first(), Some(&"Change to “world”"), "{titles:?}");
    assert_eq!(titles.last(), Some(&"Add “wrold” to Dictionary"));
    assert_eq!(actions[0].kind, Some(CodeActionKind::QUICKFIX));
    // Nothing on a word spelled right.
    assert!(code_actions(&open, cx, DOCUMENT.find("Hello").unwrap()).is_empty());

    perform(&open, cx, actions[0].clone());
    assert!(open.text(cx).contains("Hello world, hello world."));
    assert!(open.misspelled(cx).is_empty());

    // An action without what it needs does nothing.
    perform(&open, cx, CodeAction::default());
    let stale = CodeAction {
        data: Some(json!({ "start": 1, "end": 100_000, "text": "x" })),
        ..Default::default()
    };
    perform(&open, cx, stale);
    assert!(open.text(cx).contains("Hello world, hello world."));
}

#[gpui::test]
fn adds_words_to_the_personal_dictionary(cx: &mut TestAppContext) {
    let open = open(cx);
    let at = DOCUMENT.find("wrold").unwrap();
    let add = code_actions(&open, cx, at).pop().unwrap();
    perform(&open, cx, add);
    open.workspace.read_with(cx, |ws, _| {
        assert_eq!(ws.settings.spell_words, ["wrold"]);
    });
    assert!(open.misspelled(cx).is_empty());
    // Without a dictionary there is nothing to suggest.
    open.type_text(cx, "Anothr\n");
    open.update(cx, |ws, _, _| {
        *ws.buffer_parts(&open.main).unwrap().1.speller.borrow_mut() = None;
    });
    let at = open.text(cx).find("Anothr").unwrap();
    assert!(code_actions(&open, cx, at).is_empty());
}

#[gpui::test]
fn context_menu_on_a_misspelling(cx: &mut TestAppContext) {
    let open = open(cx);
    let editor = open.editor(cx);
    let (underlines, speller) = open.workspace.read_with(cx, |ws, _| {
        let spell = ws.buffer_parts(&open.main).unwrap().1;
        (spell.underlines.clone(), spell.speller.clone())
    });
    let menu = context_menu(editor.clone(), underlines, speller.clone());
    let mut cx = VisualTestContext::from_window(open.window.into(), cx);
    for offset in [DOCUMENT.find("wrold").unwrap() + 1, 0] {
        let built = cx.update(|window, cx| {
            editor.update(cx, |e, cx| e.set_selected_range(offset..offset, cx));
            menu(NativeMenu::new(), window, cx)
        });
        assert!(!built.is_empty());
    }
    // Without its dictionary, a misspelling has no suggestions.
    *speller.borrow_mut() = None;
    let offset = DOCUMENT.find("wrold").unwrap();
    let built = cx.update(|window, cx| {
        editor.update(cx, |e, cx| e.set_selected_range(offset..offset, cx));
        menu(NativeMenu::new(), window, cx)
    });
    assert!(!built.is_empty());
}

#[gpui::test]
fn clearing_a_buffer_drops_its_underlines(cx: &mut TestAppContext) {
    let open = open(cx);
    assert_eq!(open.misspelled(cx), ["wrold"]);
    let editor = open.editor(cx);
    cx.update(|cx| {
        let mut spell = BufferSpell::new(&editor, cx);
        let range = DOCUMENT.find("Hello").unwrap()..DOCUMENT.find("Hello").unwrap() + 5;
        spell
            .underlines
            .set(underlines(vec![range], gpui_kit::red()), cx);
        *spell.speller.borrow_mut() = Some(speller());
        assert_eq!(spell.underlines.get_ranges(cx).len(), 1);
        spell.clear(cx);
        assert!(spell.underlines.get_ranges(cx).is_empty());
        assert!(spell.speller.borrow().is_none());
    });
}
