// 翻訳エンジン群 (SPEC §7.2)
// - local:  ローカルONNX翻訳(既定)。モデル未導入時はエラーを返す。
// - deepl / google / gemini: クラウドREST。失敗時は local へフォールバック。
// 結果はメモリ内キャッシュ (SPEC: キャッシュヒット時 100〜200ms台)。
use crate::config::Config;
use crate::util;
use std::collections::HashMap;
use std::sync::Mutex;

/// キャッシュキー: (エンジン, 訳先言語, 原文)
type CacheKey = (String, String, String);
static CACHE: Mutex<Option<HashMap<CacheKey, String>>> = Mutex::new(None);
const CACHE_MAX: usize = 500;

/// 訳文と補足バッジ(フォールバック発生時など)を返す
pub struct Translated {
    pub text: String,
    pub badge: Option<String>,
}

/// ローカル翻訳モデルの有無 (%APPDATA%\FocusTranslator\models\onnx_translate\)
pub fn local_model_available() -> bool {
    crate::onnx_translate_install::installed()
}

/// 原文から訳先言語を決める(CJK原文なら英語へ、それ以外は設定言語へ)
fn decide_target(cfg: &Config, text: &str) -> String {
    if util::contains_cjk(text) && cfg.target_lang == "ja" {
        "en".into()
    } else {
        cfg.target_lang.clone()
    }
}

/// 指定エンジンで翻訳。クラウド失敗時は local へフォールバック (SPEC §11)。
pub fn translate(engine: &str, cfg: &Config, text: &str) -> Result<Translated, String> {
    let target = decide_target(cfg, text);
    let key = (engine.to_string(), target.clone(), text.to_string());

    // キャッシュ確認
    {
        let mut guard = CACHE.lock().unwrap();
        let map = guard.get_or_insert_with(HashMap::new);
        if let Some(hit) = map.get(&key) {
            return Ok(Translated { text: hit.clone(), badge: Some("cache".into()) });
        }
    }

    let result = translate_once(engine, cfg, text, &target);
    match result {
        Ok(t) => {
            let mut guard = CACHE.lock().unwrap();
            let map = guard.get_or_insert_with(HashMap::new);
            if map.len() >= CACHE_MAX {
                map.clear();
            }
            map.insert(key, t.clone());
            Ok(Translated { text: t, badge: None })
        }
        Err(e) if engine != "local" => {
            // クラウド翻訳失敗 → ローカルへフォールバックし local バッジ表示
            match translate_once("local", cfg, text, &target) {
                Ok(t) => Ok(Translated { text: t, badge: Some("local".into()) }),
                Err(_) => Err(e),
            }
        }
        Err(e) => Err(e),
    }
}

fn translate_once(engine: &str, cfg: &Config, text: &str, target: &str) -> Result<String, String> {
    match engine {
        "local" => translate_local(text, target),
        "deepl" => translate_deepl(cfg, text, target),
        "google" => translate_google(cfg, text, target),
        "gemini" => translate_gemini(cfg, text, target),
        other => Err(format!("不明な翻訳エンジン: {other}")),
    }
}

/// ローカルONNX翻訳 (opus-mt-ja-en / opus-mt-en-jap, ort によるONNX Runtime推論)。
/// モデル導入(ダウンロード)は onnx_translate_install、推論本体は onnx_translate を参照。
fn translate_local(text: &str, target: &str) -> Result<String, String> {
    crate::onnx_translate::translate(text, target == "ja")
}

fn translate_deepl(cfg: &Config, text: &str, target: &str) -> Result<String, String> {
    let key = cfg.deepl_key();
    if key.is_empty() {
        return Err("DeepL APIキーが未設定です".into());
    }
    // ":fx" で終わるキーは Free プラン
    let host = if key.ends_with(":fx") { "api-free.deepl.com" } else { "api.deepl.com" };
    let url = format!("https://{host}/v2/translate");
    let body = serde_json::json!({
        "text": [text],
        "target_lang": target.to_uppercase(),
    });
    let mut res = ureq::post(&url)
        .header("Authorization", format!("DeepL-Auth-Key {key}"))
        .send_json(&body)
        .map_err(|e| format!("DeepL呼び出し失敗: {e}"))?;
    let v: serde_json::Value =
        res.body_mut().read_json().map_err(|e| format!("DeepL応答解析失敗: {e}"))?;
    v["translations"][0]["text"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or("DeepL応答に訳文がありません".into())
}

fn translate_google(cfg: &Config, text: &str, target: &str) -> Result<String, String> {
    let key = cfg.google_key();
    if key.is_empty() {
        return Err("Google APIキーが未設定です".into());
    }
    let url = format!("https://translation.googleapis.com/language/translate/v2?key={key}");
    let body = serde_json::json!({ "q": text, "target": target, "format": "text" });
    let mut res = ureq::post(&url)
        .send_json(&body)
        .map_err(|e| format!("Google翻訳呼び出し失敗: {e}"))?;
    let v: serde_json::Value =
        res.body_mut().read_json().map_err(|e| format!("Google応答解析失敗: {e}"))?;
    v["data"]["translations"][0]["translatedText"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or("Google応答に訳文がありません".into())
}

fn translate_gemini(cfg: &Config, text: &str, target: &str) -> Result<String, String> {
    let key = cfg.gemini_key();
    if key.is_empty() {
        return Err("Gemini APIキーが未設定です".into());
    }
    let target_name = if target == "en" { "English" } else { "Japanese" };
    let prompt =
        format!("Translate the following text to {target_name}. Output only the translation.\n\n{text}");
    let body = serde_json::json!({
        "contents": [{ "parts": [{ "text": prompt }] }]
    });
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
        cfg.gemini_model
    );
    let mut res = ureq::post(&url)
        .header("x-goog-api-key", &key)
        .send_json(&body)
        .map_err(|e| format!("Gemini呼び出し失敗: {e}"))?;
    let v: serde_json::Value =
        res.body_mut().read_json().map_err(|e| format!("Gemini応答解析失敗: {e}"))?;
    v["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or("Gemini応答に訳文がありません".into())
}
