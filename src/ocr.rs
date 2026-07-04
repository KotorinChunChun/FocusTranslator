// OCR エンジン群 (SPEC §7.1)
// - win:      Windows.Media.Ocr (既定・ローカル)
// - paddle:   PaddleOCR (初版はモデル未同梱のためスキャフォールドのみ)
// - yomitoku / ndl: 外部OCRサーバー (HTTP POST /ocr, GET /health)
// - gemini:   OCR+翻訳統合 (画像→原文+訳文を一括取得)
use crate::capture::Captured;
use crate::config::Config;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Security::Cryptography::CryptographicBuffer;

/// OCR結果(gemini 統合モードは訳文も返す)
pub struct OcrOutput {
    pub text: String,
    pub translation: Option<String>,
}

/// 指定エンジンでOCRを実行する。
/// focus_y: 帯内の注目Y座標(単一行選択用)。None なら全行を段落結合。
pub fn run(engine: &str, cfg: &Config, img: &Captured, focus_y: Option<f32>) -> Result<OcrOutput, String> {
    match engine {
        "win" => ocr_windows(img, focus_y).map(|t| OcrOutput { text: t, translation: None }),
        "paddle" => Err("PaddleOCR は初版ではモデル未同梱のため利用できません".into()),
        "yomitoku" => {
            ocr_http(&cfg.yomitoku_url, img).map(|t| OcrOutput { text: t, translation: None })
        }
        "ndl" => ocr_http(&cfg.ndl_url, img).map(|t| OcrOutput { text: t, translation: None }),
        "gemini" => gemini_ocr_translate(cfg, img),
        other => Err(format!("不明なOCRエンジン: {other}")),
    }
}

/// Windows.Media.Ocr によるローカルOCR
pub fn ocr_windows(img: &Captured, focus_y: Option<f32>) -> Result<String, String> {
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|_| "OCRエンジンを初期化できません(言語パック未導入の可能性)".to_string())?;
    let ibuf = CryptographicBuffer::CreateFromByteArray(&img.bgra)
        .map_err(|e| format!("バッファ作成失敗: {e}"))?;
    let bmp = SoftwareBitmap::CreateCopyFromBuffer(
        &ibuf,
        BitmapPixelFormat::Bgra8,
        img.width as i32,
        img.height as i32,
    )
    .map_err(|e| format!("ビットマップ作成失敗: {e}"))?;
    let result = engine
        .RecognizeAsync(&bmp)
        .map_err(|e| format!("OCR開始失敗: {e}"))?
        .join()
        .map_err(|e| format!("OCR実行失敗: {e}"))?;

    let lines = result.Lines().map_err(|e| format!("行取得失敗: {e}"))?;
    let mut items: Vec<(f32, String)> = Vec::new(); // (行の中心Y, テキスト)
    let count = lines.Size().unwrap_or(0);
    for i in 0..count {
        let Ok(line) = lines.GetAt(i) else { continue };
        let text = line.Text().map(|t| t.to_string()).unwrap_or_default();
        if text.trim().is_empty() {
            continue;
        }
        let mut top = f32::MAX;
        let mut bottom = f32::MIN;
        if let Ok(words) = line.Words() {
            let wcount = words.Size().unwrap_or(0);
            for wi in 0..wcount {
                let Ok(w) = words.GetAt(wi) else { continue };
                if let Ok(r) = w.BoundingRect() {
                    top = top.min(r.Y);
                    bottom = bottom.max(r.Y + r.Height);
                }
            }
        }
        let cy = if top <= bottom { (top + bottom) / 2.0 } else { img.height as f32 / 2.0 };
        items.push((cy, text));
    }
    if items.is_empty() {
        return Err("テキストを検出できませんでした".into());
    }

    match focus_y {
        Some(fy) => {
            // カーソルに最も近い1行を採用
            let mut best = &items[0];
            for it in &items {
                if (it.0 - fy).abs() < (best.0 - fy).abs() {
                    best = it;
                }
            }
            Ok(normalize_line(&best.1))
        }
        None => {
            // 複数行を段落として結合(範囲指定モード)
            items.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let texts: Vec<String> = items.iter().map(|i| normalize_line(&i.1)).collect();
            Ok(join_paragraph(&texts))
        }
    }
}

/// Windows OCR は CJK でも単語間に空白を入れることがあるため整形する
fn normalize_line(s: &str) -> String {
    let t = s.trim();
    if crate::util::contains_cjk(t) {
        // CJK文字に挟まれた空白を除去
        let chars: Vec<char> = t.chars().collect();
        let mut out = String::with_capacity(t.len());
        for (i, &c) in chars.iter().enumerate() {
            if c == ' ' {
                let prev = chars[..i].iter().rev().find(|c| **c != ' ');
                let next = chars[i + 1..].iter().find(|c| **c != ' ');
                if let (Some(&p), Some(&n)) = (prev, next)
                    && is_cjk_char(p) && is_cjk_char(n) {
                        continue;
                    }
            }
            out.push(c);
        }
        out
    } else {
        t.to_string()
    }
}

fn is_cjk_char(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFF9D)
}

/// 複数行を段落として結合 (SPEC §3.2)
pub fn join_paragraph(lines: &[String]) -> String {
    let mut out = String::new();
    for line in lines {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        if out.is_empty() {
            out.push_str(l);
        } else if out.ends_with('-') && !crate::util::contains_cjk(l) {
            // 行末ハイフンは連結(英文の分綴り)
            out.pop();
            out.push_str(l);
        } else if crate::util::contains_cjk(out.chars().last().map(String::from).as_deref().unwrap_or(""))
        {
            out.push_str(l);
        } else {
            out.push(' ');
            out.push_str(l);
        }
    }
    out
}

/// 外部OCRサーバー (YomiToku / NDL-OCR): POST {url}/ocr に PNG を送る
pub fn ocr_http(base_url: &str, img: &Captured) -> Result<String, String> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("サーバーURLが未設定です".into());
    }
    let png = crate::capture::to_png(img);
    let url = format!("{base}/ocr");
    let mut res = ureq::post(&url)
        .header("Content-Type", "image/png")
        .send(&png[..])
        .map_err(|e| format!("外部OCRサーバーに接続できません: {e}"))?;
    let v: serde_json::Value = res
        .body_mut()
        .read_json()
        .map_err(|e| format!("応答の解析に失敗: {e}"))?;
    // 想定形式: {"text": "..."} または {"results":[{"text":"..."}]}
    if let Some(t) = v.get("text").and_then(|t| t.as_str()) {
        return Ok(t.trim().to_string());
    }
    if let Some(arr) = v.get("results").and_then(|r| r.as_array()) {
        let lines: Vec<String> = arr
            .iter()
            .filter_map(|e| e.get("text").and_then(|t| t.as_str()))
            .map(|s| s.to_string())
            .collect();
        if !lines.is_empty() {
            return Ok(join_paragraph(&lines));
        }
    }
    Err("外部OCRサーバーの応答形式が不明です".into())
}

/// 外部OCRサーバーの接続確認 (GET {url}/health)
pub fn health_check(base_url: &str) -> bool {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return false;
    }
    ureq::get(format!("{base}/health"))
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(3)))
        .build()
        .call()
        .is_ok()
}

/// Gemini OCR+翻訳統合モード: 画像から原文と訳文を一括取得 (SPEC §8)
pub fn gemini_ocr_translate(cfg: &Config, img: &Captured) -> Result<OcrOutput, String> {
    let key = cfg.gemini_key();
    if key.is_empty() {
        return Err("Gemini APIキーが未設定です".into());
    }
    let png = crate::capture::to_png(img);
    let b64 = B64.encode(&png);
    let target = if cfg.target_lang == "en" { "English" } else { "Japanese" };
    let prompt = format!(
        "Extract the text in this image and translate it to {target}. \
         Respond with JSON only: {{\"source\": \"<extracted text>\", \"translation\": \"<translation>\"}}"
    );
    let body = serde_json::json!({
        "contents": [{ "parts": [
            { "text": prompt },
            { "inlineData": { "mimeType": "image/png", "data": b64 } }
        ]}],
        "generationConfig": { "responseMimeType": "application/json" }
    });
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
        cfg.gemini_model
    );
    let mut res = ureq::post(&url)
        .header("x-goog-api-key", &key)
        .send_json(&body)
        .map_err(|e| format!("Gemini呼び出し失敗: {e}"))?;
    let v: serde_json::Value = res
        .body_mut()
        .read_json()
        .map_err(|e| format!("Gemini応答の解析失敗: {e}"))?;
    let text = v["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .ok_or("Gemini応答にテキストがありません")?;
    let inner: serde_json::Value =
        serde_json::from_str(text.trim()).map_err(|_| "Gemini応答のJSON解析に失敗".to_string())?;
    let source = inner["source"].as_str().unwrap_or("").trim().to_string();
    let translation = inner["translation"].as_str().unwrap_or("").trim().to_string();
    if source.is_empty() {
        return Err("テキストを検出できませんでした".into());
    }
    Ok(OcrOutput {
        text: source,
        translation: if translation.is_empty() { None } else { Some(translation) },
    })
}
