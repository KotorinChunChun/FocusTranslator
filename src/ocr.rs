// OCR エンジン群 (SPEC §7.1)
// - oneocr:   OneOCR (oneocr.dll、Windows 11 Snipping Tool同梱。既定・ローカル。oneocr 参照)
// - win:      Windows.Media.Ocr (ローカル)
// - paddle:   PaddleOCR (モデル導入は paddle_install、ONNX Runtime推論は paddle_ocr 参照)
// - llm:      LLM OCR+翻訳統合 (画像→原文+訳文を一括取得。llm_api 参照)
use crate::capture::Captured;
use crate::config::Config;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Security::Cryptography::CryptographicBuffer;

/// OCR結果(llm 統合モードは訳文も返す)
#[derive(Default)]
pub struct OcrOutput {
    pub text: String,
    pub translation: Option<String>,
    /// LLM統合モードの生応答JSON(APIキーマスク済み。ログDB用)。他エンジンはNone。
    pub raw_response: Option<String>,
    /// LLM統合モードのトークン数(ログDB用)
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    /// Paragraph モード時のカーソル直下1行 (Windows OCR のみ)。
    /// UIA Name からの段落復元 (worker::paragraph_from_text) の検索キーに使う。
    pub focus_line: Option<String>,
}

impl OcrOutput {
    fn text_only(text: String) -> Self {
        OcrOutput { text, ..Default::default() }
    }
}

/// OCRの行選択モード
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Focus {
    /// 指定Y座標(画像内)に最も近い1行を採用
    Line(f32),
    /// 指定Y座標を含む段落を採用 (行間ギャップで段落境界を推定し、折返し行を結合)
    Paragraph(f32),
    /// 全行を段落結合 (範囲指定モード)
    All,
}

impl Focus {
    /// 単一行選択のY座標 (Line のときのみ)
    fn line_y(&self) -> Option<f32> {
        match self {
            Focus::Line(fy) => Some(*fy),
            _ => None,
        }
    }
}

/// 指定エンジンでOCRを実行する。
/// ctx はLLM統合OCRプロンプトのプレースホルダ置換に使う (SPECv0.4 §7.1)。
pub fn run(
    engine: &str,
    cfg: &Config,
    img: &Captured,
    focus: Focus,
    ctx: &crate::config::PromptContext,
) -> Result<OcrOutput, String> {
    match engine {
        "oneocr" => crate::oneocr::ocr_oneocr(img, focus).map(|(text, focus_line)| OcrOutput {
            text,
            focus_line,
            ..Default::default()
        }),
        "win" => ocr_windows(img, focus).map(|(text, focus_line)| OcrOutput {
            text,
            focus_line,
            ..Default::default()
        }),
        "paddle" => {
            if crate::paddle_install::installed() {
                // PaddleOCR は段落境界推定が未対応のため、Paragraph は帯内全行の結合で近似する
                crate::paddle_ocr::ocr_paddle(img, focus.line_y()).map(OcrOutput::text_only)
            } else {
                Err("PaddleOCRのモデルが未導入です。設定画面からインストールしてください".into())
            }
        }
        "llm" => llm_ocr_translate(cfg, img, focus, ctx),
        other => Err(format!("不明なOCRエンジン: {other}")),
    }
}

/// Windows.Media.Ocr によるローカルOCR。
/// 戻り値: (採用テキスト, Paragraphモード時のカーソル直下1行)
pub fn ocr_windows(img: &Captured, focus: Focus) -> Result<(String, Option<String>), String> {
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
    // (行の上端Y, 下端Y, テキスト)
    let mut items: Vec<(f32, f32, String)> = Vec::new();
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
        if top > bottom {
            let c = img.height as f32 / 2.0;
            top = c;
            bottom = c;
        }
        items.push((top, bottom, text));
    }
    select_by_focus(items, focus)
}

/// (行の上端Y, 下端Y, テキスト) のリストから Focus に応じて採用テキストを選ぶ。
/// Windows.Media.Ocr と OneOCR で共用する。
/// 戻り値: (採用テキスト, Paragraphモード時のカーソル直下1行)
pub(crate) fn select_by_focus(
    mut items: Vec<(f32, f32, String)>,
    focus: Focus,
) -> Result<(String, Option<String>), String> {
    if items.is_empty() {
        return Err("テキストを検出できませんでした".into());
    }
    items.sort_by(|a, b| line_cy(a).partial_cmp(&line_cy(b)).unwrap_or(std::cmp::Ordering::Equal));

    match focus {
        Focus::Line(fy) => {
            // カーソルに最も近い1行を採用
            let mut best = &items[0];
            for it in &items {
                if (line_cy(it) - fy).abs() < (line_cy(best) - fy).abs() {
                    best = it;
                }
            }
            Ok((normalize_line(&best.2), None))
        }
        Focus::Paragraph(fy) => {
            // カーソル行から行間ギャップの小さい隣接行へ広げ、段落として結合する
            let (para, focus_line) = paragraph_at(&items, fy);
            let texts: Vec<String> = para.iter().map(|i| normalize_line(&i.2)).collect();
            Ok((join_paragraph(&texts), Some(normalize_line(focus_line))))
        }
        Focus::All => {
            // 複数行を段落として結合(範囲指定モード)
            let texts: Vec<String> = items.iter().map(|i| normalize_line(&i.2)).collect();
            Ok((join_paragraph(&texts), None))
        }
    }
}

fn line_cy(item: &(f32, f32, String)) -> f32 {
    (item.0 + item.1) / 2.0
}

/// fy を含む(最も近い)行を起点に、行間ギャップが小さい隣接行へ上下に広げて段落を切り出す。
/// 折返しの行間は行高より十分小さく、段落間の空きは行高程度以上あることを利用する。
/// 戻り値: (段落の行スライス, カーソル直下の行テキスト)
fn paragraph_at(items: &[(f32, f32, String)], fy: f32) -> (&[(f32, f32, String)], &str) {
    let mut focus = 0;
    for (i, it) in items.iter().enumerate() {
        if (line_cy(it) - fy).abs() < (line_cy(&items[focus]) - fy).abs() {
            focus = i;
        }
    }
    // 行高の中央値から段落境界とみなすギャップ閾値を決める
    let mut heights: Vec<f32> = items.iter().map(|i| i.1 - i.0).collect();
    heights.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median_h = heights[heights.len() / 2].max(8.0);
    let gap_limit = median_h * 0.8;

    let mut start = focus;
    while start > 0 && items[start].0 - items[start - 1].1 <= gap_limit {
        start -= 1;
    }
    let mut end = focus;
    while end + 1 < items.len() && items[end + 1].0 - items[end].1 <= gap_limit {
        end += 1;
    }
    (&items[start..=end], &items[focus].2)
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

/// 単一行選択(Line)のときのみ、注目行を中心に高さ64pxの帯へ切り抜く。
/// Paragraph / All は画像全体を対象とする(段落・全行を拾うため)。
/// 外部OCR/LLMへの送信画像と、ログ保存画像の切り出しで共用する。
pub fn crop_for_focus(img: &Captured, focus: Focus) -> std::borrow::Cow<'_, Captured> {
    if let Focus::Line(fy) = focus {
        let h = 64; // 高さ64pxの帯に切り抜く(複数行を拾うのを防ぐ)
        let top = (fy - h as f32 / 2.0).round() as i32;
        if let Some(cropped) = crate::capture::crop(img, 0, top, img.width as i32, h) {
            return std::borrow::Cow::Owned(cropped);
        }
    }
    std::borrow::Cow::Borrowed(img)
}

/// LLM OCR+翻訳統合モード: 画像から原文と訳文を一括取得 (SPEC §8)
pub fn llm_ocr_translate(
    cfg: &Config,
    img: &Captured,
    focus: Focus,
    ctx: &crate::config::PromptContext,
) -> Result<OcrOutput, String> {
    let prof = cfg.active_profile().ok_or("LLM APIプロファイルが設定されていません")?;
    let target_img = crop_for_focus(img, focus);
    let png = crate::capture::to_png(&target_img);
    let b64 = B64.encode(&png);
    // OCRプロンプト実行時点では原文・訳文は未取得のため空文字。実行エンジン名だけ補う。
    let mut ctx = ctx.clone();
    ctx.original_text.clear();
    ctx.translated_text.clear();
    ctx.tr_engine.clear();
    ctx.ocr_engine = "llm".into();
    let prompt = cfg.fill_prompt(&prof.ocr_prompt, &ctx);

    let res = crate::llm_api::call(prof, &crate::llm_api::LlmRequest {
        prompt: &prompt,
        image_png_b64: Some(&b64),
        json_mode: true,
    })?;

    let inner: serde_json::Value =
        serde_json::from_str(res.text.trim()).map_err(|_| "LLM応答のJSON解析に失敗".to_string())?;
    let source = inner.get("source").and_then(|s| s.as_str()).unwrap_or("").trim().to_string();
    let translation = inner.get("translation").and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
    if source.is_empty() {
        return Err("テキストを検出できませんでした".into());
    }
    Ok(OcrOutput {
        text: source,
        translation: if translation.is_empty() { None } else { Some(translation) },
        raw_response: Some(crate::translate::mask_keys(cfg, &res.response_json)),
        tokens_in: res.tokens_in,
        tokens_out: res.tokens_out,
        focus_line: None,
    })
}
