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
        img: Option<Arc<Captured>>,
        pin: bool,
        anchor: (i32, i32),
        focus_y: Option<f32>,
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
    app_exe: Option<&str>,
    app_title: Option<&str>,
    uia_path: Option<&str>,
) -> Option<i64> {
    if !cfg.log_enabled {
        return None;
    }
    let id = crate::logdb::log_recognition(mode, method, engine, ms, text, error, image, cfg.debug_mode, app_exe, app_title, uia_path);
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

/// Gemini統合モードの翻訳ログを記録する(OCR側で取得した生応答・トークンを使う)。
fn log_trans_gemini(cfg: &Config, recog_id: Option<i64>, ms: u128, tr: &str, o: &ocr::OcrOutput) {
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
        let tr_engine = effective_translator(&cfg);
        init_com();
        let t0 = Instant::now();

        // 経路A: UIA
        let (app_title, uia_path) = uia::get_context_at_point(x, y, HWND(target as *mut _));
        if let Some(text) = uia::line_at_point(x, y) {
            let ms = t0.elapsed().as_millis();
            util::perf_log(cfg.perf_log, &format!("source UIA {ms}ms"));
            let (app_exe, _): (Option<String>, Option<String>) = crate::util::get_window_context(HWND(target as *mut _));
            let recog_id = log_recog(&cfg, "hold", "uia", "uia", ms, Some(&text), None, None, app_exe.as_deref(), Some(&app_title), Some(&uia_path));
            post(main, generation, WorkerMsg::Source {
                text: text.clone(),
                method: "UIA",
                img: None,
                pin: true,
                anchor: (x, y),
                focus_y: None,
                ms,
                recog_id,
                app_title: app_title.clone(),
                uia_path: uia_path.clone(),
            });
            translate(generation, cfg, text, tr_engine, main, recog_id);
            return;
        }

        // 経路B: WGC + 帯OCR
        let engine = effective_ocr(&cfg);
        let band = match capture_band(x, y, target, 1200, 160) {
            Ok(b) => b,
            Err(e) => {
                let (app_exe, app_title): (Option<String>, Option<String>) = crate::util::get_window_context(HWND(target as *mut _));
                log_recog(&cfg, "hold", "ocr", &engine, t0.elapsed().as_millis(), None, Some(&e), None, app_exe.as_deref(), app_title.as_deref(), None);
                post(main, generation, WorkerMsg::Error { msg: e, anchor: (x, y), engine: None });
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
                let (app_exe, _): (Option<String>, Option<String>) = crate::util::get_window_context(HWND(target as *mut _));
                let recog_id =
                    log_recog(&cfg, "hold", "ocr", &engine, ms, Some(&o.text), None, Some(&used.img), app_exe.as_deref(), Some(&app_title), Some(&uia_path));
                post(main, generation, WorkerMsg::Source {
                    text: o.text.clone(),
                    method: "OCR",
                    img: Some(Arc::new(used.img)),
                    pin: o.text.contains('\n'),
                    anchor: (x, y),
                    focus_y: Some(used.focus_y),
                    ms,
                    recog_id,
                    app_title: app_title.clone(),
                    uia_path: uia_path.clone(),
                });
                if let Some(tr) = &o.translation {
                    // Gemini統合モード: 訳文も同時取得済み
                    let tms = t0.elapsed().as_millis();
                    log_trans_gemini(&cfg, recog_id, tms, tr, &o);
                    post(main, generation, WorkerMsg::Translation {
                        text: tr.clone(),
                        badge: Some("Gemini統合".into()),
                        ms: tms,
                        recog_id,
                    });
                } else {
                    translate(generation, cfg, o.text, tr_engine, main, recog_id);
                }
            }
            Err(e) => {
                let (app_exe, app_title): (Option<String>, Option<String>) = crate::util::get_window_context(HWND(target as *mut _));
                log_recog(&cfg, "hold", "ocr", &engine, t0.elapsed().as_millis(), None, Some(&e), None, app_exe.as_deref(), app_title.as_deref(), None);
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
    focus_y: Option<f32>,
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
        let (image, last_focus_y) = match img {
            Some(i) => (i, focus_y),
            None => match capture_band(x, y, target, 1200, 160) {
                Ok(b) => (Arc::new(b.img), Some(b.focus_y)),
                Err(e) => {
                    post(main, generation, WorkerMsg::Error { msg: e, anchor, engine: None });
                    return;
                }
            },
        };
        match ocr::run(&ocr_engine, &cfg, &image, last_focus_y) {
            Ok(o) => {
                let ms = t0.elapsed().as_millis();
                util::perf_log(cfg.perf_log, &format!("reocr {ocr_engine} {ms}ms"));
                let (app_title, uia_path) = uia::get_context_at_point(x, y, HWND(target as *mut _));
                let (app_exe, _): (Option<String>, Option<String>) = crate::util::get_window_context(HWND(target as *mut _));
                let recog_id =
                    log_recog(&cfg, "chip", "ocr", &ocr_engine, ms, Some(&o.text), None, Some(&image), app_exe.as_deref(), Some(&app_title), Some(&uia_path));
                post(main, generation, WorkerMsg::Source {
                    text: o.text.clone(),
                    method: "OCR",
                    img: Some(image),
                    pin: o.text.contains('\n'),
                    anchor,
                    focus_y: last_focus_y,
                    ms,
                    recog_id,
                    app_title,
                    uia_path,
                });
                if let Some(tr) = &o.translation {
                    let tms = t0.elapsed().as_millis();
                    log_trans_gemini(&cfg, recog_id, tms, tr, &o);
                    post(main, generation, WorkerMsg::Translation {
                        text: tr.clone(),
                        badge: Some("Gemini統合".into()),
                        ms: tms,
                        recog_id,
                    });
                } else {
                    translate(generation, cfg, o.text, tr_engine, main, recog_id);
                }
            }
            Err(e) => {
                let (app_exe, app_title): (Option<String>, Option<String>) = crate::util::get_window_context(HWND(target as *mut _));
                log_recog(&cfg, "chip", "ocr", &ocr_engine, t0.elapsed().as_millis(), None, Some(&e), None, app_exe.as_deref(), app_title.as_deref(), None);
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
        // focus_y = None → 全行を段落結合
        match ocr::run(&engine, &cfg, &img, None) {
            Ok(o) => {
                let ms = t0.elapsed().as_millis();
                util::perf_log(cfg.perf_log, &format!("region OCR({engine}) {ms}ms"));
                let (app_title, uia_path) = uia::get_context_at_point(cx, cy, root);
                let (app_exe, _): (Option<String>, Option<String>) = crate::util::get_window_context(root);
                let recog_id =
                    log_recog(&cfg, "region", "ocr", &engine, ms, Some(&o.text), None, Some(&img), app_exe.as_deref(), Some(&app_title), Some(&uia_path));
                post(main, generation, WorkerMsg::Source {
                    text: o.text.clone(),
                    method: "OCR",
                    img: Some(Arc::new(img)),
                    pin: true,
                    anchor,
                    focus_y: None,
                    ms,
                    recog_id,
                    app_title,
                    uia_path,
                });
                if let Some(tr) = &o.translation {
                    let tms = t0.elapsed().as_millis();
                    log_trans_gemini(&cfg, recog_id, tms, tr, &o);
                    post(main, generation, WorkerMsg::Translation {
                        text: tr.clone(),
                        badge: Some("Gemini統合".into()),
                        ms: tms,
                        recog_id,
                    });
                } else {
                    translate(generation, cfg, o.text, tr_engine, main, recog_id);
                }
            }
            Err(e) => {
                let (app_exe, app_title): (Option<String>, Option<String>) = crate::util::get_window_context(root);
                log_recog(&cfg, "region", "ocr", &engine, t0.elapsed().as_millis(), None, Some(&e), None, app_exe.as_deref(), app_title.as_deref(), None);
                post(main, generation, WorkerMsg::Error { msg: e, anchor, engine: Some(engine) });
            }
        }
    });
}

pub fn build_explain_prompt(cfg: &Config, text: &str) -> Option<String> {
    let prof = cfg.active_profile()?;
    let glossary_text = if cfg.glossary.is_empty() {
        String::new()
    } else {
        let lines = cfg.glossary.iter().map(|e| format!("{}={}", e.source, e.target)).collect::<Vec<_>>().join("\n");
        format!("Glossary:\n{}", lines)
    };
    Some(prof.explain_prompt
        .replace("{{source_lang}}", &cfg.source_lang)
        .replace("{{target_lang}}", &cfg.target_lang)
        .replace("{{text}}", text)
        .replace("{{glossary}}", &glossary_text))
}

pub fn explain(generation: u64, recog_id: i64, cfg: Config, prompt: String, main: isize) {
    std::thread::spawn(move || {
        init_com();
        let Some(prof) = cfg.active_profile() else {
            post(main, generation, WorkerMsg::Error { msg: "LLM APIプロファイルが設定されていません".into(), anchor: (0,0), engine: None });
            return;
        };
        let key = prof.get_key();
        if key.is_empty() {
            post(main, generation, WorkerMsg::Error { msg: format!("APIキーが未設定です ({})", prof.name), anchor: (0,0), engine: None });
            return;
        }

        let expl_text = match prof.api_type {
            crate::config::ApiType::Gemini => {
                let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", prof.model_name);
                let body = serde_json::json!({
                    "contents": [{ "parts": [{ "text": prompt }] }]
                });
                ureq::post(&url).header("x-goog-api-key", &key).send_json(&body).ok()
                    .and_then(|mut r| r.body_mut().read_json::<serde_json::Value>().ok())
                    .and_then(|v| v["candidates"][0]["content"]["parts"][0]["text"].as_str().map(|s| s.trim().to_string()))
            }
            crate::config::ApiType::OpenAI => {
                let url = if prof.api_url.is_empty() { "https://api.openai.com/v1/chat/completions" } else { &prof.api_url };
                let body = serde_json::json!({
                    "model": prof.model_name,
                    "messages": [
                        { "role": "user", "content": prompt }
                    ]
                });
                ureq::post(url).header("Authorization", format!("Bearer {}", key)).send_json(&body).ok()
                    .and_then(|mut r| r.body_mut().read_json::<serde_json::Value>().ok())
                    .and_then(|v| v["choices"][0]["message"]["content"].as_str().map(|s| s.trim().to_string()))
            }
            crate::config::ApiType::Claude => {
                let url = if prof.api_url.is_empty() { "https://api.anthropic.com/v1/messages" } else { &prof.api_url };
                let body = serde_json::json!({
                    "model": prof.model_name,
                    "max_tokens": 1024,
                    "messages": [
                        { "role": "user", "content": prompt }
                    ]
                });
                ureq::post(url).header("x-api-key", &key).header("anthropic-version", "2023-06-01").send_json(&body).ok()
                    .and_then(|mut r| r.body_mut().read_json::<serde_json::Value>().ok())
                    .and_then(|v| v["content"][0]["text"].as_str().map(|s| s.trim().to_string()))
            }
        };

        if let Some(expl) = expl_text {
            crate::logdb::save_explanation(recog_id, &expl);
            post(main, generation, WorkerMsg::Explanation { text: expl });
        } else {
            post(main, generation, WorkerMsg::Error { msg: "解説の取得に失敗しました".into(), anchor: (0,0), engine: None });
        }
    });
}
