// 設定の永続化 (%APPDATA%\FocusTranslator\config.json)
// APIキーは DPAPI で暗号化した base64 を保存する。
use crate::util;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub enum ApiType {
    #[default]
    Gemini,
    OpenAI,
    Claude,
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

/// Gemini/Claude/ChatGPTの公式APIエンドポイント。これらのURLで呼び出す場合はAPIキーが必須。
/// (ローカルサーバ等、既定と異なるURLを指すプロファイルはAPIキー空欄でも呼び出しを許容する)
const MAJOR_API_URLS: [&str; 3] = [
    crate::llm_api::GEMINI_URL_BASE,
    crate::llm_api::DEFAULT_OPENAI_URL,
    crate::llm_api::DEFAULT_CLAUDE_URL,
];

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

    /// このプロファイルの呼び出しにAPIキーが必須か。公式エンドポイント(完全一致)のみ必須で、
    /// ローカルサーバ等の非公式URLはキー空欄でも呼び出しを許容する。
    /// チップ表示判定 (is_ready) と実呼び出し (llm_api::call) の双方がこの判定を共用する。
    pub fn requires_key(&self) -> bool {
        MAJOR_API_URLS.contains(&self.api_url.trim())
    }

    /// このプロファイルで呼び出しを試みても失敗しないと分かる状態か(ボタンのグレーアウト判定用)。
    /// API URL/モデル名が未設定なら必ず失敗する。APIキーの空欄自体は許容するが、
    /// Gemini/Claude/ChatGPTの主要な公式URLではAPIキーが無いと必ず失敗するため許容しない。
    pub fn is_ready(&self) -> bool {
        if self.api_url.trim().is_empty() || self.model_name.trim().is_empty() {
            return false;
        }
        if self.get_key().is_empty() && self.requires_key() {
            return false;
        }
        true
    }
}

/// プロバイダ種別ごとの既定モデル名と既定URL (設定UIの種別切替・マイグレーションで共用)
impl ApiType {
    pub fn default_model(&self) -> &'static str {
        match self {
            ApiType::Gemini => "gemini-3.5-flash",
            ApiType::OpenAI => "gpt-4o-mini",
            ApiType::Claude => "claude-haiku-4-5-20251001",
        }
    }
    pub fn default_url(&self) -> &'static str {
        match self {
            ApiType::Gemini => crate::llm_api::GEMINI_URL_BASE,
            ApiType::OpenAI => crate::llm_api::DEFAULT_OPENAI_URL,
            ApiType::Claude => crate::llm_api::DEFAULT_CLAUDE_URL,
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
    /// ホールドピン留めまでの秒数 (既定: 3秒)
    pub pin_hold_seconds: u32,
    /// 範囲指定ホットキー (例: "Ctrl+Alt+T")
    pub region_hotkey: String,
    /// 既定OCRエンジン: "oneocr" | "win" | "paddle" | "llm"
    pub default_ocr: String,
    /// 既定翻訳エンジン: "local" | "deepl" | "google" | "gemini"
    pub default_translator: String,
    /// 訳先言語 (原文がCJKの場合は自動で "en" へ反転)
    pub target_lang: String,
    /// 翻訳元言語 (既定 en)
    pub source_lang: String,
    pub deepl_key_enc: String,
    pub google_key_enc: String,
    
    // API Profile設定
    pub api_profiles: Vec<ApiProfile>,
    /// セッション中に実際に使用中のLLMプロファイル (チップ切替やキャプチャ開始で変わる)。
    pub active_api_profile: String,
    /// 既定LLMプロファイル (設定画面でのみ変更)。右Ctrl起動時に active_api_profile の初期値となる。
    /// これはあくまで「起動時にどれを選ぶか」を決める役割で、現行オーバーレイには波及しない。
    /// フィールド単独の #[serde(default)] により、旧設定 (このフィールド自体が無い) を
    /// 読み込んだときは Config::default() の "Gemini" ではなく空文字列にフォールバックする。
    /// load() 側はこの空文字列を「既定プロファイル未確定 = 旧設定」の目印として使う。
    #[serde(default)]
    pub default_api_profile: String,

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
    /// 外部送信同意: テキスト送信 / 画像送信
    pub consent_text: bool,
    pub consent_image: bool,
    pub autostart: bool,
    pub perf_log: bool,
    /// 実行ログをSQLiteに記録する (既定OFF)
    pub log_enabled: bool,
    /// デバッグモード: OCR時にキャプチャ画像をPNG保存 (既定OFF)
    pub debug_mode: bool,
    /// 領域表示 (キャプチャキー側): キャプチャキー(hold_key)押下中、UIA要素や
    /// キャプチャ範囲を枠表示するデバッグ機能 (既定OFF)
    pub detect_enabled: bool,
    /// プレビューキー: 実際の翻訳は行わず、検出範囲の枠表示だけを確認できるキー
    /// (hold_key と同じ表記、既定 LCtrl)
    pub detect_key: String,
    /// 領域表示 (プレビューキー側): プレビューキー(detect_key)押下中も枠表示するか (既定OFF)
    pub preview_detect_enabled: bool,
    /// 認識ログの保持上限件数
    pub log_max_records: u32,
    /// 初回起動時のセットアップ提案ダイアログを表示済みか
    pub first_launch_done: bool,
    /// オーバーレイの配色テーマ: "system" (Windowsのアプリモードに追従) | "light" | "dark"
    pub overlay_theme: String,
}

/// 翻訳プロンプトの既定値
pub const DEFAULT_GEMINI_TRANSLATE_PROMPT: &str =
    "Translate the following text from {{source_lang}} to {{target_lang}}. Output only the translation.\n\n{{original_text}}";
/// OCR+翻訳統合プロンプトの既定値
pub const DEFAULT_GEMINI_OCR_PROMPT: &str =
    "Extract the text in this image and translate it from {{source_lang}} to {{target_lang}}. Respond with JSON only: {\"source\": \"<extracted text>\", \"translation\": \"<translation>\"}";
/// 解説プロンプトの既定値 (SPECv0.4 §7.2)
pub const DEFAULT_GEMINI_EXPLAIN_PROMPT: &str = "以下は、{{app_title}} というタイトルのWindowsアプリケーションの中で表示されているテキストです。\nこれが何か{{target_lang}}で説明してください。\nアプリのUIなら機能や用途の解説を、コンテンツなら意味の解説をお願いします。\n\n## 実行ファイル名\n{{app_exe}}\n\n## UIAパス\n{{uia_path}}\n\n## 表示テキスト\n{{original_text}}";
/// v0.3 までの解説プロンプト既定値 (設定移行の判定用。当時の文言そのままで比較する)
const OLD_EXPLAIN_PROMPT: &str =
    "Explain the grammar, nuances, and background of the following text in {{target_lang}}.\n{{glossary}}\n\n{{original_text}}";

/// 初回起動時・マイグレーション時に生成する既定LLMプロファイルの名前一覧
pub const DEFAULT_PROFILE_NAMES: [&str; 4] =
    ["Gemini", "GPT", "Claude", "LocalLLM"];

/// 既定LLMプロファイルをその名前から新規生成する (設定に同名プロファイルが無いときの
/// 初期値/バックフィル用)。
fn seed_profile(name: &str) -> ApiProfile {
    let (api_type, model_name, api_url): (ApiType, String, String) = match name {
        "Gemini" => (ApiType::Gemini, ApiType::Gemini.default_model().into(), ApiType::Gemini.default_url().into()),
        "GPT" => (ApiType::OpenAI, ApiType::OpenAI.default_model().into(), ApiType::OpenAI.default_url().into()),
        "Claude" => (ApiType::Claude, ApiType::Claude.default_model().into(), ApiType::Claude.default_url().into()),
        "LocalLLM" => (ApiType::OpenAI, "gemma4:e2b".into(), "http://localhost:11434/v1/chat/completions".into()),
        _ => (ApiType::Gemini, ApiType::Gemini.default_model().into(), ApiType::Gemini.default_url().into()),
    };
    ApiProfile {
        name: name.into(),
        api_type,
        model_name,
        api_url,
        api_key_enc: String::new(),
        ocr_prompt: DEFAULT_GEMINI_OCR_PROMPT.into(),
        translate_prompt: DEFAULT_GEMINI_TRANSLATE_PROMPT.into(),
        explain_prompt: DEFAULT_GEMINI_EXPLAIN_PROMPT.into(),
    }
}

/// プロンプトのプレースホルダ置換に使う実行時コンテキスト (SPECv0.4 §7.1)。
/// 該当しない項目 (UIA経路の ocr_engine、翻訳前の translated_text 等) は空文字のままにする。
#[derive(Default, Clone)]
pub struct PromptContext {
    /// OCRまたはUIAで取得した翻訳前の原文
    pub original_text: String,
    /// 翻訳エンジンの処理を通過した後の訳文 (翻訳前は空文字)
    pub translated_text: String,
    /// 対象アプリケーションのウィンドウタイトル
    pub app_title: String,
    /// 対象アプリケーションの実行ファイル名
    pub app_exe: String,
    /// UIA取得時の要素のパス (画像OCR時は空文字)
    pub uia_path: String,
    /// 実行されたOCRエンジン名 (UIA経路は空文字)
    pub ocr_engine: String,
    /// 実行された翻訳エンジン名 (翻訳前は空文字)
    pub tr_engine: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hold_key: "RCtrl".into(),
            poll_ms: 100,
            pin_hold_seconds: 3,
            region_hotkey: "Ctrl+Alt+T".into(),
            default_ocr: "oneocr".into(),
            default_translator: "local".into(),
            target_lang: "ja".into(),
            source_lang: "en".into(),
            deepl_key_enc: String::new(),
            google_key_enc: String::new(),
            api_profiles: Vec::new(),
            active_api_profile: "Gemini".into(),
            default_api_profile: "Gemini".into(),
            gemini_key_enc: String::new(),
            gemini_model: "gemini-3.5-flash".into(),
            gpt_url: "https://api.openai.com/v1/chat/completions".into(),
            gpt_key_enc: String::new(),
            gpt_model: "gpt-4o-mini".into(),
            gemini_translate_prompt: DEFAULT_GEMINI_TRANSLATE_PROMPT.into(),
            gemini_ocr_prompt: DEFAULT_GEMINI_OCR_PROMPT.into(),
            gemini_explain_prompt: DEFAULT_GEMINI_EXPLAIN_PROMPT.into(),
            consent_text: false,
            consent_image: false,
            autostart: false,
            perf_log: false,
            log_enabled: false,
            debug_mode: false,
            detect_enabled: false,
            detect_key: "LCtrl".into(),
            preview_detect_enabled: false,
            log_max_records: 5000,
            first_launch_done: false,
            overlay_theme: "system".into(),
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
        let mut migrated = false;
        // 既定プロファイル未確定 = このプロファイル管理の仕組み (default_api_profile) を
        // まだ経ていない旧設定。この間だけ「不足している既定プロファイルの補充」を行う。
        // 一度確定した後は、ユーザーが「Gemini」等の名前のプロファイルを削除しても
        // 二度と復活させない (でなければ削除ボタンがこの4名に対して機能しなくなる)。
        let needs_default_backfill = cfg.default_api_profile.is_empty();
        // マイグレーション (旧設定からプロファイルへ)
        if cfg.api_profiles.is_empty() {
            fn pick(v: &str, default: &str) -> String {
                if v.is_empty() { default.into() } else { v.into() }
            }
            let mut gemini = seed_profile("Gemini");
            gemini.model_name = pick(&cfg.gemini_model, ApiType::Gemini.default_model());
            gemini.api_key_enc = cfg.gemini_key_enc.clone();
            gemini.ocr_prompt = pick(&cfg.gemini_ocr_prompt, DEFAULT_GEMINI_OCR_PROMPT);
            gemini.translate_prompt = pick(&cfg.gemini_translate_prompt, DEFAULT_GEMINI_TRANSLATE_PROMPT);
            gemini.explain_prompt = pick(&cfg.gemini_explain_prompt, DEFAULT_GEMINI_EXPLAIN_PROMPT);
            cfg.api_profiles.push(gemini);

            let mut gpt = seed_profile("GPT");
            gpt.model_name = pick(&cfg.gpt_model, ApiType::OpenAI.default_model());
            gpt.api_url = pick(&cfg.gpt_url, ApiType::OpenAI.default_url());
            gpt.api_key_enc = cfg.gpt_key_enc.clone();
            cfg.api_profiles.push(gpt);

            cfg.api_profiles.push(seed_profile("Claude"));
            cfg.api_profiles.push(seed_profile("LocalLLM"));
            cfg.active_api_profile = "Gemini".into();
            migrated = true;
        }
        // 既定プロファイルの不足分を補う: 過去バージョンでは既定プロファイルの一部
        // (例: Claude / LocalLLM) しか生成されないことがあった。旧設定の移行時のみ行う
        // (needs_default_backfill を参照)。既に同名プロファイルが存在する場合は変更しない
        // (利用者の編集内容を保持する)。
        if needs_default_backfill {
            for name in DEFAULT_PROFILE_NAMES {
                if !cfg.api_profiles.iter().any(|p| p.name == name) {
                    cfg.api_profiles.push(seed_profile(name));
                    migrated = true;
                }
            }
        }
        // プレースホルダ移行 (SPECv0.4 §7.1): 旧 {{text}} を {{original_text}} へ一度きりで書き換える。
        // 旧既定の解説プロンプトのままなら、新しい既定テンプレートへ差し替える。
        // default_api_profile 未設定の旧構成は、当時の active をコピーして確定・永続化する。
        // (以後 active はセッション用に変動するため、ここで既定を固定しておく)
        if cfg.default_api_profile.is_empty() {
            cfg.default_api_profile = if cfg.active_api_profile.is_empty() {
                cfg.api_profiles.first().map(|p| p.name.clone()).unwrap_or_default()
            } else {
                cfg.active_api_profile.clone()
            };
            migrated = true;
        }
        for p in &mut cfg.api_profiles {
            for prompt in [&mut p.ocr_prompt, &mut p.translate_prompt, &mut p.explain_prompt] {
                if prompt.contains("{{text}}") {
                    *prompt = prompt.replace("{{text}}", "{{original_text}}");
                    migrated = true;
                }
            }
            if p.explain_prompt == OLD_EXPLAIN_PROMPT {
                p.explain_prompt = DEFAULT_GEMINI_EXPLAIN_PROMPT.into();
                migrated = true;
            }
            // 旧バージョンは Gemini の api_url を空欄のまま保存していた。
            // 実行時は url_or() が既定URLへ解決するため動作に支障は無いが、設定画面の
            // 表示が空欄のままになるので、ここで種別の既定URLへ補完しておく。
            if p.api_url.is_empty() {
                p.api_url = p.api_type.default_url().into();
                migrated = true;
            }
        }
        if migrated {
            cfg.save();
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

    /// オーバーレイのOCR/翻訳チップに出すプロファイル一覧。呼び出しても必ず失敗すると
    /// 分かっているもの(is_ready()==false、主にAPIキー未設定の公式API)はチップ自体を
    /// 出さない (SPECv0.5追補: 従来はグレーアウト表示だったが非表示に変更)。
    pub fn ready_api_profiles(&self) -> impl Iterator<Item = &ApiProfile> {
        self.api_profiles.iter().filter(|p| p.is_ready())
    }

    /// プロンプトテンプレートのプレースホルダ置換 (SPECv0.4 §7.1)
    pub fn fill_prompt(&self, tmpl: &str, ctx: &PromptContext) -> String {
        // 旧バージョンのテンプレートに残る {{glossary}} は空文字へ畳む(用語集機能は廃止)
        tmpl.replace("{{source_lang}}", &self.source_lang)
            .replace("{{target_lang}}", &self.target_lang)
            .replace("{{original_text}}", &ctx.original_text)
            .replace("{{translated_text}}", &ctx.translated_text)
            .replace("{{glossary}}", "")
            .replace("{{app_title}}", &ctx.app_title)
            .replace("{{app_exe}}", &ctx.app_exe)
            .replace("{{uia_path}}", &ctx.uia_path)
            .replace("{{ocr_engine}}", &ctx.ocr_engine)
            .replace("{{tr_engine}}", &ctx.tr_engine)
    }

    /// キャプチャキー(ホールドキー)の仮想キーコード
    pub fn hold_vk(&self) -> i32 {
        key_vk(&self.hold_key)
    }

    /// プレビューキーの仮想キーコード
    pub fn detect_vk(&self) -> i32 {
        key_vk(&self.detect_key)
    }

    /// 範囲指定ホットキーの (修飾キー, 仮想キー)。解析失敗時は Ctrl+Alt+T。
    pub fn region_hotkey_parsed(&self) -> (u32, u32) {
        parse_hotkey(&self.region_hotkey).unwrap_or((0x0002 | 0x0001, b'T' as u32))
    }

    /// エンジンが利用可能か(キー・URL設定の有無)
    pub fn engine_available(&self, key: &str) -> bool {
        match key {
            "oneocr" => crate::oneocr::available(),
            "win" => true,
            "paddle" => crate::paddle_install::installed(),
            "llm" => self.active_profile().is_some_and(|p| p.is_ready()),
            "local" => crate::onnx_translate_install::installed(),
            "deepl" => !self.deepl_key_enc.is_empty(),
            "google" => !self.google_key_enc.is_empty(),
            _ => false,
        }
    }
}

/// キー名(ホールドキー/検出キー共通の表記) → 仮想キーコード
fn key_vk(name: &str) -> i32 {
    match name {
        "LCtrl" => 0xA2,
        "RShift" => 0xA1,
        "RAlt" => 0xA5,
        "F8" => 0x77,
        _ => 0xA3, // RCtrl
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 過去バージョンの設定 (Gemini/GPT の2プロファイルのみ・api_url欠落・
    /// default_api_profile未導入) を読み込むと、不足していた既定プロファイル
    /// (Claude / LocalLLM) が補われ、Gemini の api_url も
    /// 補完され、default_api_profile が確定することを確認する。
    #[test]
    fn load_backfills_missing_default_profiles_and_gemini_url() {
        let _guard = crate::util::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("ft_config_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        unsafe {
            std::env::set_var("FOCUSTRANSLATOR_DATA_DIR", &tmp);
        }

        let old_json = r#"{
            "hold_key": "RCtrl",
            "poll_ms": 100,
            "pin_hold_seconds": 3,
            "region_hotkey": "Ctrl+Alt+T",
            "default_ocr": "oneocr",
            "default_translator": "local",
            "target_lang": "ja",
            "source_lang": "en",
            "api_profiles": [
                {"name": "Gemini", "api_type": "Gemini", "model_name": "gemini-3.5-flash", "api_url": "", "api_key_enc": ""},
                {"name": "GPT", "api_type": "OpenAI", "model_name": "gpt-4o-mini", "api_url": "https://api.openai.com/v1/chat/completions", "api_key_enc": ""}
            ],
            "active_api_profile": "Gemini"
        }"#;
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(Config::path(), old_json).unwrap();

        let cfg = Config::load();

        assert_eq!(cfg.api_profiles.len(), 4, "不足していた既定プロファイルが補われる");
        for name in DEFAULT_PROFILE_NAMES {
            assert!(cfg.api_profiles.iter().any(|p| p.name == name), "{name} が存在する");
        }
        let gemini = cfg.api_profiles.iter().find(|p| p.name == "Gemini").unwrap();
        assert_eq!(gemini.api_url, ApiType::Gemini.default_url(), "既存プロファイルのURL空欄が補完される");
        assert_eq!(cfg.default_api_profile, "Gemini", "旧 active を引き継いで既定が確定する");

        let _ = std::fs::remove_dir_all(&tmp);
        unsafe {
            std::env::remove_var("FOCUSTRANSLATOR_DATA_DIR");
        }
    }

    /// default_api_profile が既に確定済み (=新形式の設定) であれば、利用者が既定名の
    /// プロファイル (例: "Gemini") を削除して保存した状態を再度読み込んでも、
    /// 不足分補充ロジックによって復活しないことを確認する (設定画面の削除ボタンの前提)。
    #[test]
    fn load_does_not_resurrect_deleted_default_named_profile_once_migrated() {
        let _guard = crate::util::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("ft_config_test2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        unsafe {
            std::env::set_var("FOCUSTRANSLATOR_DATA_DIR", &tmp);
        }

        // "Gemini" を利用者が削除した後の状態を模した新形式設定 (default_api_profile 確定済み)
        let json = r#"{
            "hold_key": "RCtrl",
            "poll_ms": 100,
            "pin_hold_seconds": 3,
            "region_hotkey": "Ctrl+Alt+T",
            "default_ocr": "oneocr",
            "default_translator": "llm",
            "target_lang": "ja",
            "source_lang": "en",
            "api_profiles": [
                {"name": "GPT", "api_type": "OpenAI", "model_name": "gpt-4o-mini", "api_url": "https://api.openai.com/v1/chat/completions", "api_key_enc": ""},
                {"name": "Claude", "api_type": "Claude", "model_name": "claude-haiku-4-5-20251001", "api_url": "https://api.anthropic.com/v1/messages", "api_key_enc": ""},
                {"name": "LocalLLM", "api_type": "OpenAI", "model_name": "gemma4:e2b", "api_url": "http://localhost:11434/v1/chat/completions", "api_key_enc": ""}
            ],
            "active_api_profile": "GPT",
            "default_api_profile": "GPT"
        }"#;
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(Config::path(), json).unwrap();

        let cfg = Config::load();

        assert_eq!(cfg.api_profiles.len(), 3, "削除済みプロファイルは復活しない");
        assert!(!cfg.api_profiles.iter().any(|p| p.name == "Gemini"), "Gemini は復活しない");
        assert_eq!(cfg.default_api_profile, "GPT", "既定は変更されない");

        let _ = std::fs::remove_dir_all(&tmp);
        unsafe {
            std::env::remove_var("FOCUSTRANSLATOR_DATA_DIR");
        }
    }
}
