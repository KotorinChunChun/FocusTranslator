// エンジンID・ラベル定数 (SPEC v0.3)
// OCR/翻訳エンジンの識別キー・表示ラベルを一元管理する。
// overlay.rs, app_state.rs, chip_handler.rs など複数モジュールから参照される。
use crate::config::Config;

/// OCRエンジンのキー配列 (チップボタンのID算出に使用)
pub const OCR_KEYS: [&str; 4] = ["oneocr", "win", "paddle", "llm"];
/// OCRエンジンの表示ラベル (ボタン表示・見出し用)
pub const OCR_LABELS: [&str; 4] = ["OneOCR", "MediaOCR", "Paddle", "LLM(統合)"];
/// 翻訳エンジンのキー配列
pub const TR_KEYS: [&str; 4] = ["local", "deepl", "google", "llm"];
/// 翻訳エンジンの表示ラベル
pub const TR_LABELS: [&str; 4] = ["ローカル", "DeepL", "Google", "LLM"];

/// OCRエンジンキーから表示ラベルを取得する
pub fn ocr_label(key: &str) -> &'static str {
    OCR_KEYS.iter().position(|k| *k == key).map(|i| OCR_LABELS[i]).unwrap_or("OneOCR")
}

/// 翻訳エンジンキーから表示ラベルを取得する
pub fn tr_label(key: &str) -> &'static str {
    TR_KEYS.iter().position(|k| *k == key).map(|i| TR_LABELS[i]).unwrap_or("ローカル")
}

/// 翻訳エンジンの表示名を生成する。LLMの場合はプロファイル名を含める。
/// (例: "LLM:Gemini Default", "DeepL", "ローカル")
pub fn tr_display_name(key: &str, cfg: &Config) -> String {
    if key == "llm" {
        let profile_name = cfg
            .active_profile()
            .map(|p| p.name.clone())
            .unwrap_or_default();
        format!("LLM:{}", profile_name)
    } else {
        tr_label(key).to_string()
    }
}
