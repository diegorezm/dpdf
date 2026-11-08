slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;

    let weak = window.as_weak();
    window.on_toggle_sidebar(move || {
        if let Some(window) = weak.upgrade() {
            window.set_sidebar_visible(!window.get_sidebar_visible());
        }
    });

    window.run()
}
