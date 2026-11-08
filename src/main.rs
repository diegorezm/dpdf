// Release builds are GUI apps: don't open a console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod annotations;
mod app;
mod config;
mod keymap;
mod layout;
mod nav;
mod palette;
mod pdf;
mod search;
mod store;
mod viewer;

use std::cell::RefCell;

use app::App;
use keymap::Action;
use pdf::Worker;

slint::include_modules!();

thread_local! {
    // UI-thread state. Worker events reach it via invoke_from_event_loop,
    // whose closures must be Send and so can't capture an Rc.
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app(f: impl FnOnce(&mut App)) {
    APP.with(|app| {
        // A busy app means this is a re-entrant property-change callback;
        // the outer call refreshes anyway.
        if let Ok(mut app) = app.try_borrow_mut() {
            if let Some(app) = app.as_mut() {
                f(app);
            }
        }
    });
}

/// Registers a window callback that runs `$body` with the app.
macro_rules! on {
    ($window:ident . $setter:ident, |$app:ident, $win:ident $(, $arg:ident)*| $body:expr) => {{
        let weak = $window.as_weak();
        $window.$setter(move |$($arg),*| {
            if let Some($win) = weak.upgrade() {
                with_app(|$app| $body);
            }
        });
    }};
}

fn index(value: i32) -> usize {
    usize::try_from(value).unwrap_or(0)
}

fn main() -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;

    let weak = window.as_weak();
    let worker = Worker::spawn(move |event| {
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(window) = weak.upgrade() {
                with_app(|app| app.event(&window, event));
            }
        });
    });
    let app = App::new(&window, worker);
    APP.with(|cell| *cell.borrow_mut() = Some(app));

    // `dpdf file.pdf`, which is also how "Open with" passes files on Windows.
    for path in std::env::args_os().skip(1) {
        with_app(|app| app.open_path(&window, path.into()));
    }

    let weak = window.as_weak();
    window.on_key_pressed(move |text, ctrl, shift, alt| {
        let Some(window) = weak.upgrade() else {
            return false;
        };
        let mut handled = false;
        with_app(|app| handled = app.key_pressed(&window, &text, ctrl, shift, alt));
        handled
    });

    on!(window.on_scrolled, |app, win| app.viewer().refresh(&win));
    on!(window.on_resized, |app, win| app.viewer().relayout(&win));
    on!(window.on_thumbs_scrolled, |app, win| app
        .viewer()
        .refresh_thumbs(&win));
    on!(window.on_thumb_clicked, |app, win, page| app
        .viewer()
        .go_to_page(&win, index(page)));
    on!(window.on_open_recent, |app, win, i| app
        .open_recent(&win, index(i)));
    on!(window.on_restore_session_clicked, |app, win| app
        .restore_session(&win));
    on!(window.on_search_edited, |app, win, text| app
        .viewer()
        .search_edited(&win, &text));
    on!(window.on_search_step, |app, win, forward| app
        .viewer()
        .search_step(&win, forward));
    on!(window.on_goto_accepted, |app, win, text| app
        .go_to_typed_page(&win, &text));
    on!(window.on_settings_clicked, |app, win| app
        .run(&win, Action::Settings));
    on!(window.on_zoom_action, |app, win, action| {
        let action = match action.as_str() {
            "in" => Action::ZoomIn,
            "out" => Action::ZoomOut,
            "fit-page" => Action::FitPage,
            _ => Action::FitWidth,
        };
        app.run(&win, action)
    });
    on!(window.on_tab_clicked, |app, win, i| app
        .open_tab(&win, index(i)));
    on!(window.on_tab_closed, |app, win, i| app
        .close_tab(&win, index(i)));
    on!(window.on_reader_pointer, |app, win, kind, x, y| app
        .viewer()
        .pointer(&win, &kind, x, y));
    on!(window.on_popup_action, |app, win, action, i| app
        .popup_action(&win, &action, index(i)));
    on!(window.on_note_saved, |app, win| app.note_saved(&win));
    on!(window.on_palette_edited, |app, win, text| app
        .palette_edited(&win, &text));
    on!(window.on_palette_moved, |app, win, delta| app
        .palette_moved(&win, delta));
    on!(window.on_palette_accepted, |app, win| app
        .palette_accept(&win, None));
    on!(window.on_palette_clicked, |app, win, i| app
        .palette_accept(&win, Some(index(i))));
    on!(window.on_settings_set, |app, win, key, value| app
        .setting(&win, &key, &value));

    window.run()?;
    with_app(|app| app.quit());
    Ok(())
}
