pub const PRODUCT_NAME: &str = env!("BRAND_PRODUCT_NAME");
pub const SERVICE_NAME: &str = env!("BRAND_SERVICE_NAME");
pub const SUPPORT_URL: &str = env!("BRAND_SUPPORT_URL");
pub const SERVICE_BASE_URL: &str = env!("BRAND_SERVICE_BASE_URL");
pub const MIRROR_BASE_URL: &str = env!("BRAND_MIRROR_BASE_URL");
pub const SKINS_BASE_URL: &str = env!("BRAND_SKINS_BASE_URL");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_urls_are_https_and_product_name_is_present() {
        assert!(!PRODUCT_NAME.trim().is_empty());
        assert!(!SERVICE_NAME.trim().is_empty());
        for url in [
            SUPPORT_URL,
            SERVICE_BASE_URL,
            MIRROR_BASE_URL,
            SKINS_BASE_URL,
        ] {
            assert!(url.starts_with("https://"), "{url}");
            assert!(!url.ends_with('/'), "{url}");
        }
    }
}
