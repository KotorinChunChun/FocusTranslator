// 認識・翻訳ワーカースレッド (SPEC §6)
// 各サイクルは世代番号 generation を持ち、main 側で古い世代の結果は破棄される。
use crate::capture::{self, Captured};
use crate::config::Config;
use crate::{ocr, uia, util};
use std::sync::Arc;
use std::time::Instant;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow, PostMessageW};

/// ワーカーから main へ送るメッセージ(LPARAM に Box して渡す)
pub enum WorkerMsg {
    Source {
        text: String,
        method: &'static str,
        /// 実際に使ったOCRエンジンキー (UIA経路では None)
        engine: Option<String>,
        img: Option<Arc<Captured>>,
        pin: bool,
        anchor: (i32, i32),
        /// 保持画像の行選択モード (再OCR時に同じモードで認識する)
        focus: ocr::Focus,
        ms: u128,
        recog_id: Option<i64>,
        app_title: String,
        uia_path: String,
    },
    Translation {
        text: String,
        badge: Option<String>,
        ms: u128,
        recog_id: Option<i64>,
    },
    TranslationFailed {
        msg: String,
        engine: String,
    },
    Error {
        msg: String,
        anchor: (i32, i32),
        engine: Option<String>,
    },
    Explanation {
        text: String,
    },
}

fn post(main: isize, generation: u64, msg: WorkerMsg) {
    let ptr = Box::into_raw(Box::new(msg)) as isize;
    unsafe {
        let _ = PostMessageW(
            Some(HWND(main as *mut _)),
            crate::WM_APP_WORKER,
            WPARAM(generation as usize),
            LPARAM(ptr),
        );
    }
}

fn init_com() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

/// 翻訳対象アプリのコンテキスト (exe名 / ウィンドウタイトル / UIA要素パス; SPEC v0.2 §2.3.1)
struct AppContext {
    exe: Option<String>,
    title: String,
    uia_path: String,
}

impl AppContext {
    fn capture(x: i32, y: i32, target: HWND) -> Self {
        let (exe, title) = util::get_window_context(target);
        AppContext {
            exe,
            title: title.unwrap_or_default(),
            uia_path: uia::path_at_point(x, y),
        }
    }
}

/// 認識ログを記録し recognition_id を返す(ログOFF時は None)。
#[allow(clippy::too_many_arguments)]
fn log_recog(
    cfg: &Config,
    mode: &str,
    method: &str,
    engine: &str,
    ms: u128,
    text: Option<&str>,
    error: Option<&str>,
    image: Option<&Captured>,
    ctx: &AppContext,
) -> Option<i64> {
    if !cfg.log_enabled {
        return None;
    }
    let id = crate::logdb::log_recognition(
        mode, method, engine, ms, text, error, image, cfg.debug_mode,
        ctx.exe.as_deref(), Some(&ctx.title), Some(&ctx.uia_path),
    );
    crate::logdb::rotate(cfg.log_max_records);
    id
}

/// 翻訳成功ログを記録する(ログOFF時は何もしない)。
fn log_trans_ok(cfg: &Config, recog_id: Option<i64>, ms: u128, t: &crate::translate::Translated) {
    if !cfg.log_enabled {
        return;
    }
    crate::logdb::log_translation(
        recog_id,
        &t.engine,
        &t.source_lang,
        &t.target_lang,
        ms,
        t.cache_hit,
        Some(&t.text),
        None,
        t.detail.request_json.as_deref(),
        t.detail.response_json.as_deref(),
        t.detail.tokens_in,
        t.detail.tokens_out,
    );
}

/// 翻訳失敗ログを記録する。
fn log_trans_err(cfg: &Config, recog_id: Option<i64>, engine: &str, ms: u128, err: &str) {
    if !cfg.log_enabled {
        return;
    }
    crate::logdb::log_translation(
        recog_id, engine, &cfg.source_lang, &cfg.target_lang, ms, false, None, Some(err),
        None, None, None, None,
    );
}

/// LLM統合モードの翻訳ログを記録する(OCR側で取得した生応答・トークンを使う)。
fn log_trans_llm(cfg: &Config, recog_id: Option<i64>, ms: u128, tr: &str, o: &ocr::OcrOutput) {
    if !cfg.log_enabled {
        return;
    }
    crate::logdb::log_translation(
        recog_id, "llm", &cfg.source_lang, &cfg.target_lang, ms, false,
        Some(tr), None, None, o.raw_response.as_deref(), o.tokens_in, o.tokens_out,
    );
}

/// 同意が無いクラウド/外部OCRエンジンをローカルへ置き換える (SPEC §9: 同意なしで外部送信しない)
fn effective_ocr(cfg: &Config) -> String {
    let e = cfg.default_ocr.as_str();
    let allowed = match e {
        "llm" => cfg.consent_image,
        "yomitoku" | "ndl" => cfg.consent_ext_ocr,
        _ => true,
    };
    if allowed && cfg.engine_available(e) { e.to_string() } else { "win".to_string() }
}

fn effective_translator(cfg: &Config) -> String {
    let e = cfg.default_translator.as_str();
    let allowed = match e {
        "deepl" | "google" | "llm" => cfg.consent_text,
        _ => true,
    };
    if allowed { e.to_string() } else { "local".to_string() }
}

/// ホールド認識で切り出すカーソル周辺帯のサイズ (px)。領域検出モードの枠表示と共用。
pub const BAND_W: i32 = 1200;
pub const BAND_H: i32 = 160;
/// UIA矩形とカーソルY座標の許容距離 (px)。これ以上離れた矩形は採用しない。
const NEAR_Y_PX: i32 = 10;
/// 直下要素(TextPatternなし)をそのままキャプチャする高さの上限 (px)。
/// これより高い要素は段落帯 (要素幅 × カーソル中心 PARA_BAND_H) に切り替える。
const HOVER_MAX_H: i32 = 320;
/// 段落帯の高さ (px)。段落全体を拾えるよう既定帯より上下に広くとる。
const PARA_BAND_H: i32 = 320;
/// 採用した矩形に付ける余白 (px)。文字の欠けを防ぐ。
const CAP_PAD: i32 = 6;

/// キャプチャ矩形の由来 (plan_capture_rect の決定結果)
#[derive(Clone, Copy, PartialEq)]
pub enum CapKind {
    /// UIA行矩形(黄)の統合
    Line,
    /// TextPattern要素(緑)
    Element,
    /// 直下要素(紫)
    Hover,
    /// 背の高い直下要素内の段落帯 (OCRは段落モード)
    HoverBand,
    /// 既定のカーソル中心帯(橙)
    Band,
}

impl CapKind {
    /// 領域検出モードのラベル表示用
    pub fn label(&self) -> &'static str {
        match self {
            CapKind::Line => "UIA行",
            CapKind::Element => "UIA要素",
            CapKind::Hover => "直下要素",
            CapKind::HoverBand => "段落帯",
            CapKind::Band => "既定帯",
        }
    }
}

/// 矩形とY座標の垂直距離 (内側なら0)
fn v_dist(r: &RECT, y: i32) -> i32 {
    (r.top - y).max(y - r.bottom).max(0)
}

/// 幅・高さが最低限あるか
fn rect_valid(r: &RECT) -> bool {
    r.right - r.left >= 8 && r.bottom - r.top >= 4
}

/// 複数矩形を1つの外接矩形へ統合
fn merge_rects(rects: &[RECT]) -> RECT {
    let mut m = rects[0];
    for r in &rects[1..] {
        m.left = m.left.min(r.left);
        m.top = m.top.min(r.top);
        m.right = m.right.max(r.right);
        m.bottom = m.bottom.max(r.bottom);
    }
    m
}

/// 余白を付けてウィンドウ矩形へクランプ
fn pad_clamp(r: &RECT, win: &RECT) -> RECT {
    RECT {
        left: (r.left - CAP_PAD).max(win.left),
        top: (r.top - CAP_PAD).max(win.top),
        right: (r.right + CAP_PAD).min(win.right),
        bottom: (r.bottom + CAP_PAD).min(win.bottom),
    }
}

/// UIA検出結果からキャプチャすべきスクリーン矩形を決める。
/// 優先: UIA行矩形(統合) → TextPattern要素 → 直下要素(高すぎる場合は段落帯) → 既定帯。
/// カーソルYから NEAR_Y_PX 以上離れた矩形は誤検出とみなして採用しない。
/// 領域検出モード(detect)の枠表示と実際のキャプチャで共用する。
pub fn plan_capture_rect(p: &crate::uia::UiaProbe, win: &RECT, x: i32, y: i32) -> (RECT, CapKind) {
    // 黄: 行矩形群を1つの長方形に統合
    if !p.line_rects.is_empty() {
        let merged = merge_rects(&p.line_rects);
        if rect_valid(&merged) && v_dist(&merged, y) < NEAR_Y_PX {
            return (pad_clamp(&merged, win), CapKind::Line);
        }
    }
    // 緑: TextPattern が見つかった要素
    if let Some(r) = &p.element_rect
        && rect_valid(r) && v_dist(r, y) < NEAR_Y_PX {
            return (pad_clamp(r, win), CapKind::Element);
        }
    // 紫: 直下要素。高すぎる場合はカーソル位置の段落を狙った帯へ切り替える
    if let Some(r) = &p.hover_rect
        && rect_valid(r) && v_dist(r, y) < NEAR_Y_PX {
            if r.bottom - r.top > HOVER_MAX_H {
                let band = RECT {
                    left: r.left,
                    top: (y - PARA_BAND_H / 2).max(r.top),
                    right: r.right,
                    bottom: (y + PARA_BAND_H / 2).min(r.bottom),
                };
                return (pad_clamp(&band, win), CapKind::HoverBand);
            }
            return (pad_clamp(r, win), CapKind::Hover);
        }
    // 橙: 既定のカーソル中心帯
    (capture::band_screen_rect(win, x, y, BAND_W, BAND_H), CapKind::Band)
}

/// キャプチャ由来に応じたOCRの行選択モード
fn focus_for(kind: CapKind, fy: f32) -> ocr::Focus {
    match kind {
        CapKind::HoverBand => ocr::Focus::Paragraph(fy),
        _ => ocr::Focus::Line(fy),
    }
}

/// 検索用の正規化: 空白を除去し小文字化する (OCRとUIA Nameの表記ゆれ吸収)
fn normalize_for_match(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).flat_map(|c| c.to_lowercase()).collect()
}

/// 要素の全テキスト (UIA Name) から、OCRで得たカーソル直下1行に合致する段落を取り出す。
/// 折返しは Name 上では改行にならないため、改行区切り=段落として扱える。
/// これにより画像に写っていない折返し部分も含む正確な1段落が復元できる (ユーザー提案)。
pub fn paragraph_from_text(full: &str, ocr_line: &str) -> Option<String> {
    let key = normalize_for_match(ocr_line);
    if key.chars().count() < 4 {
        return None;
    }
    // OCR誤認識に備え、行全体 → 先頭12文字 → 末尾12文字 の順で検索キーを緩める
    let chars: Vec<char> = key.chars().collect();
    let head: String = chars.iter().take(12).collect();
    let tail: String = chars.iter().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect();
    let mut keys: Vec<&str> = vec![&key];
    if chars.len() > 12 {
        keys.push(&head);
        keys.push(&tail);
    }
    for k in keys {
        if k.chars().count() < 6 && k != key {
            continue;
        }
        for para in full.split(['\n', '\r']).map(str::trim).filter(|p| !p.is_empty()) {
            if normalize_for_match(para).contains(k) {
                return Some(para.to_string());
            }
        }
    }
    None
}

struct Band {
    img: Captured,
    focus_y: f32,
}

/// 対象ウィンドウをキャプチャし、スクリーン座標の矩形 sr を切り出す。
/// focus_y は切り出し後画像内でのカーソルYを返す。
fn capture_screen_rect(target: isize, sr: &RECT, y: i32) -> Result<Band, String> {
    let hwnd = HWND(target as *mut _);
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() || IsIconic(hwnd).as_bool() {
            return Err("このウィンドウは取得できません".into());
        }
    }
    let full = capture::capture_window(hwnd)?;
    let r = capture::window_frame_rect(hwnd);
    let rw = (r.right - r.left).max(1);
    let rh = (r.bottom - r.top).max(1);
    let scale_x = full.width as f32 / rw as f32;
    let scale_y = full.height as f32 / rh as f32;
    let left = ((sr.left - r.left) as f32 * scale_x) as i32;
    let top = ((sr.top - r.top) as f32 * scale_y) as i32;
    let w = ((sr.right - sr.left) as f32 * scale_x) as i32;
    let h = ((sr.bottom - sr.top) as f32 * scale_y) as i32;
    let band = capture::crop(&full, left, top, w, h).ok_or("このウィンドウは取得できません")?;
    let focus_y = ((y - sr.top.max(r.top)) as f32 * scale_y).max(0.0);
    Ok(Band { img: band, focus_y })
}

/// ポインタ直下ウィンドウをキャプチャし、カーソル周辺の帯を切り出す (SPEC §6.3)
fn capture_band(x: i32, y: i32, target: isize, bw: i32, bh: i32) -> Result<Band, String> {
    let win = capture::window_frame_rect(HWND(target as *mut _));
    let sr = capture::band_screen_rect(&win, x, y, bw, bh);
    capture_screen_rect(target, &sr, y)
}

/// UIA検出結果に基づいてキャプチャ領域を決めて切り出す (黄/緑/紫 → 既定帯の順)
fn capture_probe(x: i32, y: i32, target: isize, probe: &crate::uia::UiaProbe) -> Result<(Band, CapKind), String> {
    let win = capture::window_frame_rect(HWND(target as *mut _));
    let (rect, kind) = plan_capture_rect(probe, &win, x, y);
    capture_screen_rect(target, &rect, y).map(|b| (b, kind))
}

/// OCR結果に統合訳文が含まれていればそのまま表示し、無ければ翻訳ワーカーへ回す
fn dispatch_translation(
    generation: u64,
    cfg: Config,
    o: ocr::OcrOutput,
    tr_engine: String,
    main: isize,
    recog_id: Option<i64>,
    t0: &Instant,
) {
    if let Some(tr) = &o.translation {
        // LLM統合モード: 訳文も同時取得済み
        let tms = t0.elapsed().as_millis();
        log_trans_llm(&cfg, recog_id, tms, tr, &o);
        post(main, generation, WorkerMsg::Translation {
            text: tr.clone(),
            badge: Some("LLM統合".into()),
            ms: tms,
            recog_id,
        });
    } else {
        translate(generation, cfg, o.text, tr_engine, main, recog_id);
    }
}

/// ホールドモードの認識サイクル: UIA優先 → WGCキャプチャOCR (SPEC §6.4)
/// キャプチャ領域は UIA検出結果 (行矩形/要素/直下要素) を優先し、無ければ既定帯。
pub fn recognize_cycle(generation: u64, x: i32, y: i32, target: isize, cfg: Config, main: isize) {
    std::thread::spawn(move || {
        let tr_engine = effective_translator(&cfg);
        init_com();
        let t0 = Instant::now();
        let ctx = AppContext::capture(x, y, HWND(target as *mut _));
        let probe = uia::probe_at_point(x, y);

        // 経路A: UIA
        if let Some(text) = probe.text.clone() {
            let ms = t0.elapsed().as_millis();
            util::perf_log(cfg.perf_log, &format!("source UIA {ms}ms"));

            // OCRは行っていないが、後でOCRエンジンへ切り替えた際に再キャプチャ不要で使えるよう、
            // また認識ログにも紐づけられるよう、この時点で検出領域の画像を撮影しておく。
            let cap = capture_probe(x, y, target, &probe).ok();
            let focus = cap
                .as_ref()
                .map(|(b, kind)| focus_for(*kind, b.focus_y))
                .unwrap_or(ocr::Focus::All);
            let log_img: Option<Captured> =
                cap.as_ref().map(|(b, _)| ocr::crop_for_focus(&b.img, focus).into_owned());
            let img = cap.map(|(b, _)| Arc::new(b.img));

            let recog_id = log_recog(
                &cfg, "hold", "uia", "uia", ms, Some(&text), None,
                log_img.as_ref(), &ctx,
            );
            post(main, generation, WorkerMsg::Source {
                text: text.clone(),
                method: "UIA",
                engine: None,
                img,
                pin: true,
                anchor: (x, y),
                focus,
                ms,
                recog_id,
                app_title: ctx.title,
                uia_path: ctx.uia_path,
            });
            translate(generation, cfg, text, tr_engine, main, recog_id);
            return;
        }

        // 経路B: WGC + キャプチャOCR (直下要素 → 段落帯 → 既定帯)
        let engine = effective_ocr(&cfg);
        let cap = match capture_probe(x, y, target, &probe) {
            Ok(c) => c,
            Err(e) => {
                log_recog(&cfg, "hold", "ocr", &engine, t0.elapsed().as_millis(), None, Some(&e), None, &ctx);
                post(main, generation, WorkerMsg::Error { msg: e, anchor: (x, y), engine: None });
                return;
            }
        };
        let (band, kind) = cap;
        let mut used = band;
        let mut focus = focus_for(kind, used.focus_y);
        let mut out = ocr::run(&engine, &cfg, &used.img, focus);
        if out.is_err() {
            // 既定の帯を拡大して再試行 (SPEC §6.3)
            if let Ok(wide) = capture_band(x, y, target, 1800, 340) {
                focus = ocr::Focus::Line(wide.focus_y);
                out = ocr::run(&engine, &cfg, &wide.img, focus);
                used = wide;
            }
        }
        // 段落帯: OCRしたカーソル直下行を直下要素の全テキスト (UIA Name) から検索し、
        // 合致した段落で置き換える。画像外へ折り返された部分も正確に復元できる。
        if let Ok(o) = &mut out
            && let Some(full) = &probe.hover_text
            && let Some(line) = &o.focus_line
            && let Some(para) = paragraph_from_text(full, line) {
                o.text = para;
            }
        match out {
            Ok(o) => {
                let ms = t0.elapsed().as_millis();
                util::perf_log(cfg.perf_log, &format!("source OCR({engine}) {ms}ms"));
                // ログにはOCR対象領域だけを保存する (全体は保持画像=再OCR用)
                let log_img = ocr::crop_for_focus(&used.img, focus);
                let recog_id =
                    log_recog(&cfg, "hold", "ocr", &engine, ms, Some(&o.text), None, Some(&log_img), &ctx);
                post(main, generation, WorkerMsg::Source {
                    text: o.text.clone(),
                    method: "OCR",
                    engine: Some(engine.clone()),
                    img: Some(Arc::new(used.img)),
                    pin: o.text.contains('\n'),
                    anchor: (x, y),
                    focus,
                    ms,
                    recog_id,
                    app_title: ctx.title,
                    uia_path: ctx.uia_path,
                });
                dispatch_translation(generation, cfg, o, tr_engine, main, recog_id, &t0);
            }
            Err(e) => {
                log_recog(&cfg, "hold", "ocr", &engine, t0.elapsed().as_millis(), None, Some(&e), None, &ctx);
                post(main, generation, WorkerMsg::Error { msg: e, anchor: (x, y), engine: Some(engine) });
            }
        }
    });
}

fn translate(generation: u64, cfg: Config, text: String, engine: String, main: isize, recog_id: Option<i64>) {
    std::thread::spawn(move || {
        let t0 = Instant::now();
        match crate::translate::translate(&engine, &cfg, &text) {
            Ok(t) => {
                let ms = t0.elapsed().as_millis();
                util::perf_log(cfg.perf_log, &format!("translate {engine} {ms}ms"));
                log_trans_ok(&cfg, recog_id, ms, &t);
                post(main, generation, WorkerMsg::Translation { text: t.text, badge: t.badge, ms, recog_id });
            }
            Err(e) => {
                log_trans_err(&cfg, recog_id, &engine, t0.elapsed().as_millis(), &e);
                post(main, generation, WorkerMsg::TranslationFailed { msg: e, engine });
            }
        }
    });
}

/// OCRチップ切替: 保持画像(無ければ再キャプチャ)で選択エンジンOCR→再翻訳 (SPEC §8)
#[allow(clippy::too_many_arguments)]
pub fn reocr(
    generation: u64,
    img: Option<Arc<Captured>>,
    focus: ocr::Focus,
    x: i32,
    y: i32,
    target: isize,
    ocr_engine: String,
    tr_engine: String,
    cfg: Config,
    main: isize,
    anchor: (i32, i32),
) {
    std::thread::spawn(move || {
        init_com();
        let t0 = Instant::now();
        let mut hover_text: Option<String> = None;
        let (image, focus) = match img {
            Some(i) => (i, focus),
            None => {
                // 保持画像なし: 初回と同じ基準 (UIA検出領域優先) で再キャプチャする
                let probe = uia::probe_at_point(x, y);
                match capture_probe(x, y, target, &probe) {
                    Ok((b, kind)) => {
                        let f = focus_for(kind, b.focus_y);
                        hover_text = probe.hover_text;
                        (Arc::new(b.img), f)
                    }
                    Err(e) => {
                        post(main, generation, WorkerMsg::Error { msg: e, anchor, engine: None });
                        return;
                    }
                }
            }
        };
        let ctx = AppContext::capture(x, y, HWND(target as *mut _));
        let mut result = ocr::run(&ocr_engine, &cfg, &image, focus);
        // 段落帯の再キャプチャ時も、直下要素の全テキストから段落を復元する
        if let Ok(o) = &mut result
            && let Some(full) = &hover_text
            && let Some(line) = &o.focus_line
            && let Some(para) = paragraph_from_text(full, line) {
                o.text = para;
            }
        match result {
            Ok(o) => {
                let ms = t0.elapsed().as_millis();
                util::perf_log(cfg.perf_log, &format!("reocr {ocr_engine} {ms}ms"));
                // ログにはOCR対象領域だけを保存する
                let log_img = ocr::crop_for_focus(&image, focus);
                let recog_id =
                    log_recog(&cfg, "chip", "ocr", &ocr_engine, ms, Some(&o.text), None, Some(&log_img), &ctx);
                post(main, generation, WorkerMsg::Source {
                    text: o.text.clone(),
                    method: "OCR",
                    engine: Some(ocr_engine.clone()),
                    img: Some(image),
                    pin: o.text.contains('\n'),
                    anchor,
                    focus,
                    ms,
                    recog_id,
                    app_title: ctx.title,
                    uia_path: ctx.uia_path,
                });
                dispatch_translation(generation, cfg, o, tr_engine, main, recog_id, &t0);
            }
            Err(e) => {
                log_recog(&cfg, "chip", "ocr", &ocr_engine, t0.elapsed().as_millis(), None, Some(&e), None, &ctx);
                post(main, generation, WorkerMsg::Error { msg: e, anchor, engine: Some(ocr_engine) });
            }
        }
    });
}

/// 翻訳チップ切替: 現在の原文を選択エンジンで再翻訳 (SPEC §8)。
pub fn retranslate(generation: u64, engine: String, cfg: Config, text: String, main: isize, recog_id: Option<i64>) {
    translate(generation, cfg, text, engine, main, recog_id);
}

/// 範囲指定モード: 選択矩形をOCRして段落結合→翻訳、最初からピン留め (SPEC §3.2)
pub fn region_cycle(generation: u64, rect: RECT, cfg: Config, main: isize) {
    std::thread::spawn(move || {
        let tr_engine = effective_translator(&cfg);
        init_com();
        let t0 = Instant::now();
        let cx = (rect.left + rect.right) / 2;
        let cy = (rect.top + rect.bottom) / 2;
        let anchor = (rect.left, rect.bottom);

        let hwnd = unsafe {
            windows::Win32::UI::WindowsAndMessaging::WindowFromPoint(
                windows::Win32::Foundation::POINT { x: cx, y: cy },
            )
        };
        let root = unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetAncestor(
                hwnd,
                windows::Win32::UI::WindowsAndMessaging::GA_ROOT,
            )
        };
        if root.is_invalid() {
            post(main, generation, WorkerMsg::Error {
                msg: "このウィンドウは取得できません".into(),
                anchor,
                engine: None,
            });
            return;
        }
        let full = match capture::capture_window(root) {
            Ok(f) => f,
            Err(e) => {
                post(main, generation, WorkerMsg::Error { msg: e, anchor, engine: None });
                return;
            }
        };
        let r = capture::window_frame_rect(root);
        let rw = (r.right - r.left).max(1);
        let rh = (r.bottom - r.top).max(1);
        let sx = full.width as f32 / rw as f32;
        let sy = full.height as f32 / rh as f32;
        let crop = capture::crop(
            &full,
            (((rect.left - r.left) as f32) * sx) as i32,
            (((rect.top - r.top) as f32) * sy) as i32,
            (((rect.right - rect.left) as f32) * sx) as i32,
            (((rect.bottom - rect.top) as f32) * sy) as i32,
        );
        let Some(img) = crop else {
            post(main, generation, WorkerMsg::Error {
                msg: "選択範囲を切り出せませんでした".into(),
                anchor,
                engine: None,
            });
            return;
        };

        let engine = effective_ocr(&cfg);
        // Focus::All → 全行を段落結合
        let ctx = AppContext::capture(cx, cy, root);
        match ocr::run(&engine, &cfg, &img, ocr::Focus::All) {
            Ok(o) => {
                let ms = t0.elapsed().as_millis();
                util::perf_log(cfg.perf_log, &format!("region OCR({engine}) {ms}ms"));
                let recog_id =
                    log_recog(&cfg, "region", "ocr", &engine, ms, Some(&o.text), None, Some(&img), &ctx);
                post(main, generation, WorkerMsg::Source {
                    text: o.text.clone(),
                    method: "OCR",
                    engine: Some(engine.clone()),
                    img: Some(Arc::new(img)),
                    pin: true,
                    anchor,
                    focus: ocr::Focus::All,
                    ms,
                    recog_id,
                    app_title: ctx.title,
                    uia_path: ctx.uia_path,
                });
                dispatch_translation(generation, cfg, o, tr_engine, main, recog_id, &t0);
            }
            Err(e) => {
                log_recog(&cfg, "region", "ocr", &engine, t0.elapsed().as_millis(), None, Some(&e), None, &ctx);
                post(main, generation, WorkerMsg::Error { msg: e, anchor, engine: Some(engine) });
            }
        }
    });
}

/// 解説プロンプトを組み立てる (LLMプロファイル未設定時は None)。
/// アプリ名・UIAパスがあれば前提情報としてコンテキストブロックを付与する。
pub fn build_explain_prompt(cfg: &Config, text: &str, app_title: &str, uia_path: &str) -> Option<String> {
    let prof = cfg.active_profile()?;
    let mut prompt = cfg.fill_prompt(&prof.explain_prompt, text);
    if !app_title.is_empty() || !uia_path.is_empty() {
        prompt.push_str("\n\n[Context]");
        if !app_title.is_empty() {
            prompt.push_str(&format!("\nApplication: {app_title}"));
        }
        if !uia_path.is_empty() {
            prompt.push_str(&format!("\nUI Path: {uia_path}"));
        }
    }
    Some(prompt)
}

/// 解説の取得 (SPEC v0.2 §2.2.2): 成功時はDBへ保存してオーバーレイへ通知する。
/// profile はダイアログで選択されたAPIプロファイル名 (見つからなければアクティブを使用)。
pub fn explain(generation: u64, recog_id: i64, cfg: Config, prompt: String, profile: String, main: isize) {
    std::thread::spawn(move || {
        init_com();
        let result = cfg
            .api_profiles
            .iter()
            .find(|p| p.name == profile)
            .or_else(|| cfg.active_profile())
            .ok_or_else(|| "LLM APIプロファイルが設定されていません".to_string())
            .and_then(|prof| crate::llm_api::call(prof, &crate::llm_api::LlmRequest::text(&prompt)));
        match result {
            Ok(res) => {
                crate::logdb::save_explanation(recog_id, &res.text);
                post(main, generation, WorkerMsg::Explanation { text: res.text });
            }
            Err(e) => {
                post(main, generation, WorkerMsg::Error {
                    msg: format!("解説の取得に失敗しました: {e}"),
                    anchor: (0, 0),
                    engine: None,
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::paragraph_from_text;

    #[test]
    fn 完全一致で段落を取り出せる() {
        let full = "First paragraph here.\nThe quick brown fox jumps over the lazy dog and continues wrapping.\nThird paragraph.";
        let got = paragraph_from_text(full, "quick brown fox jumps");
        assert_eq!(got.as_deref(), Some("The quick brown fox jumps over the lazy dog and continues wrapping."));
    }

    #[test]
    fn 空白やケースのゆれを吸収する() {
        let full = "段落その一。\nこれは 折り返された テキストの段落です。長い文章が続きます。\n段落その三。";
        // Windows OCR がCJKに空白を挟んでも一致する
        let got = paragraph_from_text(full, "折り返された テキスト の段落");
        assert_eq!(got.as_deref(), Some("これは 折り返された テキストの段落です。長い文章が続きます。"));
    }

    #[test]
    fn 末尾が誤認識でも先頭キーで一致する() {
        let full = "aaa\nAn example paragraph that wraps at the right edge of the view.\nbbb";
        // 行末の誤認識 (edge→edqe) があっても先頭12文字で拾う
        let got = paragraph_from_text(full, "An example paragraph that wraps at the right edqe");
        assert_eq!(got.as_deref(), Some("An example paragraph that wraps at the right edge of the view."));
    }

    #[test]
    fn 短すぎるキーは不採用() {
        assert_eq!(paragraph_from_text("abc def ghi", "ab"), None);
    }

    #[test]
    fn 一致しなければ_none() {
        assert_eq!(paragraph_from_text("全く別のテキスト", "hello world example"), None);
    }
}
