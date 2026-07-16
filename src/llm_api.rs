// LLM API (Gemini / OpenAI互換 / Claude) 共通のREST呼び出し
// 翻訳 / OCR統合 / 解説の各機能はここを経由してプロバイダ差異を吸収する。
use crate::config::{ApiProfile, ApiType};

pub const DEFAULT_OPENAI_URL: &str = "https://api.openai.com/v1/chat/completions";
pub const DEFAULT_CLAUDE_URL: &str = "https://api.anthropic.com/v1/messages";
pub const GEMINI_URL_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";
/// Claude API は max_tokens が必須
const CLAUDE_MAX_TOKENS: u32 = 1024;

/// LLMへの1リクエスト。画像はOCR統合モードのみ付与する。
pub struct LlmRequest<'a> {
    pub prompt: &'a str,
    /// PNG画像 (base64)
    pub image_png_b64: Option<&'a str>,
    /// 構造化JSON応答を要求 (Gemini/OpenAIのみAPIレベルで指定可。Claudeはプロンプト側で指示)
    pub json_mode: bool,
}

impl<'a> LlmRequest<'a> {
    pub fn text(prompt: &'a str) -> Self {
        LlmRequest { prompt, image_png_b64: None, json_mode: false }
    }
}

pub struct LlmResponse {
    pub text: String,
    /// 送信ボディJSON (キー未マスク。ログ保存前に translate::mask_keys を通すこと)
    pub request_json: String,
    /// 生応答JSON (同上)
    pub response_json: String,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
}

/// プロファイルのプロバイダ種別に応じてリクエストを組み立てて送信し、
/// 応答本文テキストとトークン数を取り出す。
pub fn call(prof: &ApiProfile, req: &LlmRequest) -> Result<LlmResponse, String> {
    let key = prof.get_key();
    let is_local = prof.api_url.contains("localhost") || prof.api_url.contains("127.0.0.1");
    if key.is_empty() && !is_local {
        return Err(format!("APIキーが未設定です ({})", prof.name));
    }
    match prof.api_type {
        ApiType::Gemini => call_gemini(prof, &key, req),
        ApiType::OpenAI => call_openai(prof, &key, req),
        ApiType::Claude => call_claude(prof, &key, req),
    }
}

/// 送信ボディJSON文字列のみを組み立てる (APIキーは含まない: ヘッダーで送るため)。
/// 実送信前にDBキャッシュを検索するために使う (SPECv0.4.8追補: 翻訳APIキャッシュ)。
pub fn build_request_json(prof: &ApiProfile, req: &LlmRequest) -> String {
    match prof.api_type {
        ApiType::Gemini => gemini_body(req).to_string(),
        ApiType::OpenAI => openai_body(prof, req).to_string(),
        ApiType::Claude => claude_body(prof, req).to_string(),
    }
}

fn gemini_body(req: &LlmRequest) -> serde_json::Value {
    let mut parts = vec![serde_json::json!({ "text": req.prompt })];
    if let Some(b64) = req.image_png_b64 {
        parts.push(serde_json::json!({ "inlineData": { "mimeType": "image/png", "data": b64 } }));
    }
    let mut body = serde_json::json!({ "contents": [{ "parts": parts }] });
    if req.json_mode {
        body["generationConfig"] = serde_json::json!({ "responseMimeType": "application/json" });
    }
    body
}

fn openai_body(prof: &ApiProfile, req: &LlmRequest) -> serde_json::Value {
    let content = match req.image_png_b64 {
        Some(b64) => serde_json::json!([
            { "type": "text", "text": req.prompt },
            { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{b64}") } }
        ]),
        None => serde_json::json!(req.prompt),
    };
    let mut body = serde_json::json!({
        "model": prof.model_name,
        "messages": [{ "role": "user", "content": content }]
    });
    if req.json_mode {
        body["response_format"] = serde_json::json!({ "type": "json_object" });
    }
    body
}

fn claude_body(prof: &ApiProfile, req: &LlmRequest) -> serde_json::Value {
    let content = match req.image_png_b64 {
        Some(b64) => serde_json::json!([
            { "type": "text", "text": req.prompt },
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": b64 } }
        ]),
        None => serde_json::json!(req.prompt),
    };
    serde_json::json!({
        "model": prof.model_name,
        "max_tokens": CLAUDE_MAX_TOKENS,
        "messages": [{ "role": "user", "content": content }]
    })
}

fn url_or<'a>(url: &'a str, default: &'a str) -> &'a str {
    if url.is_empty() { default } else { url }
}

/// POST + JSON応答解析。label はエラーメッセージ用のプロバイダ表示名。
fn post_json(
    url: &str,
    headers: &[(&str, &str)],
    body: &serde_json::Value,
    label: &str,
) -> Result<serde_json::Value, String> {
    let mut req = ureq::post(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let mut res = req.send_json(body).map_err(|e| format!("{label}呼び出し失敗: {e}"))?;
    res.body_mut().read_json().map_err(|e| format!("{label}応答解析失敗: {e}"))
}

fn usage_i64(v: &serde_json::Value, obj: &str, field: &str) -> Option<i64> {
    v.get(obj).and_then(|u| u.get(field)).and_then(|t| t.as_i64())
}

fn call_gemini(prof: &ApiProfile, key: &str, req: &LlmRequest) -> Result<LlmResponse, String> {
    let body = gemini_body(req);
    let base = url_or(&prof.api_url, GEMINI_URL_BASE);
    let url = format!("{base}/{}:generateContent", prof.model_name);
    let request_json = body.to_string();
    let v = post_json(&url, &[("x-goog-api-key", key)], &body, "Gemini")?;
    let text = v["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .ok_or("Gemini応答にテキストがありません")?
        .trim()
        .to_string();
    Ok(LlmResponse {
        text,
        request_json,
        response_json: v.to_string(),
        tokens_in: usage_i64(&v, "usageMetadata", "promptTokenCount"),
        tokens_out: usage_i64(&v, "usageMetadata", "candidatesTokenCount"),
    })
}

fn call_openai(prof: &ApiProfile, key: &str, req: &LlmRequest) -> Result<LlmResponse, String> {
    let body = openai_body(prof, req);
    let url = url_or(&prof.api_url, DEFAULT_OPENAI_URL);
    let request_json = body.to_string();
    let auth = format!("Bearer {key}");
    let v = post_json(url, &[("Authorization", &auth)], &body, "GPT互換API")?;
    let text = v["choices"][0]["message"]["content"]
        .as_str()
        .ok_or("GPT応答にテキストがありません")?
        .trim()
        .to_string();
    Ok(LlmResponse {
        text,
        request_json,
        response_json: v.to_string(),
        tokens_in: usage_i64(&v, "usage", "prompt_tokens"),
        tokens_out: usage_i64(&v, "usage", "completion_tokens"),
    })
}

fn call_claude(prof: &ApiProfile, key: &str, req: &LlmRequest) -> Result<LlmResponse, String> {
    let body = claude_body(prof, req);
    let url = url_or(&prof.api_url, DEFAULT_CLAUDE_URL);
    let request_json = body.to_string();
    let v = post_json(
        url,
        &[("x-api-key", key), ("anthropic-version", "2023-06-01")],
        &body,
        "Claude API",
    )?;
    let text = v["content"][0]["text"]
        .as_str()
        .ok_or("Claude応答にテキストがありません")?
        .trim()
        .to_string();
    Ok(LlmResponse {
        text,
        request_json,
        response_json: v.to_string(),
        tokens_in: usage_i64(&v, "usage", "input_tokens"),
        tokens_out: usage_i64(&v, "usage", "output_tokens"),
    })
}
