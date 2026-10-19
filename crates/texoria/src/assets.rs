//! Embedded assets: the app's own icons and theme, layered over GPUI Kit's
//! default component assets.

use std::borrow::Cow;

use gpui_kit::component::IconNamed;
use gpui_kit::*;

pub const THEMES: &str = include_str!("../assets/themes.json");

/// IBM Plex Mono (SIL Open Font License), the editor typeface.
pub fn fonts() -> Vec<Cow<'static, [u8]>> {
    vec![
        Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexMono-Italic.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexMono-SemiBold.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexMono-Bold.ttf")),
    ]
}

macro_rules! glyphs {
    ($($name:ident => $file:literal,)*) => {
        /// Icons used by TeXoria (Lucide, ISC licensed).
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Glyph {
            $($name,)*
        }

        impl IconNamed for Glyph {
            fn path(self) -> SharedString {
                match self {
                    $(Glyph::$name => concat!("texoria/icons/", $file, ".svg"),)*
                }
                .into()
            }
        }

        fn glyph_bytes(path: &str) -> Option<&'static [u8]> {
            match path {
                $(concat!("texoria/icons/", $file, ".svg") => {
                    Some(include_bytes!(concat!("../assets/icons/", $file, ".svg")))
                })*
                _ => None,
            }
        }
    };
}

glyphs! {
    Play => "play",
    PanelLeft => "panel-left",
    ZoomIn => "zoom-in",
    ZoomOut => "zoom-out",
    Sun => "sun",
    Moon => "moon",
    Warning => "triangle-alert",
    Ok => "circle-check",
    Spinner => "loader-circle",
    File => "file-text",
    Error => "circle-x",
    Info => "info",
    Section => "hash",
    Figure => "image",
    Table => "table",
    Open => "folder-open",
    Close => "x",
    Book => "book-open",
    Branch => "git-branch",
    Commit => "git-commit-horizontal",
    Conflict => "git-merge-conflict",
    Push => "arrow-up",
    Pull => "arrow-down",
    Fetch => "cloud-download",
    Refresh => "refresh-cw",
    Plus => "plus",
    Minus => "minus",
    Discard => "undo-2",
    Check => "check",
    Split => "columns-2",
    Unified => "rows-2",
    PrevChange => "chevron-up",
    NextChange => "chevron-down",
    Folder => "folder",
    Expand => "chevron-right",
    ExpandAll => "chevrons-up-down",
    CollapseAll => "chevrons-down-up",
    PlainFile => "file",
    NewFile => "file-plus",
    NewFolder => "folder-plus",
    Settings => "settings",
    Languages => "languages",
    Sparkles => "sparkles",
    Stop => "square",
    Send => "send-horizontal",
    NewChat => "message-square-plus",
    Edit => "pencil",
    Read => "eye",
    Search => "search",
    Terminal => "terminal",
    Thought => "brain",
    Plan => "list-checks",
    Tool => "wrench",
    Web => "globe",
    Changes => "file-diff",
    CaseSensitive => "case-sensitive",
    WholeWord => "whole-word",
    Regex => "regex",
    ReplaceAll => "replace-all",
    Export => "download",
    Presentation => "presentation",
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = glyph_bytes(path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit::assets::Assets.list(path)
    }
}
