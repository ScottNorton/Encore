//! Hash-route parsing. Pure path-segment logic, kept separate from the
//! DOM-driven dispatch in `app::route` so it can be unit-tested on the host.

/// Split a location hash into non-empty path segments.
///
/// `#settings/system/logs` -> ["settings", "system", "logs"];
/// `#`, ``, and `#/` -> [].
pub fn parse_path(hash: &str) -> Vec<String> {
    hash.trim_start_matches('#')
        .split('/')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_defaults_to_home() {
        assert!(parse_path("#").is_empty());
        assert!(parse_path("").is_empty());
    }

    #[test]
    fn single_segment() {
        assert_eq!(parse_path("#sound"), vec!["sound".to_string()]);
    }

    #[test]
    fn nested_segments() {
        assert_eq!(
            parse_path("#settings/system/logs"),
            vec![
                "settings".to_string(),
                "system".to_string(),
                "logs".to_string()
            ]
        );
    }

    #[test]
    fn strips_leading_slash_and_blanks() {
        assert_eq!(
            parse_path("#/settings//network/"),
            vec!["settings".to_string(), "network".to_string()]
        );
    }
}
