//! TeXoria's light ("Paper") and dark ("Ink") themes.

use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
use gpui_kit::*;

use crate::assets::THEMES;

const LIGHT: &str = "TeXoria Paper";
const DARK: &str = "TeXoria Ink";

/// Register the themes and pick the one matching the system appearance.
pub fn init(window: Option<&mut Window>, cx: &mut App) {
    if let Err(e) = ThemeRegistry::global_mut(cx).load_themes_from_str(THEMES) {
        log::error!("invalid theme file: {e}");
        return;
    }
    let themes = ThemeRegistry::global(cx).themes().clone();
    let (Some(light), Some(dark)) = (themes.get(LIGHT).cloned(), themes.get(DARK).cloned()) else {
        return;
    };
    let mode = match cx.window_appearance() {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
        _ => ThemeMode::Light,
    };
    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
    Theme::change(mode, window, cx);
}

pub fn toggle(window: &mut Window, cx: &mut App) {
    let mode = if cx.theme_is_dark() {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    Theme::change(mode, Some(window), cx);
}

pub trait ThemeExt {
    fn theme_is_dark(&self) -> bool;
}

impl ThemeExt for App {
    fn theme_is_dark(&self) -> bool {
        Theme::global(self).is_dark()
    }
}
