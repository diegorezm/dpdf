// Release builds are GUI apps: don't open a console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod keymap;
mod layout;
mod nav;
mod pdf;
mod search;
mod store;
mod viewer;

use std::cell::RefCell;

use keymap::Action;
use pdf::{Event, Worker};
use store::Store;
use viewer::Viewer;

slint::include_modules!();

thread_local! {
    // UI-thread state. Worker events reach it via invoke_from_event_loop,
    // whose closures must be Send and so can't capture an Rc.
    static VIEWER: RefCell<Option<Viewer>> = const { RefCell::new(None) };
}

fn with_viewer(f: impl FnOnce(&mut Viewer)) {
    VIEWER.with(|viewer| {
        // A busy viewer means this is a re-entrant property-change callback;
        // the outer call refreshes anyway.
        if let Ok(mut viewer) = viewer.try_borrow_mut() {
            if let Some(viewer) = viewer.as_mut() {
                f(viewer);
            }
        }
    });
}

/// Registers a window callback that runs `f` with the viewer.
macro_rules! on {
    ($window:ident . $setter:ident, |$viewer:ident, $win:ident $(, $arg:ident)*| $body:expr) => {{
        let weak = $window.as_weak();
        $window.$setter(move |$($arg),*| {
            if let Some($win) = weak.upgrade() {
                with_viewer(|$viewer| $body);
            }
        });
    }};
}

fn main() -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;

    let weak = window.as_weak();
    let worker = Worker::spawn(move |event| {
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(window) = weak.upgrade() {
                with_viewer(|viewer| apply_event(viewer, &window, event));
            }
        });
    });
    let viewer = Viewer::new(&window, worker, Store::load());
    VIEWER.with(|cell| *cell.borrow_mut() = Some(viewer));

    // `dpdf file.pdf`, which is also how "Open with" passes files on Windows.
    if let Some(path) = std::env::args_os().nth(1) {
        with_viewer(|viewer| viewer.open(&window, path.into()));
    }

    let weak = window.as_weak();
    window.on_key_pressed(move |text, ctrl, _shift| {
        let (Some(window), Some(action)) = (weak.upgrade(), keymap::map_key(&text, ctrl)) else {
            return false;
        };
        handle_action(&window, action);
        true
    });

    on!(window.on_scrolled, |viewer, win| viewer.refresh(&win));
    on!(window.on_resized, |viewer, win| viewer.relayout(&win));
    on!(window.on_thumbs_scrolled, |viewer, win| viewer
        .refresh_thumbs(&win));
    on!(window.on_thumb_clicked, |viewer, win, page| {
        viewer.go_to_page(&win, usize::try_from(page).unwrap_or(0))
    });
    on!(window.on_open_recent, |viewer, win, index| {
        viewer.open_recent(&win, usize::try_from(index).unwrap_or(0))
    });
    on!(window.on_search_edited, |viewer, win, text| viewer
        .search_edited(&win, &text));
    on!(window.on_search_step, |viewer, win, forward| viewer
        .search_step(&win, forward));
    on!(window.on_goto_accepted, |viewer, win, text| viewer
        .go_to_typed_page(&win, &text));
    on!(window.on_settings_clicked, |viewer, win| viewer
        .handle(&win, Action::Settings));
    on!(window.on_zoom_action, |viewer, win, action| {
        let action = match action.as_str() {
            "in" => Action::ZoomIn,
            "out" => Action::ZoomOut,
            "fit-page" => Action::FitPage,
            _ => Action::FitWidth,
        };
        viewer.handle(&win, action)
    });
    on!(window.on_toggle_theme, |viewer, win| viewer
        .toggle_theme(&win));
    on!(window.on_toggle_sidebar_default, |viewer, win| viewer
        .toggle_sidebar_default(&win));

    window.run()?;
    with_viewer(|viewer| viewer.remember_position());
    Ok(())
}

fn handle_action(window: &AppWindow, action: Action) {
    if action == Action::Open {
        // Modal native dialog; returns once the user picks or cancels.
        let picked = rfd::FileDialog::new()
            .set_title("Open PDF")
            .add_filter("PDF", &["pdf"])
            .pick_file();
        if let Some(path) = picked {
            with_viewer(|viewer| viewer.open(window, path));
        }
        return;
    }
    with_viewer(|viewer| viewer.handle(window, action));
}

fn apply_event(viewer: &mut Viewer, window: &AppWindow, event: Event) {
    match event {
        Event::Opened { title, page_sizes } => viewer.opened(window, title, page_sizes),
        Event::Rendered {
            page,
            width_px,
            bitmap,
        } => viewer.rendered(window, page, width_px, bitmap),
        Event::Thumbnail { page, bitmap } => viewer.thumbnail(window, page, bitmap),
        Event::SearchHits { id, page, matches } => viewer.search_hits(window, id, page, matches),
        Event::SearchDone { id } => viewer.search_done(window, id),
        Event::Error(message) => viewer.open_failed(window, message),
    }
}
