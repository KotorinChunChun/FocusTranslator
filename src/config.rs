// 設定の永続化 (%APPDATA%\FocusTranslator\config.json)
// APIキーは DPAPI で暗号化した base64 を保存する。
use crate::util;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct GlossaryEntry {
    pub source: String,
    pub target: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ApiType {
    Gemini,
    OpenAI,
    Claude,
}
impl Default for ApiType {
    fn default() -> Self { ApiType::Gemini }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct ApiProfile {
    pub name: String,
    pub api_type: ApiType,
    pub model_name: String,
    pub api_url: String,
    pub api_key_enc: String,
    pub ocr_prompt: String,
    pub translate_prompt: String,
    pub explain_prompt: String,
}

impl ApiProfile {
    pub fn get_key(&self) -> String {
        crate::util::dpapi_decrypt(&self.api_key_enc)
    }
    pub fn set_key(&mut self, plain: &str) {
        if plain.is_empty() {
            self.api_key_enc.clear();
        } else {
            self.api_key_enc = crate::util::dpapi_encrypt(plain);
        }
    }
}

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
    /// 翻訳元言語 (既定 en)
    pub source_lang: String,
    pub deepl_key_enc: String,
    pub google_key_enc: String,
    
    // API Profile設定 (v0.2以降)
    pub api_profiles: Vec<ApiProfile>,
    pub active_api_profile: String,

    // 旧バージョンのAPI設定 (読み取り専用・保存しない)
    #[serde(default, skip_serializing)]
    pub gemini_key_enc: String,
    #[serde(default, skip_serializing)]
    pub gemini_model: String,
    #[serde(default, skip_serializing)]
    pub gpt_url: String,
    #[serde(default, skip_serializing)]
    pub gpt_key_enc: String,
    #[serde(default, skip_serializing)]
    pub gpt_model: String,
    #[serde(default, skip_serializing)]
    pub gemini_translate_prompt: String,
    #[serde(default, skip_serializing)]
    pub gemini_ocr_prompt: String,
    #[serde(default, skip_serializing)]
    pub gemini_explain_prompt: String,
    pub yomitoku_url: String,
    pub ndl_url: String,
    /// 外部送信同意: テキスト送信 / 画像送信 / 外部OCRサーバー送信
    pub consent_text: bool,
    pub consent_image: bool,
    pub consent_ext_ocr: bool,
    pub autostart: bool,
    pub perf_log: bool,
    /// 実行ログをSQLiteに記録する (既定OFF)
    pub log_enabled: bool,
    /// デバッグモード: OCR時にキャプチャ画像をPNG保存 (既定OFF)
    pub debug_mode: bool,
    /// 認識ログの保持上限件数
    pub log_max_records: u32,
    /// ローカルONNX翻訳のモデル種別: "opus_mt" | "fugu_mt" | "nllb200"
    pub local_model_variant: String,
    /// 用語集 (原文 -> 訳文)
    pub glossary: Vec<GlossaryEntry>,
}

/// Gemini翻訳プロンプトの既定値
pub const DEFAULT_GEMINI_TRANSLATE_PROMPT: &str =
    "Translate the following text from {{source_lang}} to {{target_lang}}. Output only the translation.\n{{glossary}}\n\n{{text}}";
/// Gemini OCR+翻訳統合プロンプトの既定値
pub const DEFAULT_GEMINI_OCR_PROMPT: &str =
    "Extract the text in this image and translate it from {{source_lang}} to {{target_lang}}. Respond with JSON only: {\"source\": \"<extracted text>\", \"translation\": \"<translation>\"}\n{{glossary}}";
/// Gemini解説プロンプトの既定値
pub const DEFAULT_GEMINI_EXPLAIN_PROMPT: &str =
    "Explain the grammar, nuances, and background of the following text in {{target_lang}}.\n{{glossary}}\n\n{{text}}";

impl Default for Config {
    fn default() -> Self {
        Config {
            hold_key: "RCtrl".into(),
            poll_ms: 100,
            region_hotkey: "Ctrl+Alt+T".into(),
            default_ocr: "win".into(),
            default_translator: "local".into(),
            target_lang: "ja".into(),
            source_lang: "en".into(),
            deepl_key_enc: String::new(),
            google_key_enc: String::new(),
            api_profiles: Vec::new(),
            active_api_profile: "Gemini Default".into(),
            gemini_key_enc: String::new(),
            gemini_model: "gemini-3.5-flash".into(),
            gpt_url: "https://api.openai.com/v1/chat/completions".into(),
            gpt_key_enc: String::new(),
            gpt_model: "gpt-4o-mini".into(),
            gemini_translate_prompt: DEFAULT_GEMINI_TRANSLATE_PROMPT.into(),
            gemini_ocr_prompt: DEFAULT_GEMINI_OCR_PROMPT.into(),
            gemini_explain_prompt: DEFAULT_GEMINI_EXPLAIN_PROMPT.into(),
            yomitoku_url: String::new(),
            ndl_url: String::new(),
            consent_text: false,
            consent_image: false,
            consent_ext_ocr: false,
            autostart: false,
            perf_log: false,
            log_enabled: false,
            debug_mode: false,
            log_max_records: 5000,
            local_model_variant: "opus_mt".into(),
            glossary: Vec::new(),
        }
    }
}

impl Config {
    pub fn path() -> std::path::PathBuf {
        util::config_dir().join("config.json")
    }

    pub fn load() -> Config {
        let mut cfg: Config = match std::fs::read_to_string(Self::path()) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Config::default(),
        };
        // マイグレーション (旧設定からプロファイルへ)
        if cfg.api_profiles.is_empty() {
            let p1 = ApiProfile {
                name: "Gemini Default".into(),
                api_type: ApiType::Gemini,
                model_name: if cfg.gemini_model.is_empty() { "gemini-3.5-flash".into() } else { cfg.gemini_model.clone() },
                api_url: "".into(),
                api_key_enc: cfg.gemini_key_enc.clone(),
                ocr_prompt: if cfg.gemini_ocr_prompt.is_empty() { DEFAULT_GEMINI_OCR_PROMPT.into() } else { cfg.gemini_ocr_prompt.clone() },
                translate_prompt: if cfg.gemini_translate_prompt.is_empty() { DEFAULT_GEMINI_TRANSLATE_PROMPT.into() } else { cfg.gemini_translate_prompt.clone() },
                explain_prompt: if cfg.gemini_explain_prompt.is_empty() { DEFAULT_GEMINI_EXPLAIN_PROMPT.into() } else { cfg.gemini_explain_prompt.clone() },
            };
            let p2 = ApiProfile {
                name: "GPT Default".into(),
                api_type: ApiType::OpenAI,
                model_name: if cfg.gpt_model.is_empty() { "gpt-4o-mini".into() } else { cfg.gpt_model.clone() },
                api_url: if cfg.gpt_url.is_empty() { "https://api.openai.com/v1/chat/completions".into() } else { cfg.gpt_url.clone() },
                api_key_enc: cfg.gpt_key_enc.clone(),
                ocr_prompt: DEFAULT_GEMINI_OCR_PROMPT.into(),
                translate_prompt: DEFAULT_GEMINI_TRANSLATE_PROMPT.into(),
                explain_prompt: DEFAULT_GEMINI_EXPLAIN_PROMPT.into(),
            };
            cfg.api_profiles.push(p1);
            cfg.api_profiles.push(p2);
            cfg.active_api_profile = "Gemini Default".into();
        }
        cfg
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

    pub fn active_profile(&self) -> Option<&ApiProfile> {
        self.api_profiles.iter().find(|p| p.name == self.active_api_profile)
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
            "llm" => {
                if let Some(p) = self.active_profile() {
                    !p.get_key().is_empty()
                } else {
                    false
                }
            }
            "local" => crate::onnx_translate_install::installed(
                crate::onnx_translate_install::Variant::from_key(&self.local_model_variant),
            ),
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
