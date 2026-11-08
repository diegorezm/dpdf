// Release builds are GUI apps: don't open a console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod keymap;
mod layout;
mod nav;
mod pdf;
mod viewer;

use std::cell::RefCell;

use keymap::Action;
use pdf::{Event, Worker};
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
    VIEWER.with(|viewer| *viewer.borrow_mut() = Some(Viewer::new(&window, worker)));

    // `dpdf file.pdf`, which is also how "Open with" passes files on Windows.
    if let Some(path) = std::env::args_os().nth(1) {
        with_viewer(|viewer| viewer.open(path.into()));
    }

    let weak = window.as_weak();
    window.on_key_pressed(move |text, ctrl| {
        let (Some(window), Some(action)) = (weak.upgrade(), keymap::map_key(&text, ctrl)) else {
            return false;
        };
        handle_action(&window, action);
        true
    });

    let weak = window.as_weak();
    window.on_scrolled(move || {
        if let Some(window) = weak.upgrade() {
            with_viewer(|viewer| viewer.refresh(&window));
        }
    });

    let weak = window.as_weak();
    window.on_resized(move || {
        if let Some(window) = weak.upgrade() {
            with_viewer(|viewer| viewer.relayout(&window));
        }
    });

    window.run()
}

fn handle_action(window: &AppWindow, action: Action) {
    match action {
        Action::Open => {
            let picked = rfd::FileDialog::new()
                .set_title("Open PDF")
                .add_filter("PDF", &["pdf"])
                .pick_file();
            if let Some(path) = picked {
                with_viewer(|viewer| viewer.open(path));
            }
        }
        Action::ToggleSidebar => window.set_sidebar_visible(!window.get_sidebar_visible()),
        _ => with_viewer(|viewer| viewer.handle(window, action)),
    }
}

fn apply_event(viewer: &mut Viewer, window: &AppWindow, event: Event) {
    match event {
        Event::Opened { title, page_sizes } => viewer.opened(window, title, page_sizes),
        Event::Rendered {
            page,
            width_px,
            bitmap,
        } => viewer.rendered(window, page, width_px, bitmap),
        Event::Error(message) => window.set_status(message.into()),
    }
}
