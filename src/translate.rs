// 翻訳エンジン群 (SPEC §7.2)
// - local:  ローカルONNX翻訳(既定)。モデル未導入時はエラーを返す。
// - deepl / google / llm: クラウドREST。失敗時は local へフォールバック。
// 結果はメモリ内キャッシュ (SPEC: キャッシュヒット時 100〜200ms台)。
// ログDB用に送受信JSON・トークン・言語・実際に使ったエンジンも返す。
use crate::config::Config;
use std::collections::HashMap;
use std::sync::Mutex;

/// キャッシュキー: (エンジン, 訳先言語, 原文)
type CacheKey = (String, String, String);
static CACHE: Mutex<Option<HashMap<CacheKey, String>>> = Mutex::new(None);
const CACHE_MAX: usize = 500;

/// クラウドREST呼び出しの詳細(ログDB用)
#[derive(Default, Clone)]
pub struct TransDetail {
    pub request_json: Option<String>,
    pub response_json: Option<String>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
}

/// 訳文と補足バッジ(フォールバック発生時など)+ ログ用メタ情報を返す
pub struct Translated {
    pub text: String,
    pub badge: Option<String>,
    /// 実際に使ったエンジン(クラウド失敗時は "local" に変わる)
    pub engine: String,
    pub source_lang: String,
    pub target_lang: String,
    pub cache_hit: bool,
    pub detail: TransDetail,
}

/// 翻訳方向 (source, target) を決める。常に設定通り(cfg.source_lang → cfg.target_lang)に固定し、
/// 原文の内容による自動判定・反転は行わない。
/// request/response JSON に含まれうる設定済みAPIキーを伏字化する (SPEC §2.4)
pub(crate) fn mask_keys(cfg: &Config, s: &str) -> String {
    let mut out = s.to_string();
    for k in [cfg.deepl_key(), cfg.google_key()] {
        if k.len() >= 8 {
            out = out.replace(&k, "***MASKED***");
        }
    }
    for p in &cfg.api_profiles {
        let k = p.get_key();
        if k.len() >= 8 {
            out = out.replace(&k, "***MASKED***");
        }
    }
    out
}

/// 指定エンジンで翻訳。クラウド失敗時は local へフォールバック (SPEC §11)。
pub fn translate(engine: &str, cfg: &Config, text: &str) -> Result<Translated, String> {
    let (source, target) = (cfg.source_lang.clone(), cfg.target_lang.clone());
    let key = (engine.to_string(), target.clone(), text.to_string());

    // キャッシュ確認
    {
        let mut guard = CACHE.lock().unwrap();
        let map = guard.get_or_insert_with(HashMap::new);
        if let Some(hit) = map.get(&key) {
            return Ok(Translated {
                text: hit.clone(),
                badge: Some("cache".into()),
                engine: engine.into(),
                source_lang: source,
                target_lang: target,
                cache_hit: true,
                detail: TransDetail::default(),
            });
        }
    }

    let result = translate_once(engine, cfg, text, &target);
    match result {
        Ok((t, detail)) => {
            let mut guard = CACHE.lock().unwrap();
            let map = guard.get_or_insert_with(HashMap::new);
            if map.len() >= CACHE_MAX {
                map.clear();
            }
            map.insert(key, t.clone());
            Ok(Translated {
                text: t,
                badge: None,
                engine: engine.into(),
                source_lang: source,
                target_lang: target,
                cache_hit: false,
                detail,
            })
        }
        Err(e) if engine != "local" => {
            // クラウド翻訳失敗 → ローカルへフォールバックし local バッジ表示
            match translate_once("local", cfg, text, &target) {
                Ok((t, detail)) => Ok(Translated {
                    text: t,
                    badge: Some("local".into()),
                    engine: "local".into(),
                    source_lang: source,
                    target_lang: target,
                    cache_hit: false,
                    detail,
                }),
                Err(_) => Err(e),
            }
        }
        Err(e) => Err(e),
    }
}

fn translate_once(
    engine: &str,
    cfg: &Config,
    text: &str,
    target: &str,
) -> Result<(String, TransDetail), String> {
    match engine {
        "local" => translate_local(cfg, text, target).map(|t| (t, TransDetail::default())),
        "deepl" => translate_deepl(cfg, text, target),
        "google" => translate_google(cfg, text, target),
        // LLMの翻訳方向はプロンプトテンプレート側で cfg から埋める
        "llm" => translate_llm(cfg, text),
        other => Err(format!("不明な翻訳エンジン: {other}")),
    }
}

/// ローカルONNX翻訳 (Opus-MT / FuguMT / NLLB-200 のいずれか、ort によるONNX Runtime推論)。
fn translate_local(cfg: &Config, text: &str, target: &str) -> Result<String, String> {
    let variant = crate::onnx_translate_install::Variant::from_key(&cfg.local_model_variant);
    crate::onnx_translate::translate(text, target == "ja", variant)
}

fn translate_deepl(cfg: &Config, text: &str, target: &str) -> Result<(String, TransDetail), String> {
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
    let req_json = mask_keys(cfg, &body.to_string());
    let mut res = ureq::post(&url)
        .header("Authorization", format!("DeepL-Auth-Key {key}"))
        .send_json(&body)
        .map_err(|e| format!("DeepL呼び出し失敗: {e}"))?;
    let v: serde_json::Value =
        res.body_mut().read_json().map_err(|e| format!("DeepL応答解析失敗: {e}"))?;
    let detail = TransDetail {
        request_json: Some(req_json),
        response_json: Some(mask_keys(cfg, &v.to_string())),
        tokens_in: None,
        tokens_out: None,
    };
    v["translations"][0]["text"]
        .as_str()
        .map(|s| (s.to_string(), detail))
        .ok_or("DeepL応答に訳文がありません".into())
}

fn translate_google(cfg: &Config, text: &str, target: &str) -> Result<(String, TransDetail), String> {
    let key = cfg.google_key();
    if key.is_empty() {
        return Err("Google APIキーが未設定です".into());
    }
    let url = format!("https://translation.googleapis.com/language/translate/v2?key={key}");
    let body = serde_json::json!({ "q": text, "target": target, "format": "text" });
    let req_json = mask_keys(cfg, &body.to_string());
    let mut res = ureq::post(&url)
        .send_json(&body)
        .map_err(|e| format!("Google翻訳呼び出し失敗: {e}"))?;
    let v: serde_json::Value =
        res.body_mut().read_json().map_err(|e| format!("Google応答解析失敗: {e}"))?;
    let detail = TransDetail {
        request_json: Some(req_json),
        response_json: Some(mask_keys(cfg, &v.to_string())),
        tokens_in: None,
        tokens_out: None,
    };
    v["data"]["translations"][0]["translatedText"]
        .as_str()
        .map(|s| (s.to_string(), detail))
        .ok_or("Google応答に訳文がありません".into())
}

/// アクティブなLLMプロファイルで翻訳 (プロバイダ差異は llm_api が吸収)
fn translate_llm(cfg: &Config, text: &str) -> Result<(String, TransDetail), String> {
    let prof = cfg.active_profile().ok_or("LLM APIプロファイルが設定されていません")?;
    let prompt = cfg.fill_prompt(&prof.translate_prompt, text);
    let res = crate::llm_api::call(prof, &crate::llm_api::LlmRequest::text(&prompt))?;
    let detail = TransDetail {
        request_json: Some(mask_keys(cfg, &res.request_json)),
        response_json: Some(mask_keys(cfg, &res.response_json)),
        tokens_in: res.tokens_in,
        tokens_out: res.tokens_out,
    };
    Ok((res.text, detail))
}

