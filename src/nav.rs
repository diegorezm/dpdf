use crate::keymap::Action;

/// Returns the zero-based page an action navigates to, or `None` if the
/// action does not navigate or there is no document.
pub fn target_page(action: Action, current: usize, page_count: usize) -> Option<usize> {
    let last = page_count.checked_sub(1)?;
    match action {
        Action::PreviousPage => Some(current.saturating_sub(1)),
        Action::NextPage => Some((current + 1).min(last)),
        Action::FirstPage => Some(0),
        Action::LastPage => Some(last),
        _ => None,
    }
}

/// Parses a one-based page number typed by the user into a zero-based page,
/// clamped to the document.
pub fn parse_page(text: &str, page_count: usize) -> Option<usize> {
    let number: usize = text.trim().parse().ok()?;
    let last = page_count.checked_sub(1)?;
    Some(number.saturating_sub(1).min(last))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_at_bounds() {
        assert_eq!(target_page(Action::PreviousPage, 0, 10), Some(0));
        assert_eq!(target_page(Action::NextPage, 9, 10), Some(9));
        assert_eq!(target_page(Action::NextPage, 3, 10), Some(4));
        assert_eq!(target_page(Action::LastPage, 3, 10), Some(9));
        assert_eq!(target_page(Action::FirstPage, 3, 10), Some(0));
    }

    #[test]
    fn no_document() {
        assert_eq!(target_page(Action::NextPage, 0, 0), None);
        assert_eq!(parse_page("3", 0), None);
    }

    #[test]
    fn non_navigation_actions() {
        assert_eq!(target_page(Action::Open, 0, 10), None);
    }

    #[test]
    fn parses_page_numbers() {
        assert_eq!(parse_page("1", 10), Some(0));
        assert_eq!(parse_page(" 7 ", 10), Some(6));
        assert_eq!(parse_page("0", 10), Some(0));
        assert_eq!(parse_page("999", 10), Some(9));
        assert_eq!(parse_page("abc", 10), None);
        assert_eq!(parse_page("", 10), None);
    }
}
