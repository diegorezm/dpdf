use slint::platform::Key;

/// User-level commands triggered from the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Open,
    Search,
    GoToPage,
    Settings,
    ToggleSidebar,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    FitWidth,
    FitPage,
    PreviousPage,
    NextPage,
    FirstPage,
    LastPage,
    Fullscreen,
    Escape,
}

/// Maps a key press (Slint key text plus Ctrl state) to an action.
pub fn map_key(text: &str, ctrl: bool) -> Option<Action> {
    if ctrl {
        return match text.to_ascii_lowercase().as_str() {
            "o" => Some(Action::Open),
            "f" => Some(Action::Search),
            "p" => Some(Action::GoToPage),
            "," => Some(Action::Settings),
            "b" => Some(Action::ToggleSidebar),
            // `=` is the unshifted `+` key on most layouts.
            "+" | "=" => Some(Action::ZoomIn),
            "-" => Some(Action::ZoomOut),
            "0" => Some(Action::ZoomReset),
            "1" => Some(Action::FitWidth),
            "2" => Some(Action::FitPage),
            _ => None,
        };
    }

    let mut chars = text.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return None;
    };
    let is = |key: Key| c == char::from(key);
    if is(Key::LeftArrow) || is(Key::PageUp) {
        Some(Action::PreviousPage)
    } else if is(Key::RightArrow) || is(Key::PageDown) {
        Some(Action::NextPage)
    } else if is(Key::Home) {
        Some(Action::FirstPage)
    } else if is(Key::End) {
        Some(Action::LastPage)
    } else if is(Key::F11) {
        Some(Action::Fullscreen)
    } else if is(Key::Escape) {
        Some(Action::Escape)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: Key) -> String {
        char::from(k).to_string()
    }

    #[test]
    fn ctrl_shortcuts() {
        assert_eq!(map_key("o", true), Some(Action::Open));
        assert_eq!(map_key("O", true), Some(Action::Open));
        assert_eq!(map_key("f", true), Some(Action::Search));
        assert_eq!(map_key("p", true), Some(Action::GoToPage));
        assert_eq!(map_key(",", true), Some(Action::Settings));
        assert_eq!(map_key("b", true), Some(Action::ToggleSidebar));
        assert_eq!(map_key("+", true), Some(Action::ZoomIn));
        assert_eq!(map_key("=", true), Some(Action::ZoomIn));
        assert_eq!(map_key("-", true), Some(Action::ZoomOut));
        assert_eq!(map_key("0", true), Some(Action::ZoomReset));
        assert_eq!(map_key("1", true), Some(Action::FitWidth));
        assert_eq!(map_key("2", true), Some(Action::FitPage));
        assert_eq!(map_key("o", false), None);
    }

    #[test]
    fn plain_keys() {
        assert_eq!(
            map_key(&key(Key::LeftArrow), false),
            Some(Action::PreviousPage)
        );
        assert_eq!(
            map_key(&key(Key::PageUp), false),
            Some(Action::PreviousPage)
        );
        assert_eq!(
            map_key(&key(Key::RightArrow), false),
            Some(Action::NextPage)
        );
        assert_eq!(map_key(&key(Key::PageDown), false), Some(Action::NextPage));
        assert_eq!(map_key(&key(Key::Home), false), Some(Action::FirstPage));
        assert_eq!(map_key(&key(Key::End), false), Some(Action::LastPage));
        assert_eq!(map_key(&key(Key::F11), false), Some(Action::Fullscreen));
        assert_eq!(map_key(&key(Key::Escape), false), Some(Action::Escape));
        assert_eq!(map_key("", false), None);
    }
}
