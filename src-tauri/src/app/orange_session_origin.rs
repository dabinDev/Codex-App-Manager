pub fn normalize_service_root(configured_url: &str) -> &str {
    configured_url.strip_suffix('/').unwrap_or(configured_url)
}

pub fn remembered_origin_matches(stored_origin: Option<&str>, configured_origin: &str) -> bool {
    stored_origin == Some(configured_origin)
}

#[cfg(test)]
mod tests {
    use super::{normalize_service_root, remembered_origin_matches};

    #[test]
    fn normalized_service_root_preserves_the_configured_path() {
        assert_eq!(
            normalize_service_root("https://api.example.test/tenant-a"),
            "https://api.example.test/tenant-a"
        );
        assert_eq!(
            normalize_service_root("https://api.example.test/"),
            "https://api.example.test"
        );
        assert!(!remembered_origin_matches(
            Some("https://api.example.test/tenant-a"),
            normalize_service_root("https://api.example.test/tenant-b")
        ));
    }

    #[test]
    fn remembered_origin_must_match_the_configured_origin_exactly() {
        assert!(remembered_origin_matches(
            Some("https://api.example.test"),
            "https://api.example.test"
        ));
        assert!(!remembered_origin_matches(
            Some("https://old.example.test"),
            "https://api.example.test"
        ));
        assert!(!remembered_origin_matches(None, "https://api.example.test"));
    }
}
