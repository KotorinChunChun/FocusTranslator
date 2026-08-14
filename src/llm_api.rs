// LLM API (Gemini / OpenAI互換 / Claude) 共通のREST呼び出し
// 翻訳 / OCR統合 / 解説の各機能はここを経由してプロバイダ差異を吸収する。
use crate::config::{ApiProfile, ApiType};

pub const DEFAULT_OPENAI_URL: &str = "https://api.openai.com/v1/chat/completions";
pub const DEFAULT_CLAUDE_URL: &str = "https://api.anthropic.com/v1/messages";
pub const GEMINI_URL_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";
/// Hugging Face Inference Providers の統一(OpenAI互換)ルーター (SPECv0.5.5)
pub const DEFAULT_HUGGINGFACE_URL: &str = "https://router.huggingface.co/v1/chat/completions";
/// OpenRouter のOpenAI互換API (SPECv0.5.5)
pub const DEFAULT_OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
/// GitHub Models のOpenAI互換API (SPECv0.5.5)
pub const DEFAULT_GITHUB_MODELS_URL: &str = "https://models.github.ai/inference/chat/completions";
/// NVIDIA NIM (build.nvidia.com API カタログ) のOpenAI互換API (SPECv0.5.5)
pub const DEFAULT_NVIDIA_NIM_URL: &str = "https://integrate.api.nvidia.com/v1/chat/completions";
/// Ollamaのローカルサーバ (OpenAI互換API)。APIキー不要 (SPECv0.5.5)
pub const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434/v1/chat/completions";
/// LM Studioのローカルサーバ (OpenAI互換API)。APIキー不要 (SPECv0.5.5)
pub const DEFAULT_LMSTUDIO_URL: &str = "http://localhost:1234/v1/chat/completions";

/// LLMへの1リクエスト。画像はOCR・翻訳・解説で必要に応じて付与する。
pub struct LlmRequest<'a> {
    pub prompt: &'a str,
    /// PNG画像 (base64)
    pub image_png_b64: Option<&'a str>,
    /// 構造化JSON応答を要求 (Gemini/OpenAIのみAPIレベルで指定可。Claudeはプロンプト側で指示)
    pub json_mode: bool,
}

#[cfg(test)]
impl<'a> LlmRequest<'a> {
    pub fn text(prompt: &'a str) -> Self {
        Self {
            prompt,
            image_png_b64: None,
            json_mode: false,
        }
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
    if prof.api_type.is_cli() {
        return crate::llm_cli::call(prof, req);
    }
    let key = prof.get_key();
    // APIキー要否はチップ表示判定 (ApiProfile::is_ready) と同一基準に統一する。
    // localhost判定だと、LAN上のサーバ等がチップ有効なのに呼び出しで失敗する齟齬が生じる。
    if key.is_empty() && prof.requires_key() {
        return Err(format!("APIキーが未設定です ({})", prof.name));
    }
    match prof.api_type {
        ApiType::Gemini => call_gemini(prof, &key, req),
        ApiType::OpenAI
        | ApiType::LlamaCpp
        | ApiType::HuggingFace
        | ApiType::OpenRouter
        | ApiType::GitHubModels
        | ApiType::NvidiaNim
        | ApiType::Ollama
        | ApiType::LmStudio => call_openai(prof, &key, req),
        ApiType::Claude => call_claude(prof, &key, req),
        ApiType::CodexCli
        | ApiType::ClaudeCli
        | ApiType::CopilotCli
        | ApiType::GeminiCli
        | ApiType::KimiCli => unreachable!("CLI種別は上で処理済み"),
    }
}

/// 送信ボディJSON文字列のみを組み立てる (APIキーは含まない: ヘッダーで送るため)。
/// 実送信前にDBキャッシュを検索するために使う (SPECv0.4.8追補: 翻訳APIキャッシュ)。
pub fn build_request_json(prof: &ApiProfile, req: &LlmRequest) -> String {
    if prof.api_type.is_cli() {
        return crate::llm_cli::build_request_json(prof, req);
    }
    match prof.api_type {
        ApiType::Gemini => gemini_body(prof, req).to_string(),
        ApiType::OpenAI
        | ApiType::LlamaCpp
        | ApiType::HuggingFace
        | ApiType::OpenRouter
        | ApiType::GitHubModels
        | ApiType::NvidiaNim
        | ApiType::Ollama
        | ApiType::LmStudio => openai_body(prof, req).to_string(),
        ApiType::Claude => claude_body(prof, req).to_string(),
        ApiType::CodexCli
        | ApiType::ClaudeCli
        | ApiType::CopilotCli
        | ApiType::GeminiCli
        | ApiType::KimiCli => unreachable!("CLI種別は上で処理済み"),
    }
}

fn gemini_body(prof: &ApiProfile, req: &LlmRequest) -> serde_json::Value {
    let mut parts = vec![serde_json::json!({ "text": req.prompt })];
    if let Some(b64) = req.image_png_b64 {
        parts.push(serde_json::json!({ "inlineData": { "mimeType": "image/png", "data": b64 } }));
    }
    let mut body = serde_json::json!({ "contents": [{ "parts": parts }] });
    let mut gen_cfg = serde_json::Map::new();
    if req.json_mode {
        gen_cfg.insert(
            "responseMimeType".into(),
            serde_json::json!("application/json"),
        );
    }
    // 最大応答トークン数 (SPECv0.5.3: プロファイル設定。0 ならプロバイダ既定に任せる)
    if prof.max_tokens > 0 {
        gen_cfg.insert("maxOutputTokens".into(), serde_json::json!(prof.max_tokens));
    }
    if !gen_cfg.is_empty() {
        body["generationConfig"] = serde_json::Value::Object(gen_cfg);
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
    // 最大応答トークン数 (SPECv0.5.3: プロファイル設定。0 ならプロバイダ既定に任せる)
    if prof.max_tokens > 0 {
        body["max_tokens"] = serde_json::json!(prof.max_tokens);
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
    // Claude API は max_tokens が必須のため、未設定(0)でも既定値を適用する (SPECv0.5.3)
    let max_tokens = if prof.max_tokens > 0 {
        prof.max_tokens
    } else {
        crate::config::DEFAULT_MAX_TOKENS
    };
    serde_json::json!({
        "model": prof.model_name,
        "max_tokens": max_tokens,
        "messages": [{ "role": "user", "content": content }]
    })
}

/// API/CLIで現在利用可能なモデルを取得する。OpenAI互換APIに加え、
/// Gemini/Claude固有APIと各CLIの一覧・候補取得をプロバイダ別に処理する。
pub struct ModelListResult {
    pub model_ids: Vec<String>,
    /// CLI検出など、モデル一覧以外の成功内容を設定画面へ表示する場合に使う。
    pub detail: Option<String>,
}

pub fn check_connection(prof: &ApiProfile) -> Result<ModelListResult, String> {
    if prof.api_type.is_cli() {
        return crate::llm_cli::check_connection(prof).map(|(detail, model_ids)| ModelListResult {
            model_ids,
            detail: Some(detail),
        });
    }
    if prof.api_type == ApiType::Gemini {
        return list_gemini_models(prof);
    }
    if prof.api_type == ApiType::Claude {
        return list_claude_models(prof);
    }
    let base = if prof.api_url.trim().is_empty() {
        prof.api_type.default_url()
    } else {
        prof.api_url.trim()
    };
    let models_url = match base.strip_suffix("/chat/completions") {
        Some(b) => format!("{b}/models"),
        None => base.to_string(),
    };
    let key = prof.get_key();
    let auth = format!("Bearer {key}");
    let mut builder = ureq::get(&models_url);
    if !key.is_empty() {
        builder = builder.header("Authorization", auth.as_str());
    }
    let mut res = builder
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(6)))
        .build()
        .call()
        .map_err(|e| format!("接続に失敗しました: {e}"))?;
    let json: serde_json::Value = res
        .body_mut()
        .read_json()
        .map_err(|e| format!("応答の解析に失敗しました: {e}"))?;
    let model_ids: Vec<String> = json["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let mut model_ids = model_ids;
    sort_model_ids(&mut model_ids);
    Ok(ModelListResult {
        model_ids,
        detail: None,
    })
}

fn list_gemini_models(prof: &ApiProfile) -> Result<ModelListResult, String> {
    let key = prof.get_key();
    if key.is_empty() {
        return Err("Gemini APIキーが未設定です".into());
    }
    let url = format!("{}?key={key}", GEMINI_URL_BASE);
    let mut res = ureq::get(&url)
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(6)))
        .build()
        .call()
        .map_err(|e| format!("接続に失敗しました: {e}"))?;
    let json: serde_json::Value = res
        .body_mut()
        .read_json()
        .map_err(|e| format!("応答の解析に失敗しました: {e}"))?;
    let mut model_ids = json["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| {
            m["supportedGenerationMethods"]
                .as_array()
                .is_none_or(|methods| {
                    methods
                        .iter()
                        .any(|v| v.as_str() == Some("generateContent"))
                })
        })
        .filter_map(|m| {
            m["name"]
                .as_str()?
                .strip_prefix("models/")
                .map(str::to_string)
        })
        .collect();
    sort_model_ids(&mut model_ids);
    Ok(ModelListResult {
        model_ids,
        detail: None,
    })
}

fn list_claude_models(prof: &ApiProfile) -> Result<ModelListResult, String> {
    let key = prof.get_key();
    if key.is_empty() {
        return Err("Claude APIキーが未設定です".into());
    }
    let endpoint = prof.api_url.trim();
    let base = if endpoint.is_empty() {
        "https://api.anthropic.com/v1"
    } else {
        endpoint
            .strip_suffix("/messages")
            .unwrap_or(endpoint)
            .trim_end_matches('/')
    };
    let url = if base.ends_with("/models") {
        base.to_string()
    } else {
        format!("{base}/models")
    };
    let mut res = ureq::get(&url)
        .header("x-api-key", &key)
        .header("anthropic-version", "2023-06-01")
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(6)))
        .build()
        .call()
        .map_err(|e| format!("接続に失敗しました: {e}"))?;
    let json: serde_json::Value = res
        .body_mut()
        .read_json()
        .map_err(|e| format!("応答の解析に失敗しました: {e}"))?;
    let mut model_ids = json["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect();
    sort_model_ids(&mut model_ids);
    Ok(ModelListResult {
        model_ids,
        detail: None,
    })
}

fn sort_model_ids(models: &mut Vec<String>) {
    models.sort_by_key(|s| s.to_ascii_lowercase());
    models.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
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
    let mut res = req
        .send_json(body)
        .map_err(|e| format!("{label}呼び出し失敗: {e}"))?;
    res.body_mut()
        .read_json()
        .map_err(|e| format!("{label}応答解析失敗: {e}"))
}

fn usage_i64(v: &serde_json::Value, obj: &str, field: &str) -> Option<i64> {
    v.get(obj)
        .and_then(|u| u.get(field))
        .and_then(|t| t.as_i64())
}

fn call_gemini(prof: &ApiProfile, key: &str, req: &LlmRequest) -> Result<LlmResponse, String> {
    let body = gemini_body(prof, req);
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
