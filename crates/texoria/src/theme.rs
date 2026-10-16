//! TeXoria's light ("Paper") and dark ("Ink") themes.

use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
use gpui_kit::*;

use crate::assets::THEMES;
use crate::settings::ThemeChoice;

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
    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
    apply(ThemeChoice::System, window, cx);
}

/// Switch to the theme `choice` asks for.
pub fn apply(choice: ThemeChoice, window: Option<&mut Window>, cx: &mut App) {
    let mode = match choice {
        ThemeChoice::Light => ThemeMode::Light,
        ThemeChoice::Dark => ThemeMode::Dark,
        ThemeChoice::System => match cx.window_appearance() {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
            _ => ThemeMode::Light,
        },
    };
    Theme::change(mode, window, cx);
}

/// The choice that switches away from the theme on screen.
pub fn toggled(cx: &App) -> ThemeChoice {
    if cx.theme_is_dark() {
        ThemeChoice::Light
    } else {
        ThemeChoice::Dark
    }
}

pub trait ThemeExt {
    fn theme_is_dark(&self) -> bool;
}

impl ThemeExt for App {
    fn theme_is_dark(&self) -> bool {
        Theme::global(self).is_dark()
    }
}
