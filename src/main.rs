// Release builds are GUI apps: don't open a console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod keymap;
mod nav;
mod pdf;

use keymap::Action;
use pdf::{Event, Request, Worker};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;

    let weak = window.as_weak();
    let worker = Worker::spawn(move |event| {
        let weak = weak.clone();
        // Pixel data is Send; the slint::Image is built on the UI thread.
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(window) = weak.upgrade() {
                apply_event(&window, event);
            }
        });
    });

    let weak = window.as_weak();
    window.on_key_pressed(move |text, ctrl| {
        let Some(window) = weak.upgrade() else {
            return false;
        };
        let Some(action) = keymap::map_key(&text, ctrl) else {
            return false;
        };
        handle_action(&window, &worker, action);
        true
    });

    window.run()
}

fn handle_action(window: &AppWindow, worker: &Worker, action: Action) {
    match action {
        Action::Open => {
            let picked = rfd::FileDialog::new()
                .set_title("Open PDF")
                .add_filter("PDF", &["pdf"])
                .pick_file();
            if let Some(path) = picked {
                window.set_page_count(0);
                window.set_status("Opening…".into());
                worker.send(Request::Open(path));
                request_render(window, worker, 0);
            }
        }
        Action::ToggleSidebar => window.set_sidebar_visible(!window.get_sidebar_visible()),
        _ => {
            // The UI shows one-based page numbers; navigation is zero-based.
            let current = usize::try_from(window.get_current_page() - 1).unwrap_or(0);
            let page_count = usize::try_from(window.get_page_count()).unwrap_or(0);
            if let Some(page) = nav::target_page(action, current, page_count) {
                if page != current {
                    window.set_current_page(ui_int(page + 1));
                    request_render(window, worker, page);
                }
            }
        }
    }
}

fn request_render(window: &AppWindow, worker: &Worker, page: usize) {
    let scale = window.window().scale_factor();
    let to_px = |logical: f32| (logical * scale).max(1.0) as u32;
    worker.send(Request::Render {
        page,
        max_width: to_px(window.get_reader_width()),
        max_height: to_px(window.get_reader_height()),
    });
}

fn apply_event(window: &AppWindow, event: Event) {
    match event {
        Event::Opened { title, page_count } => {
            window.set_document_title(title.into());
            window.set_page_count(ui_int(page_count));
            window.set_current_page(1);
            window.set_status("".into());
        }
        Event::Rendered(page) => {
            let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                &page.rgba,
                page.width,
                page.height,
            );
            window.set_page_image(Image::from_rgba8(buffer));
            window.set_has_page(true);
        }
        Event::Error(message) => {
            window.set_has_page(false);
            window.set_status(message.into());
        }
    }
}

fn ui_int(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}
