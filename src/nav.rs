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

/// The article number in "art. 37", "Art 5º", or "artigo 121", for
/// jumping to an article of a law.
pub fn parse_article(text: &str) -> Option<u32> {
    let lower = text.trim().to_lowercase();
    let rest = lower
        .strip_prefix("artigo")
        .or_else(|| lower.strip_prefix("art"))?;
    let rest = rest.trim_start_matches('.').trim_start();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let tail = rest[digits.len()..].trim();
    if !tail.is_empty() && !["º", "°", "o", "."].contains(&tail) {
        return None;
    }
    digits.parse().ok().filter(|n| *n > 0)
}

/// Whether a page's text has a line starting article `number`, like
/// "Art. 37. A administração..." but not "Art. 370" or a citation in the
/// middle of a sentence ("nos termos do art. 37").
pub fn starts_article(text: &str, number: u32) -> bool {
    let number = number.to_string();
    text.lines().any(|line| {
        let Some(rest) = line.trim_start().strip_prefix("Art") else {
            return false;
        };
        let rest = rest.strip_prefix('.').unwrap_or(rest).trim_start();
        rest.strip_prefix(number.as_str())
            .is_some_and(|after| !after.starts_with(|c: char| c.is_ascii_digit()))
    })
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

    #[test]
    fn articles() {
        assert_eq!(parse_article("art. 37"), Some(37));
        assert_eq!(parse_article("Art 5º"), Some(5));
        assert_eq!(parse_article("artigo 121"), Some(121));
        assert_eq!(parse_article("art.1"), Some(1));
        assert_eq!(parse_article("arte"), None);
        assert_eq!(parse_article("art. 37 da CF"), None);
        assert_eq!(parse_article("12"), None);

        let page = "TÍTULO III
Art. 37. A administração pública direta
nos termos do art. 38 desta lei";
        assert!(starts_article(page, 37));
        assert!(!starts_article(page, 3));
        assert!(!starts_article(page, 38));
        assert!(starts_article("  Art. 5º Todos são iguais", 5));
        assert!(!starts_article("Art. 370. Outro", 37));
    }
}
