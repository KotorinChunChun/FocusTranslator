// 設定の永続化 (%APPDATA%\FocusTranslator\config.json)
// APIキーは DPAPI で暗号化した base64 を保存する。
use crate::util;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// ホールドキー: "RCtrl" | "LCtrl" | "RShift" | "RAlt" | "F8"
    pub hold_key: String,
    /// GetAsyncKeyState の監視周期 (ms)
    pub poll_ms: u32,
    /// 範囲指定ホットキー (例: "Ctrl+Alt+T")
    pub region_hotkey: String,
    /// 既定OCRエンジン: "win" | "paddle" | "yomitoku" | "ndl" | "gemini"
    pub default_ocr: String,
    /// 既定翻訳エンジン: "local" | "deepl" | "google" | "gemini"
    pub default_translator: String,
    /// 訳先言語 (原文がCJKの場合は自動で "en" へ反転)
    pub target_lang: String,
    pub deepl_key_enc: String,
    pub google_key_enc: String,
    pub gemini_key_enc: String,
    pub gemini_model: String,
    pub yomitoku_url: String,
    pub ndl_url: String,
    /// 外部送信同意: テキスト送信 / 画像送信 / 外部OCRサーバー送信
    pub consent_text: bool,
    pub consent_image: bool,
    pub consent_ext_ocr: bool,
    pub autostart: bool,
    pub perf_log: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hold_key: "RCtrl".into(),
            poll_ms: 100,
            region_hotkey: "Ctrl+Alt+T".into(),
            default_ocr: "win".into(),
            default_translator: "local".into(),
            target_lang: "ja".into(),
            deepl_key_enc: String::new(),
            google_key_enc: String::new(),
            gemini_key_enc: String::new(),
            gemini_model: "gemini-2.5-flash".into(),
            yomitoku_url: String::new(),
            ndl_url: String::new(),
            consent_text: false,
            consent_image: false,
            consent_ext_ocr: false,
            autostart: false,
            perf_log: false,
        }
    }
}

impl Config {
    pub fn path() -> std::path::PathBuf {
        util::config_dir().join("config.json")
    }

    pub fn load() -> Config {
        match std::fs::read_to_string(Self::path()) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), s);
        }
    }

    pub fn deepl_key(&self) -> String {
        util::dpapi_decrypt(&self.deepl_key_enc)
    }
    pub fn google_key(&self) -> String {
        util::dpapi_decrypt(&self.google_key_enc)
    }
    pub fn gemini_key(&self) -> String {
        util::dpapi_decrypt(&self.gemini_key_enc)
    }

    /// ホールドキーの仮想キーコード
    pub fn hold_vk(&self) -> i32 {
        match self.hold_key.as_str() {
            "LCtrl" => 0xA2,
            "RShift" => 0xA1,
            "RAlt" => 0xA5,
            "F8" => 0x77,
            _ => 0xA3, // RCtrl
        }
    }

    /// 範囲指定ホットキーの (修飾キー, 仮想キー)。解析失敗時は Ctrl+Alt+T。
    pub fn region_hotkey_parsed(&self) -> (u32, u32) {
        parse_hotkey(&self.region_hotkey).unwrap_or((0x0002 | 0x0001, b'T' as u32))
    }

    /// エンジンが利用可能か(キー・URL設定の有無)
    pub fn engine_available(&self, key: &str) -> bool {
        match key {
            "win" => true,
            "paddle" => crate::paddle_install::installed(),
            "yomitoku" => !self.yomitoku_url.trim().is_empty(),
            "ndl" => !self.ndl_url.trim().is_empty(),
            "gemini" => !self.gemini_key_enc.is_empty(),
            "local" => crate::translate::local_model_available(),
            "deepl" => !self.deepl_key_enc.is_empty(),
            "google" => !self.google_key_enc.is_empty(),
            _ => false,
        }
    }
}

/// "Ctrl+Alt+T" のような表記を (MOD_*, VK) に変換
pub fn parse_hotkey(s: &str) -> Option<(u32, u32)> {
    const MOD_ALT: u32 = 0x0001;
    const MOD_CONTROL: u32 = 0x0002;
    const MOD_SHIFT: u32 = 0x0004;
    const MOD_WIN: u32 = 0x0008;
    let mut mods = 0u32;
    let mut vk = 0u32;
    for part in s.split('+') {
        let p = part.trim();
        match p.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "alt" => mods |= MOD_ALT,
            "shift" => mods |= MOD_SHIFT,
            "win" => mods |= MOD_WIN,
            other => {
                let ch: Vec<char> = other.chars().collect();
                if ch.len() == 1 && ch[0].is_ascii_alphanumeric() {
                    vk = ch[0].to_ascii_uppercase() as u32;
                } else if let Some(n) = other.strip_prefix('f').and_then(|n| n.parse::<u32>().ok())
                    && (1..=24).contains(&n) {
                        vk = 0x70 + n - 1;
                    }
            }
        }
    }
    if vk != 0 && mods != 0 { Some((mods, vk)) } else { None }
}
