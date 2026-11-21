pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub struct WindowTitle;
impl WindowTitle {
    pub fn create(screen: &str) -> String {
        Self::create_with_branding(screen, false)
    }

    pub fn create_with_branding(screen: &str, boss_guard: bool) -> String {
        let product = crate::util::app_display_name(boss_guard);
        if screen.is_empty() {
            format!("{product} - ver {APP_VERSION}")
        } else {
            format!("{screen} - {product} - ver {APP_VERSION}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn titles_share_the_product_version() {
        assert_eq!(
            WindowTitle::create(""),
            format!("FocusTranslator - ver {APP_VERSION}")
        );
        assert_eq!(
            WindowTitle::create("設定"),
            format!("設定 - FocusTranslator - ver {APP_VERSION}")
        );
        assert!(!WindowTitle::create_with_branding("設定", true).contains("FocusTranslator"));
    }
}
