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
        Action::Open | Action::ToggleSidebar => None,
    }
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
    }

    #[test]
    fn non_navigation_actions() {
        assert_eq!(target_page(Action::Open, 0, 10), None);
    }
}
