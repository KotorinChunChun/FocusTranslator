// 認識・翻訳ワーカースレッド (SPEC §6)
// 各サイクルは世代番号 generation を持ち、main 側で古い世代の結果は破棄される。
use crate::capture::{self, Captured};
use crate::config::Config;
use crate::{ocr, translate, uia, util};
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
        img: Option<Arc<Captured>>,
        pin: bool,
        anchor: (i32, i32),
        ms: u128,
    },
    Translation {
        text: String,
        badge: Option<String>,
        ms: u128,
    },
    TranslationFailed {
        msg: String,
    },
    Error {
        msg: String,
        anchor: (i32, i32),
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
) -> Option<i64> {
    if !cfg.log_enabled {
        return None;
    }
    let id = crate::logdb::log_recognition(mode, method, engine, ms, text, error, image, cfg.debug_mode);
    crate::logdb::rotate(cfg.log_max_records);
    id
}

/// 翻訳成功ログを記録する(ログOFF時は何もしない)。
fn log_trans_ok(cfg: &Config, recog_id: Option<i64>, ms: u128, t: &translate::Translated) {
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

/// Gemini統合モードの翻訳ログを記録する(OCR側で取得した生応答・トークンを使う)。
fn log_trans_gemini(cfg: &Config, recog_id: Option<i64>, ms: u128, tr: &str, o: &ocr::OcrOutput) {
    if !cfg.log_enabled {
        return;
    }
    crate::logdb::log_translation(
        recog_id, "gemini", &cfg.source_lang, &cfg.target_lang, ms, false,
        Some(tr), None, None, o.raw_response.as_deref(), o.tokens_in, o.tokens_out,
    );
}

/// 同意が無いクラウド/外部OCRエンジンをローカルへ置き換える (SPEC §9: 同意なしで外部送信しない)
fn effective_ocr(cfg: &Config) -> String {
    let e = cfg.default_ocr.as_str();
    let allowed = match e {
        "gemini" => cfg.consent_image,
        "yomitoku" | "ndl" => cfg.consent_ext_ocr,
        _ => true,
    };
    if allowed && cfg.engine_available(e) { e.to_string() } else { "win".to_string() }
}

fn effective_translator(cfg: &Config) -> String {
    let e = cfg.default_translator.as_str();
    let allowed = match e {
        "deepl" | "google" | "gemini" => cfg.consent_text,
        _ => true,
    };
    if allowed { e.to_string() } else { "local".to_string() }
}

struct Band {
    img: Captured,
    focus_y: f32,
}

/// ポインタ直下ウィンドウをキャプチャし、カーソル周辺の帯を切り出す (SPEC §6.3)
fn capture_band(x: i32, y: i32, target: isize, bw: i32, bh: i32) -> Result<Band, String> {
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
    let rel_x = ((x - r.left) as f32 * scale_x) as i32;
    let rel_y = ((y - r.top) as f32 * scale_y) as i32;
    let left = rel_x - bw / 2;
    let top = rel_y - bh / 2;
    let band =
        capture::crop(&full, left, top, bw, bh).ok_or("このウィンドウは取得できません")?;
    let focus_y = (rel_y - top.max(0)) as f32;
    Ok(Band { img: band, focus_y })
}

/// ホールドモードの認識サイクル: UIA優先 → WGC帯OCR (SPEC §6.4)
pub fn recognize_cycle(generation: u64, x: i32, y: i32, target: isize, cfg: Config, main: isize) {
    std::thread::spawn(move || {
        init_com();
        let t0 = Instant::now();

        // 経路A: UIA
        if let Some(text) = uia::line_at_point(x, y) {
            let ms = t0.elapsed().as_millis();
            util::perf_log(cfg.perf_log, &format!("source UIA {ms}ms"));
            let recog_id = log_recog(&cfg, "hold", "uia", "uia", ms, Some(&text), None, None);
            post(main, generation, WorkerMsg::Source {
                text: text.clone(),
                method: "UIA",
                img: None,
                pin: false,
                anchor: (x, y),
                ms,
            });
            run_translation(generation, &cfg, &text, main, recog_id);
            return;
        }

        // 経路B: WGC + 帯OCR
        let engine = effective_ocr(&cfg);
        let band = match capture_band(x, y, target, 1200, 160) {
            Ok(b) => b,
            Err(e) => {
                log_recog(&cfg, "hold", "ocr", &engine, t0.elapsed().as_millis(), None, Some(&e), None);
                post(main, generation, WorkerMsg::Error { msg: e, anchor: (x, y) });
                return;
            }
        };
        let mut used = band;
        let mut out = ocr::run(&engine, &cfg, &used.img, Some(used.focus_y));
        if out.is_err() {
            // 帯を拡大して再試行 (SPEC §6.3)
            if let Ok(wide) = capture_band(x, y, target, 1800, 340) {
                out = ocr::run(&engine, &cfg, &wide.img, Some(wide.focus_y));
                used = wide;
            }
        }
        match out {
            Ok(o) => {
                let ms = t0.elapsed().as_millis();
                util::perf_log(cfg.perf_log, &format!("source OCR({engine}) {ms}ms"));
                let recog_id =
                    log_recog(&cfg, "hold", "ocr", &engine, ms, Some(&o.text), None, Some(&used.img));
                post(main, generation, WorkerMsg::Source {
                    text: o.text.clone(),
                    method: "OCR",
                    img: Some(Arc::new(used.img)),
                    pin: false,
                    anchor: (x, y),
                    ms,
                });
                if let Some(tr) = &o.translation {
                    // Gemini統合モード: 訳文も同時取得済み
                    let tms = t0.elapsed().as_millis();
                    log_trans_gemini(&cfg, recog_id, tms, tr, &o);
                    post(main, generation, WorkerMsg::Translation {
                        text: tr.clone(),
                        badge: Some("Gemini統合".into()),
                        ms: tms,
                    });
                } else {
                    run_translation(generation, &cfg, &o.text, main, recog_id);
                }
            }
            Err(e) => {
                log_recog(&cfg, "hold", "ocr", &engine, t0.elapsed().as_millis(), None, Some(&e), None);
                post(main, generation, WorkerMsg::Error { msg: e, anchor: (x, y) });
            }
        }
    });
}

fn run_translation(generation: u64, cfg: &Config, text: &str, main: isize, recog_id: Option<i64>) {
    let t0 = Instant::now();
    let engine = effective_translator(cfg);
    match translate::translate(&engine, cfg, text) {
        Ok(t) => {
            let ms = t0.elapsed().as_millis();
            util::perf_log(cfg.perf_log, &format!("translate {engine} {ms}ms"));
            log_trans_ok(cfg, recog_id, ms, &t);
            post(main, generation, WorkerMsg::Translation { text: t.text, badge: t.badge, ms });
        }
        Err(e) => {
            log_trans_err(cfg, recog_id, &engine, t0.elapsed().as_millis(), &e);
            post(main, generation, WorkerMsg::TranslationFailed { msg: e });
        }
    }
}

/// OCRチップ切替: 保持画像(無ければ再キャプチャ)で選択エンジンOCR→再翻訳 (SPEC §8)
#[allow(clippy::too_many_arguments)]
pub fn reocr(
    generation: u64,
    img: Option<Arc<Captured>>,
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
        let (image, focus) = match img {
            Some(i) => (i, None),
            None => match capture_band(x, y, target, 1200, 160) {
                Ok(b) => (Arc::new(b.img), Some(b.focus_y)),
                Err(e) => {
                    post(main, generation, WorkerMsg::Error { msg: e, anchor });
                    return;
                }
            },
        };
        match ocr::run(&ocr_engine, &cfg, &image, focus) {
            Ok(o) => {
                let ms = t0.elapsed().as_millis();
                util::perf_log(cfg.perf_log, &format!("reocr {ocr_engine} {ms}ms"));
                // チップ操作による再OCR: mode="chip"
                let recog_id =
                    log_recog(&cfg, "chip", "ocr", &ocr_engine, ms, Some(&o.text), None, Some(&image));
                post(main, generation, WorkerMsg::Source {
                    text: o.text.clone(),
                    method: "OCR",
                    img: Some(image),
                    pin: true,
                    anchor,
                    ms,
                });
                if let Some(tr) = &o.translation {
                    let tms = t0.elapsed().as_millis();
                    log_trans_gemini(&cfg, recog_id, tms, tr, &o);
                    post(main, generation, WorkerMsg::Translation {
                        text: tr.clone(),
                        badge: Some("Gemini統合".into()),
                        ms: tms,
                    });
                } else {
                    retranslate_inner(generation, &tr_engine, &cfg, &o.text, main, recog_id);
                }
            }
            Err(e) => {
                log_recog(&cfg, "chip", "ocr", &ocr_engine, t0.elapsed().as_millis(), None, Some(&e), None);
                post(main, generation, WorkerMsg::TranslationFailed { msg: e });
            }
        }
    });
}

/// 翻訳チップ切替: 現在の原文を選択エンジンで再翻訳 (SPEC §8)。
/// 原文は前サイクルのものを流用するため新規の認識ログは作らない(翻訳ログのみ、recog_id=None)。
pub fn retranslate(generation: u64, engine: String, cfg: Config, text: String, main: isize) {
    std::thread::spawn(move || {
        init_com();
        retranslate_inner(generation, &engine, &cfg, &text, main, None);
    });
}

fn retranslate_inner(
    generation: u64,
    engine: &str,
    cfg: &Config,
    text: &str,
    main: isize,
    recog_id: Option<i64>,
) {
    let t0 = Instant::now();
    match translate::translate(engine, cfg, text) {
        Ok(t) => {
            let ms = t0.elapsed().as_millis();
            util::perf_log(cfg.perf_log, &format!("translate {engine} {ms}ms"));
            log_trans_ok(cfg, recog_id, ms, &t);
            post(main, generation, WorkerMsg::Translation { text: t.text, badge: t.badge, ms });
        }
        Err(e) => {
            log_trans_err(cfg, recog_id, engine, t0.elapsed().as_millis(), &e);
            post(main, generation, WorkerMsg::TranslationFailed { msg: e });
        }
    }
}

/// 範囲指定モード: 選択矩形をOCRして段落結合→翻訳、最初からピン留め (SPEC §3.2)
pub fn region_cycle(generation: u64, rect: RECT, cfg: Config, main: isize) {
    std::thread::spawn(move || {
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
            });
            return;
        }
        let full = match capture::capture_window(root) {
            Ok(f) => f,
            Err(e) => {
                post(main, generation, WorkerMsg::Error { msg: e, anchor });
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
            });
            return;
        };

        let engine = effective_ocr(&cfg);
        // focus_y = None → 全行を段落結合
        match ocr::run(&engine, &cfg, &img, None) {
            Ok(o) => {
                let ms = t0.elapsed().as_millis();
                util::perf_log(cfg.perf_log, &format!("region OCR({engine}) {ms}ms"));
                let recog_id =
                    log_recog(&cfg, "region", "ocr", &engine, ms, Some(&o.text), None, Some(&img));
                post(main, generation, WorkerMsg::Source {
                    text: o.text.clone(),
                    method: "OCR",
                    img: Some(Arc::new(img)),
                    pin: true,
                    anchor,
                    ms,
                });
                if let Some(tr) = &o.translation {
                    let tms = t0.elapsed().as_millis();
                    log_trans_gemini(&cfg, recog_id, tms, tr, &o);
                    post(main, generation, WorkerMsg::Translation {
                        text: tr.clone(),
                        badge: Some("Gemini統合".into()),
                        ms: tms,
                    });
                } else {
                    run_translation(generation, &cfg, &o.text, main, recog_id);
                }
            }
            Err(e) => {
                log_recog(&cfg, "region", "ocr", &engine, t0.elapsed().as_millis(), None, Some(&e), None);
                post(main, generation, WorkerMsg::Error { msg: e, anchor });
            }
        }
    });
}
