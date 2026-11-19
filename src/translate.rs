// 翻訳エンジン群 (SPEC §7.2)
// - local:  ローカルONNX翻訳(既定)。モデル未導入時はエラーを返す。
// - deepl / google / llm: クラウドREST。失敗時は local へフォールバックしない (誤って
//   別エンジンの翻訳結果を表示すると利用者が気づけないため、エラーをそのまま提示する)。
// 結果はメモリ内キャッシュ (SPEC: キャッシュヒット時 100〜200ms台)。
// ログDB用に送受信JSON・トークン・言語・実際に使ったエンジンも返す。
use crate::config::Config;
use percent_encoding::percent_decode_str;
use regex::Regex;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// キャッシュキー: (エンジン, プロファイル, 翻訳元言語, 訳先言語, 原文, 添付画像hash)。
/// LLM翻訳プロンプトは {{source_lang}} を含むため、元言語もキーに含める
/// (SPECv0.5.3: 元言語だけを変更した直後に古い訳文が返るのを防ぐ)。
type CacheKey = (String, Option<String>, String, String, String, String);
static CACHE: Mutex<Option<HashMap<CacheKey, String>>> = Mutex::new(None);
const CACHE_MAX: usize = 500;

/// `%HH` が2組以上連続する箇所を、パーセントエンコーディングされた文字列とみなす。
/// 単独の `%20` などは通常の文章にも現れうるため対象外とする。
static PERCENT_ENCODED_RE: OnceLock<Regex> = OnceLock::new();

/// パーセントエンコーディングを含む文字列なら、文字列全体をUTF-8としてデコードする。
/// 検知条件を満たさない場合や、デコード後が不正なUTF-8になる場合は None を返す。
pub fn decode_percent_encoded(text: &str) -> Option<String> {
    let re = PERCENT_ENCODED_RE
        .get_or_init(|| Regex::new(r"(?:%[0-9A-Fa-f]{2}){2}").expect("固定正規表現は有効"));
    if !re.is_match(text) {
        return None;
    }

    percent_decode_str(text)
        .decode_utf8()
        .ok()
        .map(|decoded| decoded.into_owned())
}

/// クラウドREST呼び出しの詳細(ログDB用)
#[derive(Default, Clone)]
pub struct TransDetail {
    pub request_json: Option<String>,
    pub response_json: Option<String>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
}

/// 訳文と補足バッジ+ ログ用メタ情報を返す
#[derive(Clone)]
pub struct Translated {
    pub text: String,
    pub badge: Option<String>,
    /// 実際に使ったエンジン
    pub engine: String,
    pub source_lang: String,
    pub target_lang: String,
    pub cache_hit: bool,
    pub detail: TransDetail,
    /// DBのログ上で再利用された場合に元の recognition_id を保持する
    pub db_cache_recog_id: Option<i64>,
}

/// 元言語 (en/ja) と明らかに異なるテキストかどうかを判定する (SPECv0.4.8追補:
/// 誤爆翻訳によるAPI消費を防ぐ)。true を返せば翻訳対象として扱う。
/// en/ja 以外の source_lang は判定せず常に true (翻訳を実行する)。
pub fn is_source_lang_text(source_lang: &str, text: &str) -> bool {
    match source_lang {
        "en" => {
            let non_ws: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
            if non_ws.is_empty() {
                return true;
            }
            let alpha = non_ws.iter().filter(|c| c.is_ascii_alphabetic()).count();
            alpha > 0 && (alpha as f32 / non_ws.len() as f32) >= 0.1
        }
        "ja" => text.chars().any(is_japanese_char),
        _ => true,
    }
}

/// ひらがな・カタカナ・CJK統合漢字(拡張Aを含む)かどうか
fn is_japanese_char(c: char) -> bool {
    matches!(c,
        '\u{3040}'..='\u{309F}'
        | '\u{30A0}'..='\u{30FF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{3400}'..='\u{4DBF}'
    )
}

/// 同一 request_json の翻訳結果がDBログに既にあれば (text, recognition_id) を返す。
/// ログ機能OFF時はDBに触れず常に None (未初期化DBファイルを不用意に作らないため)。
fn check_db_cache(
    cfg: &Config,
    engine: &str,
    profile: Option<&str>,
    request_json: &str,
) -> Option<(String, i64)> {
    if !cfg.log_enabled {
        return None;
    }
    crate::logdb::find_cached_translation(engine, profile, request_json)
        .map(|(rid, text)| (text, rid))
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

/// 指定エンジンで翻訳。失敗時は他エンジンへフォールバックせずエラーをそのまま返す
/// (別エンジンの結果を無言で表示すると利用者が気づけないため)。
/// ctx はLLM翻訳プロンプトのプレースホルダ置換に使う (SPECv0.4 §7.1)。
/// 翻訳プロンプト実行時点では translated_text / tr_engine は常に空文字とする。
pub fn translate(
    engine: &str,
    cfg: &Config,
    text: &str,
    ctx: &crate::config::PromptContext,
    image_png_b64: Option<&str>,
    image_hash: Option<&str>,
) -> Result<Translated, String> {
    let mut ctx = ctx.clone();
    ctx.original_text = text.to_string();
    ctx.translated_text.clear();
    ctx.tr_engine.clear();
    let ctx = &ctx;
    let (source, target) = (cfg.source_lang.clone(), cfg.target_lang.clone());
    let profile = if engine == "llm" {
        Some(cfg.active_api_profile.clone())
    } else {
        None
    };
    let key = (
        engine.to_string(),
        profile,
        source.clone(),
        target.clone(),
        text.to_string(),
        image_hash.unwrap_or_default().to_string(),
    );

    // キャッシュ確認
    {
        let mut guard = CACHE.lock().unwrap();
        let map = guard.get_or_insert_with(HashMap::new);
        if let Some(hit) = map.get(&key) {
            return Ok(Translated {
                text: hit.clone(),
                // キャッシュヒットは内部的な最適化であり利用者には無関係なのでバッジは出さない
                badge: None,
                engine: engine.into(),
                source_lang: source,
                target_lang: target,
                cache_hit: true,
                detail: TransDetail::default(),
                db_cache_recog_id: None,
            });
        }
    }

    // エラーメッセージにURL等の形でAPIキーが混入しても表示・ログへ漏れないよう伏字化する
    // (SPECv0.5.3)。
    let result = translate_once(engine, cfg, text, &target, ctx, image_png_b64, image_hash)
        .map_err(|e| mask_keys(cfg, &e));
    match result {
        Ok((t, detail, db_rid)) => {
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
                db_cache_recog_id: db_rid,
            })
        }
        Err(e) => Err(e),
    }
}

fn translate_once(
    engine: &str,
    cfg: &Config,
    text: &str,
    target: &str,
    ctx: &crate::config::PromptContext,
    image_png_b64: Option<&str>,
    image_hash: Option<&str>,
) -> Result<(String, TransDetail, Option<i64>), String> {
    match engine {
        "local" => translate_local(cfg, text, target).map(|t| (t, TransDetail::default(), None)),
        "deepl" => translate_deepl(cfg, text, target),
        "google" => translate_google(cfg, text, target),
        // LLMの翻訳方向はプロンプトテンプレート側で cfg から埋める
        "llm" => translate_llm(cfg, ctx, image_png_b64, image_hash),
        other => Err(format!("不明な翻訳エンジン: {other}")),
    }
}

/// ローカルONNX翻訳 (FuguMT、ort によるONNX Runtime推論)。
fn translate_local(_cfg: &Config, text: &str, target: &str) -> Result<String, String> {
    crate::onnx_translate::translate(text, target == "ja")
}

fn translate_deepl(
    cfg: &Config,
    text: &str,
    target: &str,
) -> Result<(String, TransDetail, Option<i64>), String> {
    let key = cfg.deepl_key();
    if key.is_empty() {
        return Err("DeepL APIキーが未設定です".into());
    }
    // ":fx" で終わるキーは Free プラン
    let host = if key.ends_with(":fx") {
        "api-free.deepl.com"
    } else {
        "api.deepl.com"
    };
    let url = format!("https://{host}/v2/translate");
    let body = serde_json::json!({
        "text": [text],
        "target_lang": target.to_uppercase(),
    });
    let req_json = mask_keys(cfg, &body.to_string());
    // 送信前にDBキャッシュを検索し、同一request_jsonの成功済み翻訳があればAPIを呼ばない
    // (SPECv0.4.8追補: 別の親であっても検索対象)。
    if let Some((cached_text, rid)) = check_db_cache(cfg, "deepl", None, &req_json) {
        let detail = TransDetail {
            request_json: Some(req_json),
            ..Default::default()
        };
        return Ok((cached_text, detail, Some(rid)));
    }
    let mut res = ureq::post(&url)
        .header("Authorization", format!("DeepL-Auth-Key {key}"))
        .send_json(&body)
        .map_err(|e| format!("DeepL呼び出し失敗: {e}"))?;
    let v: serde_json::Value = res
        .body_mut()
        .read_json()
        .map_err(|e| format!("DeepL応答解析失敗: {e}"))?;
    let detail = TransDetail {
        request_json: Some(req_json),
        response_json: Some(mask_keys(cfg, &v.to_string())),
        tokens_in: None,
        tokens_out: None,
    };
    v["translations"][0]["text"]
        .as_str()
        .map(|s| (s.to_string(), detail, None))
        .ok_or("DeepL応答に訳文がありません".into())
}

fn translate_google(
    cfg: &Config,
    text: &str,
    target: &str,
) -> Result<(String, TransDetail, Option<i64>), String> {
    let key = cfg.google_key();
    if key.is_empty() {
        return Err("Google APIキーが未設定です".into());
    }
    // APIキーはURLクエリではなくヘッダーで送る (SPECv0.5.3: エラーメッセージ等に
    // URLが含まれてもキーが露出しないようにするため)。
    let url = "https://translation.googleapis.com/language/translate/v2";
    let body = serde_json::json!({ "q": text, "target": target, "format": "text" });
    let req_json = mask_keys(cfg, &body.to_string());
    if let Some((cached_text, rid)) = check_db_cache(cfg, "google", None, &req_json) {
        let detail = TransDetail {
            request_json: Some(req_json),
            ..Default::default()
        };
        return Ok((cached_text, detail, Some(rid)));
    }
    let mut res = ureq::post(url)
        .header("X-goog-api-key", &key)
        .send_json(&body)
        .map_err(|e| format!("Google翻訳呼び出し失敗: {e}"))?;
    let v: serde_json::Value = res
        .body_mut()
        .read_json()
        .map_err(|e| format!("Google応答解析失敗: {e}"))?;
    let detail = TransDetail {
        request_json: Some(req_json),
        response_json: Some(mask_keys(cfg, &v.to_string())),
        tokens_in: None,
        tokens_out: None,
    };
    v["data"]["translations"][0]["translatedText"]
        .as_str()
        .map(|s| (s.to_string(), detail, None))
        .ok_or("Google応答に訳文がありません".into())
}

/// アクティブなLLMプロファイルで翻訳 (プロバイダ差異は llm_api が吸収)
fn translate_llm(
    cfg: &Config,
    ctx: &crate::config::PromptContext,
    image_png_b64: Option<&str>,
    image_hash: Option<&str>,
) -> Result<(String, TransDetail, Option<i64>), String> {
    let prof = cfg
        .active_profile()
        .ok_or("LLM APIプロファイルが設定されていません")?;
    let mut prompt = cfg.fill_prompt(&prof.translate_prompt, ctx);
    if image_png_b64.is_some() {
        prompt.push_str("\n\n## 添付画像について\n添付画像は対象アプリ全体のスクリーンショットです。赤枠で囲まれた箇所が翻訳対象です。原文テキストと照合し、画面全体の文脈を考慮して翻訳してください。");
    }
    let req = crate::llm_api::LlmRequest {
        prompt: &prompt,
        image_png_b64,
        json_mode: false,
    };
    // DBキャッシュ・ログには巨大なbase64本体を残さず、画像hashだけを識別子として記録する。
    let log_image = image_hash.map(|hash| format!("sha256:{hash}"));
    let log_req = crate::llm_api::LlmRequest {
        prompt: &prompt,
        image_png_b64: log_image.as_deref(),
        json_mode: false,
    };
    let req_json = mask_keys(cfg, &crate::llm_api::build_request_json(prof, &log_req));
    if let Some((cached_text, rid)) = check_db_cache(cfg, "llm", Some(&prof.name), &req_json) {
        let detail = TransDetail {
            request_json: Some(req_json),
            ..Default::default()
        };
        return Ok((cached_text, detail, Some(rid)));
    }
    let res = crate::llm_api::call(prof, &req)?;
    debug_assert!(
        !res.request_json.is_empty(),
        "LLM呼び出しでは送信リクエストJSONが記録される"
    );
    let detail = TransDetail {
        request_json: Some(req_json),
        response_json: Some(mask_keys(cfg, &res.response_json)),
        tokens_in: res.tokens_in,
        tokens_out: res.tokens_out,
    };
    Ok((res.text, detail, None))
}

#[cfg(test)]
mod tests {
    use super::{decode_percent_encoded, is_source_lang_text};

    #[test]
    fn 連続したパーセントエンコーディングをデコードする() {
        assert_eq!(
            decode_percent_encoded(
                "docs/%E6%B5%B7%E5%A4%96%E8%A3%BD%E3%82%BD%E3%83%95%E3%83%88%E3%81%AEUI%E3%81%AE%E7%BF%BB%E8%A8%B3%E3%81%8A%E3%82%88%E3%81%B3%E6%A9%9F%E8%83%BD%E8%A7%A3%E8%AA%AC.png"
            ),
            Some("docs/海外製ソフトのUIの翻訳および機能解説.png".to_string())
        );
    }

    #[test]
    fn 小文字の16進表記もデコードする() {
        assert_eq!(decode_percent_encoded("%e3%81%82"), Some("あ".to_string()));
    }

    #[test]
    fn 単独のパーセントエンコーディングは検知しない() {
        assert_eq!(decode_percent_encoded("進捗%20です"), None);
    }

    #[test]
    fn 不正なutf8はデコード結果にしない() {
        assert_eq!(decode_percent_encoded("%FF%FF"), None);
    }

    #[test]
    fn en_通常の英文は翻訳対象() {
        assert!(is_source_lang_text("en", "Hello, world!"));
    }

    #[test]
    fn en_日本語のみはスキップ対象() {
        assert!(!is_source_lang_text("en", "これは日本語のテキストです"));
    }

    #[test]
    fn en_記号数字のみはスキップ対象() {
        assert!(!is_source_lang_text("en", "12:34 - 100% (合計)"));
    }

    #[test]
    fn en_境界値_ちょうど10パーセントは翻訳対象() {
        // 非空白10文字中1文字がアルファベット (10%) → 翻訳する
        assert!(is_source_lang_text("en", "a123456789"));
    }

    #[test]
    fn en_9パーセント未満はスキップ対象() {
        // 非空白11文字中1文字がアルファベット (約9.1%) → スキップ
        assert!(!is_source_lang_text("en", "a1234567891"));
    }

    #[test]
    fn en_空文字は翻訳対象扱い() {
        assert!(is_source_lang_text("en", ""));
    }

    #[test]
    fn ja_通常の日本語文は翻訳対象() {
        assert!(is_source_lang_text("ja", "こんにちは、世界。"));
    }

    #[test]
    fn ja_漢字のみでも翻訳対象() {
        assert!(is_source_lang_text("ja", "日本語漢字"));
    }

    #[test]
    fn ja_かな1文字混在でも翻訳対象() {
        assert!(is_source_lang_text("ja", "Error: エラーが発生しました"));
    }

    #[test]
    fn ja_英数のみはスキップ対象() {
        assert!(!is_source_lang_text("ja", "Hello World 12345"));
    }

    #[test]
    fn 未対応言語は常に翻訳対象() {
        assert!(is_source_lang_text("zh", "Hello"));
        assert!(is_source_lang_text("", "12345"));
    }
}
