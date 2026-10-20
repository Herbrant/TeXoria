use super::*;
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, gpui};

fn on(display: Option<String>, state: WindowState) -> WindowPlacement {
    WindowPlacement {
        display,
        x: 40.0,
        y: 30.0,
        width: 700.0,
        height: 900.0,
        state,
    }
}

#[gpui::test]
fn opens_where_it_was(cx: &mut TestAppContext) {
    let display = cx.update(|cx| cx.primary_display()).unwrap();
    let uuid = display.uuid().ok().map(|u| u.to_string());
    let bounds = Bounds::new(point(px(40.), px(30.)), size(px(700.), px(900.)));
    for (state, expected) in [
        (WindowState::Windowed, WindowBounds::Windowed(bounds)),
        (WindowState::Maximized, WindowBounds::Maximized(bounds)),
        (WindowState::Fullscreen, WindowBounds::Fullscreen(bounds)),
    ] {
        let options = cx.update(|cx| options(Some(&on(uuid.clone(), state)), cx));
        assert_eq!(options.display_id, Some(display.id()));
        assert_eq!(options.window_bounds, Some(expected));
    }
}

#[gpui::test]
fn opens_on_the_main_display_otherwise(cx: &mut TestAppContext) {
    let uuid = cx
        .update(|cx| cx.primary_display())
        .and_then(|d| d.uuid().ok())
        .map(|u| u.to_string());
    let centered = cx.update(|cx| WindowBounds::centered(DEFAULT_SIZE, cx));
    let tiny = WindowPlacement {
        width: 10.0,
        ..on(uuid.clone(), WindowState::Windowed)
    };
    let nan = WindowPlacement {
        x: f32::NAN,
        ..on(uuid, WindowState::Windowed)
    };
    // Never opened, its display unplugged, or bounds that make no sense.
    let unplugged = on(Some("gone".into()), WindowState::Windowed);
    for placement in [None, Some(&unplugged), Some(&tiny), Some(&nan)] {
        let options = cx.update(|cx| options(placement, cx));
        assert_eq!(options.display_id, None, "{placement:?}");
        assert_eq!(options.window_bounds, Some(centered), "{placement:?}");
    }
}
