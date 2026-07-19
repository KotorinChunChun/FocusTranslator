// 認識・翻訳ワーカースレッド (SPEC §6)
// 各サイクルは世代番号 generation を持ち、main 側で古い世代の結果は破棄される。
use crate::capture::Captured;
use crate::capture_plan;
use crate::config::Config;
use crate::{ocr, uia, util};
use std::sync::Arc;
use std::time::Instant;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// 認識結果の通知内容 (WorkerMsg::Source のペイロード)。
/// フィールド数が多く他のバリアントとのサイズ差が大きいため Box で運ぶ。
pub struct SourceMsg {
    pub text: String,
    pub method: &'static str,
    /// 実際に使ったOCRエンジンキー (UIA経路では None)
    pub engine: Option<String>,
    pub img: Option<Arc<Captured>>,
    pub pin: bool,
    pub anchor: (i32, i32),
    /// 保持画像の行選択モード (再OCR時に同じモードで認識する)
    pub focus: ocr::Focus,
    pub ms: u128,
    pub capture_id: Option<i64>,
    pub recog_id: Option<i64>,
    /// キャプチャ時点の対象アプリコンテキスト (exe名/タイトル/UIAパス/選択文字列)
    pub ctx: AppContext,
}

/// ワーカーからメインスレッド（ウィンドウプロシージャ）への通知。
/// バックグラウンド処理の完了や進捗をメインスレッドに伝えます。
pub enum WorkerMsg {
    Source(Box<SourceMsg>),
    Translation {
        text: String,
        badge: Option<String>,
        ms: u128,
        recog_id: Option<i64>,
    },
    TranslationSkipped {
        msg: String,
    },
    TranslationFailed {
        msg: String,
    },
    Error {
        msg: String,
        anchor: (i32, i32),
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
            crate::app_state::WM_APP_WORKER,
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

/// 翻訳対象アプリのコンテキスト (exe名 / ウィンドウタイトル / UIA要素パス; SPEC v0.3 §2.3.1)
#[derive(Clone)]
pub struct AppContext {
    pub exe: Option<String>,
    pub title: String,
    /// ログ・コピー・解説プロンプト用のパス文字列。子孫連結ノードがあれば全文を追記する
    /// (ボタン表示は10文字程度に切り詰めるが、ログには常に全文を残すため)。
    pub uia_path: String,
    /// UIAパスの各ノード (ボタン化してオーバーレイに表示。クリックでOCRの代わりに採用)
    pub uia_nodes: Vec<uia::UiaPathNode>,
    /// カーソル位置要素のUIA ControlType名 (入力内容ログ用; SPECv0.4.8追補)
    pub control_type: Option<String>,
    /// カーソル位置要素で選択中のテキスト (SPECv0.5追補)。
    /// 「選択中の文字列」チップの活性判定・全文ツールチップに使う。
    pub selected_text: Option<String>,
}

impl AppContext {
    fn capture(x: i32, y: i32, target: HWND) -> Self {
        let (exe, title) = util::get_window_context(target);
        let uia_nodes = uia::path_nodes_at_point(x, y);
        let control_type = uia::control_type_at_point(x, y);
        AppContext {
            exe,
            title: title.unwrap_or_default(),
            uia_path: build_uia_path_log(&uia_nodes),
            uia_nodes,
            control_type,
            selected_text: None,
        }
    }

    /// 認識結果を表す WorkerMsg::Source を組み立てる。アプリ情報 (exe/title/uia) は
    /// このコンテキストから埋めるため、各認識経路 (ホールド/範囲/チップ/画像編集) の
    /// 呼び出し側は認識固有の値だけを渡せばよい。
    #[allow(clippy::too_many_arguments)]
    fn source_msg(
        &self,
        text: String,
        method: &'static str,
        engine: Option<String>,
        img: Option<Arc<Captured>>,
        pin: bool,
        anchor: (i32, i32),
        focus: ocr::Focus,
        ms: u128,
        capture_id: Option<i64>,
        recog_id: Option<i64>,
    ) -> WorkerMsg {
        WorkerMsg::Source(Box::new(SourceMsg {
            text,
            method,
            engine,
            img,
            pin,
            anchor,
            focus,
            ms,
            capture_id,
            recog_id,
            ctx: self.clone(),
        }))
    }
}

/// AppContext からプロンプト置換用コンテキストを組み立てる (SPECv0.4 §7.1)。
/// ocr_engine はUIA経路なら空文字を渡す。原文・訳文は呼び出し側/下位層で補われる。
fn prompt_ctx(ctx: &AppContext, ocr_engine: &str) -> crate::config::PromptContext {
    crate::config::PromptContext {
        app_title: ctx.title.clone(),
        app_exe: ctx.exe.clone().unwrap_or_default(),
        uia_path: ctx.uia_path.clone(),
        ocr_engine: ocr_engine.to_string(),
        ..Default::default()
    }
}

/// ログ・コピー用のパス文字列を組み立てる。祖先ノードのラベルを " > " で連結し、
/// 子孫連結ノードがあれば末尾に全文(未省略)を追記する。
fn build_uia_path_log(nodes: &[uia::UiaPathNode]) -> String {
    let labels: Vec<&str> = nodes
        .iter()
        .filter(|n| n.kind == uia::NodeKind::Ancestor)
        .map(|n| n.label.as_str())
        .collect();
    let mut s = labels.join(" > ");
    if let Some(children) = nodes.iter().find(|n| n.kind == uia::NodeKind::ChildrenConcat)
        && !children.text.is_empty() {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str("[子要素] ");
            s.push_str(&children.text);
        }
    s
}

/// 入力(キャプチャ)ログを記録し capture_id を返す(ログOFF時は None)。
/// 画像はデバッグモード時のみPNG保存される。ローテーションもここで行う。
fn log_cap(cfg: &Config, mode: &str, ctx: &AppContext, image: Option<&Captured>) -> Option<i64> {
    if !cfg.log_enabled {
        return None;
    }
    let id = crate::logdb::log_capture(
        mode, ctx.exe.as_deref(), Some(&ctx.title), Some(&ctx.uia_path), ctx.control_type.as_deref(),
        image, cfg.debug_mode,
    );
    crate::logdb::rotate(cfg.log_max_records);
    id
}

/// 認識ログを記録し recognition_id を返す(ログOFF時・capture未記録時は None)。
/// image_hash は同一画像+同一エンジンでの再OCR判定に使う (SPECv0.4追補)。
#[allow(clippy::too_many_arguments)]
fn log_recog(
    cfg: &Config,
    capture_id: Option<i64>,
    method: &str,
    engine: &str,
    ms: u128,
    text: Option<&str>,
    error: Option<&str>,
    image_hash: Option<&str>,
) -> Option<i64> {
    if !cfg.log_enabled {
        return None;
    }
    crate::logdb::log_recognition(capture_id?, method, engine, ms, text, error, image_hash)
}

/// engine=llm のときだけアクティブプロファイル名を返す (translations.llm_profile 用)
fn llm_profile_of(cfg: &Config, engine: &str) -> Option<String> {
    (engine == "llm").then(|| cfg.active_api_profile.clone())
}

/// 翻訳成功ログを記録する(ログOFF時は何もしない)。
/// 同一入力+同一エンジンのキャッシュヒット時(translate::translate内のメモリキャッシュ)は
/// 再翻訳しておらず、ログにも新規記録しない (SPECv0.4追補: OCRのハッシュキャッシュと同様の扱い)。
fn log_trans_ok(cfg: &Config, recog_id: Option<i64>, ms: u128, t: &crate::translate::Translated) {
    let Some(rid) = recog_id else { return };
    if !cfg.log_enabled {
        return;
    }
    if let Some(cache_rid) = t.db_cache_recog_id {
        // DBキャッシュ(同一request_json)ヒット: 別の親なら同内容を新規に追記する
        // (SPECv0.4.8追補)。同じ親なら何もしない(既に記録済みのため)。
        if cache_rid == rid {
            return;
        }
        crate::logdb::log_translation(
            rid,
            &t.engine,
            llm_profile_of(cfg, &t.engine).as_deref(),
            &t.source_lang,
            &t.target_lang,
            0,
            true,
            Some(&t.text),
            None,
            t.detail.request_json.as_deref(),
            t.detail.response_json.as_deref(),
            Some(0),
            Some(0),
        );
        return;
    }
    if t.cache_hit {
        return;
    }
    crate::logdb::log_translation(
        rid,
        &t.engine,
        llm_profile_of(cfg, &t.engine).as_deref(),
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
    let Some(rid) = recog_id else { return };
    if !cfg.log_enabled {
        return;
    }
    crate::logdb::log_translation(
        rid, engine, llm_profile_of(cfg, engine).as_deref(), &cfg.source_lang, &cfg.target_lang,
        ms, false, None, Some(err), None, None, None, None,
    );
}

/// LLM統合モードの翻訳ログを記録する(OCR側で取得した生応答・トークンを使う)。
fn log_trans_llm(cfg: &Config, recog_id: Option<i64>, ms: u128, tr: &str, o: &ocr::OcrOutput) {
    let Some(rid) = recog_id else { return };
    if !cfg.log_enabled {
        return;
    }
    crate::logdb::log_translation(
        rid, "llm", Some(&cfg.active_api_profile), &cfg.source_lang, &cfg.target_lang, ms, false,
        Some(tr), None, None, o.raw_response.as_deref(), o.tokens_in, o.tokens_out,
    );
}

/// 同意が無いクラウド/外部OCRエンジンをローカルへ置き換える (SPEC §9: 同意なしで外部送信しない)
fn effective_ocr(cfg: &Config) -> String {
    let e = cfg.default_ocr.as_str();
    let allowed = match e {
        "llm" => cfg.consent_image,
        _ => true,
    };
    if allowed && cfg.engine_available(e) {
        e.to_string()
    } else if cfg.engine_available("oneocr") {
        "oneocr".to_string()
    } else {
        "win".to_string()
    }
}

fn effective_translator(cfg: &Config) -> String {
    let e = cfg.default_translator.as_str();
    let allowed = match e {
        "deepl" | "google" | "llm" => cfg.consent_text,
        _ => true,
    };
    if allowed { e.to_string() } else { "local".to_string() }
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

/// OCR結果に統合訳文が含まれていればそのまま表示し、無ければ翻訳ワーカーへ回す
#[allow(clippy::too_many_arguments)]
fn dispatch_translation(
    generation: u64,
    cfg: Config,
    o: ocr::OcrOutput,
    tr_engine: String,
    main: isize,
    recog_id: Option<i64>,
    pc: crate::config::PromptContext,
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
        // 自動サイクル(ホールド/範囲/再OCR)からの翻訳は元言語チェックを適用する (SPECv0.4.8追補)
        translate(generation, cfg, o.text, tr_engine, main, recog_id, pc, false);
    }
}

/// UIA経路(選択文字列/カーソル位置テキスト)の採用結果を記録・送信・翻訳する共通処理。
/// 選択文字列(経路S)とカーソル位置テキスト(経路A)のどちらも、OCRを行わず同じ形で
/// 扱えるため recognize_cycle から共用する。
#[allow(clippy::too_many_arguments)]
fn adopt_uia_text(
    generation: u64,
    cfg: Config,
    text: String,
    tr_engine: String,
    main: isize,
    x: i32,
    y: i32,
    target: isize,
    ctx: &AppContext,
    probe: &uia::UiaProbe,
    t0: &Instant,
) {
    let ms = t0.elapsed().as_millis();
    util::perf_log(cfg.perf_log, &format!("source UIA {ms}ms"));

    // OCRは行っていないが、後でOCRエンジンへ切り替えた際に再キャプチャ不要で使えるよう、
    // また認識ログにも紐づけられるよう、この時点で検出領域の画像を撮影しておく。
    let cap = capture_plan::capture_probe(x, y, target, probe).ok();
    let focus = cap
        .as_ref()
        .map(|(b, kind)| capture_plan::focus_for(*kind, b.focus_y))
        .unwrap_or(ocr::Focus::All);
    let log_img: Option<Captured> =
        cap.as_ref().map(|(b, _)| ocr::crop_for_focus(&b.img, focus).into_owned());
    let img = cap.map(|(b, _)| Arc::new(b.img));

    let capture_id = log_cap(&cfg, "hold", ctx, log_img.as_ref());
    let hash = log_img.as_ref().map(crate::capture::hash_hex);
    let recog_id = log_recog(&cfg, capture_id, "uia", "uia", ms, Some(&text), None, hash.as_deref());
    post(main, generation, ctx.source_msg(
        text.clone(), "UIA", None, img, false, (x, y), focus, ms, capture_id, recog_id,
    ));
    // UIA経路なので ocr_engine は空文字 (SPECv0.4 §7.1)
    let pc = prompt_ctx(ctx, "");
    translate(generation, cfg, text, tr_engine, main, recog_id, pc, false);
}

/// ホールドモードの認識サイクル: 選択文字列 → UIA → WGCキャプチャOCR (SPEC §6.4, SPECv0.5追補)
/// キャプチャ領域は UIA検出結果 (行矩形/要素/直下要素) を優先し、無ければ既定帯。
pub fn recognize_cycle(generation: u64, x: i32, y: i32, target: isize, cfg: Config, main: isize) {
    std::thread::spawn(move || {
        let tr_engine = effective_translator(&cfg);
        init_com();
        let t0 = Instant::now();
        let mut ctx = AppContext::capture(x, y, HWND(target as *mut _));
        let probe = uia::probe_at_point(x, y);
        ctx.selected_text = probe.selected_text.clone();

        // 経路S: 選択中の文字列 (最優先。OCRはもちろん、カーソル位置のUIAテキストよりも優先)
        if let Some(sel) = probe.selected_text.clone() {
            adopt_uia_text(generation, cfg, sel, tr_engine, main, x, y, target, &ctx, &probe, &t0);
            return;
        }

        // 経路A: UIA
        if let Some(text) = probe.text.clone() {
            adopt_uia_text(generation, cfg, text, tr_engine, main, x, y, target, &ctx, &probe, &t0);
            return;
        }

        // 経路B: WGC + キャプチャOCR (直下要素 → 段落帯 → 既定帯)
        let engine = effective_ocr(&cfg);
        let cap = match capture_plan::capture_probe(x, y, target, &probe) {
            Ok(c) => c,
            Err(e) => {
                let capture_id = log_cap(&cfg, "hold", &ctx, None);
                log_recog(&cfg, capture_id, "ocr", &engine, t0.elapsed().as_millis(), None, Some(&e), None);
                post(main, generation, WorkerMsg::Error { msg: e, anchor: (x, y) });
                return;
            }
        };
        let (band, kind) = cap;
        let mut used = band;
        let mut focus = capture_plan::focus_for(kind, used.focus_y);
        let pc = prompt_ctx(&ctx, &engine);
        let mut out = ocr::run(&engine, &cfg, &used.img, focus, &pc);
        if out.is_err() {
            // 既定の帯を拡大して再試行 (SPEC §6.3)
            if let Ok(wide) = capture_plan::capture_band(x, y, target, 1800, 340) {
                focus = ocr::Focus::Line(wide.focus_y);
                out = ocr::run(&engine, &cfg, &wide.img, focus, &pc);
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
                let hash = crate::capture::hash_hex(&log_img);
                let capture_id = log_cap(&cfg, "hold", &ctx, Some(&log_img));
                let recog_id = log_recog(&cfg, capture_id, "ocr", &engine, ms, Some(&o.text), None, Some(&hash));
                let pin = false;
                post(main, generation, ctx.source_msg(
                    o.text.clone(), "OCR", Some(engine.clone()), Some(Arc::new(used.img)), pin,
                    (x, y), focus, ms, capture_id, recog_id,
                ));
                dispatch_translation(generation, cfg, o, tr_engine, main, recog_id, pc, &t0);
            }
            Err(e) => {
                let capture_id = log_cap(&cfg, "hold", &ctx, None);
                log_recog(&cfg, capture_id, "ocr", &engine, t0.elapsed().as_millis(), None, Some(&e), None);
                post(main, generation, WorkerMsg::Error { msg: e, anchor: (x, y) });
            }
        }
    });
}

/// 翻訳を実行する。force=false のときは元言語と明らかに異なるテキスト (SPECv0.4.8追補)
/// をAPI/ローカルモデルを呼ばずにスキップする。自動サイクル(ホールド/範囲/再OCR後)は
/// force=false、翻訳チップ切替・言語反転・原文編集後の再翻訳など明示的な操作は force=true とする。
#[allow(clippy::too_many_arguments)]
fn translate(
    generation: u64,
    cfg: Config,
    text: String,
    engine: String,
    main: isize,
    recog_id: Option<i64>,
    pc: crate::config::PromptContext,
    force: bool,
) {
    if !force && !crate::translate::is_source_lang_text(&cfg.source_lang, &text) {
        let msg = format!(
            "元言語 ({}) のテキストではないため翻訳をスキップしました。",
            cfg.source_lang
        );
        post(main, generation, WorkerMsg::TranslationSkipped { msg });
        return;
    }
    std::thread::spawn(move || {
        let t0 = Instant::now();
        match crate::translate::translate(&engine, &cfg, &text, &pc) {
            Ok(t) => {
                let ms = t0.elapsed().as_millis();
                util::perf_log(cfg.perf_log, &format!("translate {engine} {ms}ms"));
                log_trans_ok(&cfg, recog_id, ms, &t);
                post(main, generation, WorkerMsg::Translation { text: t.text, badge: t.badge, ms, recog_id });
            }
            Err(e) => {
                log_trans_err(&cfg, recog_id, &engine, t0.elapsed().as_millis(), &e);
                post(main, generation, WorkerMsg::TranslationFailed { msg: e });
            }
        }
    });
}

/// 再認識ジョブ (reocr の引数一式)。OCRチップ切替と画像編集後の再認識で共用する。
pub struct ReocrJob {
    pub generation: u64,
    pub capture_id: Option<i64>,
    /// Some: 保持画像を再認識 (再キャプチャ・UIA再プローブなし)。None: カーソル位置を再キャプチャ。
    pub img: Option<Arc<Captured>>,
    /// 保持画像の行選択モード (画像編集後の再認識はクロップ済みのため Focus::All を渡す)
    pub focus: ocr::Focus,
    /// 再キャプチャ時 (img=None) に使うカーソル座標と対象ウィンドウ
    pub x: i32,
    pub y: i32,
    pub target: isize,
    pub ocr_engine: String,
    pub tr_engine: String,
    pub cfg: Config,
    pub main: isize,
    pub anchor: (i32, i32),
    /// キャプチャ時点のUIAコンテキスト (保持画像がある場合に使用)
    pub ctx: AppContext,
    /// true: 常にピン留め (画像編集後の再認識)。false: 結果が複数行のときのみピン留め
    pub force_pin: bool,
    /// パフォーマンスログの表示名 ("reocr" / "reocr_edited")
    pub perf_label: &'static str,
}

/// 再認識: 保持画像(無ければ再キャプチャ)で選択エンジンOCR→再翻訳 (SPEC §8)。
/// 保持画像がある場合は同じ capture_id を引き継ぎ、新しいcaptureは作らず読み取り結果を
/// 追記する (SPECv0.4追補)。同一画像+同一エンジンの認識が既にログにあれば、再OCRせず
/// その結果を再利用する (切替操作の記録としてrecognition行は追記する)。
/// 画像編集(トリミング)適用後の再認識 (SPECv0.4 §4-3, §8.2.1) もここを通る
/// (img=Some, focus=All, force_pin=true)。
pub fn reocr(job: ReocrJob) {
    std::thread::spawn(move || {
        init_com();
        let ReocrJob {
            generation, capture_id, img, focus, x, y, target, ocr_engine, tr_engine, cfg, main,
            anchor, ctx: held_ctx, force_pin, perf_label,
        } = job;
        let t0 = Instant::now();
        let mut hover_text: Option<String> = None;
        let mut fresh_selected_text: Option<String> = None;
        let held = img.is_some();
        let (image, focus) = match img {
            Some(i) => (i, focus),
            None => {
                // 保持画像なし: 初回と同じ基準 (UIA検出領域優先) で再キャプチャする。
                // 対象が変わっている可能性があるため、この場合のみ新しいcaptureとして扱う。
                let probe = uia::probe_at_point(x, y);
                match capture_plan::capture_probe(x, y, target, &probe) {
                    Ok((b, kind)) => {
                        let f = capture_plan::focus_for(kind, b.focus_y);
                        hover_text = probe.hover_text;
                        fresh_selected_text = probe.selected_text;
                        (Arc::new(b.img), f)
                    }
                    Err(e) => {
                        post(main, generation, WorkerMsg::Error { msg: e, anchor });
                        return;
                    }
                }
            }
        };
        // 保持画像がある場合は、キャプチャ時点のUIAコンテキストをそのまま使う。この時点で
        // 再プローブするとカーソルがオーバーレイ上(チップ押下位置)にあるため、無関係な
        // 要素のUIA情報を拾ってしまう (SPECv0.4.8追補: {{uia_path}} 誤り修正)。
        // 保持画像が無い(=対象が変わりうる)場合のみ従来通り再取得する。
        let ctx = if held {
            held_ctx
        } else {
            let mut c = AppContext::capture(x, y, HWND(target as *mut _));
            c.selected_text = fresh_selected_text;
            c
        };
        let pc = prompt_ctx(&ctx, &ocr_engine);
        // ログにはOCR対象領域だけを保存する (Focus::All なら全体のまま)
        let log_img = ocr::crop_for_focus(&image, focus);
        let hash = crate::capture::hash_hex(&log_img);

        // 同一画像+同一エンジンの既存認識結果があれば再利用する (再OCRなし・ログ追記なし)
        // 保持画像があれば既存captureへ追記、無ければ(=対象が変わりうる)新規captureを作る。
        // ここでは画像ありのcapture_idを先に決める(エラー時は画像なしで作り直す)。
        let capture_with_img = || if held { capture_id } else { log_cap(&cfg, "chip", &ctx, Some(&log_img)) };

        // 同一画像+同一エンジンの既存認識結果があれば再OCRせず再利用する。
        // 操作の記録としてrecognition行は追記する (SPECv0.4.9追補: 切替操作をログに残す)
        if let Some((cached_rid, text)) = crate::logdb::find_cached_recognition(&hash, &ocr_engine) {
            let ms = t0.elapsed().as_millis();
            util::perf_log(cfg.perf_log, &format!("{perf_label} {ocr_engine} {ms}ms (cached)"));
            let use_capture_id = capture_with_img();
            let rid = log_recog(&cfg, use_capture_id, "ocr", &ocr_engine, ms, Some(&text), None, Some(&hash))
                .or(Some(cached_rid));
            let pin = force_pin || text.contains('\n');
            post(main, generation, ctx.source_msg(
                text.clone(), "OCR", Some(ocr_engine.clone()), Some(image), pin, anchor, focus, ms,
                use_capture_id, rid,
            ));
            let o = ocr::OcrOutput { text, ..Default::default() };
            dispatch_translation(generation, cfg, o, tr_engine, main, rid, pc, &t0);
            return;
        }

        let mut result = ocr::run(&ocr_engine, &cfg, &image, focus, &pc);
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
                util::perf_log(cfg.perf_log, &format!("{perf_label} {ocr_engine} {ms}ms"));
                let use_capture_id = capture_with_img();
                let recog_id = log_recog(&cfg, use_capture_id, "ocr", &ocr_engine, ms, Some(&o.text), None, Some(&hash));
                let pin = force_pin || o.text.contains('\n');
                post(main, generation, ctx.source_msg(
                    o.text.clone(), "OCR", Some(ocr_engine.clone()), Some(image), pin, anchor, focus,
                    ms, use_capture_id, recog_id,
                ));
                dispatch_translation(generation, cfg, o, tr_engine, main, recog_id, pc, &t0);
            }
            Err(e) => {
                let use_capture_id = if held { capture_id } else { log_cap(&cfg, "chip", &ctx, None) };
                log_recog(&cfg, use_capture_id, "ocr", &ocr_engine, t0.elapsed().as_millis(), None, Some(&e), Some(&hash));
                post(main, generation, WorkerMsg::Error { msg: e, anchor });
            }
        }
    });
}

/// 翻訳チップ切替: 現在の原文を選択エンジンで再翻訳 (SPEC §8)。
pub fn retranslate(
    generation: u64,
    engine: String,
    cfg: Config,
    text: String,
    main: isize,
    recog_id: Option<i64>,
    pc: crate::config::PromptContext,
) {
    // 明示的な再翻訳操作のため元言語チェックはスキップし常に翻訳する (SPECv0.4.8追補)
    translate(generation, cfg, text, engine, main, recog_id, pc, true);
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
            });
            return;
        }
        let full = match crate::capture::capture_window(root) {
            Ok(f) => f,
            Err(e) => {
                post(main, generation, WorkerMsg::Error { msg: e, anchor });
                return;
            }
        };
        let r = crate::capture::window_frame_rect(root);
        let rw = (r.right - r.left).max(1);
        let rh = (r.bottom - r.top).max(1);
        let sx = full.width as f32 / rw as f32;
        let sy = full.height as f32 / rh as f32;
        let crop = crate::capture::crop(
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
        // Focus::All → 全行を段落結合
        let ctx = AppContext::capture(cx, cy, root);
        let pc = prompt_ctx(&ctx, &engine);
        match ocr::run(&engine, &cfg, &img, ocr::Focus::All, &pc) {
            Ok(o) => {
                let ms = t0.elapsed().as_millis();
                util::perf_log(cfg.perf_log, &format!("region OCR({engine}) {ms}ms"));
                let hash = crate::capture::hash_hex(&img);
                let capture_id = log_cap(&cfg, "region", &ctx, Some(&img));
                let recog_id = log_recog(&cfg, capture_id, "ocr", &engine, ms, Some(&o.text), None, Some(&hash));
                post(main, generation, ctx.source_msg(
                    o.text.clone(), "OCR", Some(engine.clone()), Some(Arc::new(img)), true, anchor,
                    ocr::Focus::All, ms, capture_id, recog_id,
                ));
                dispatch_translation(generation, cfg, o, tr_engine, main, recog_id, pc, &t0);
            }
            Err(e) => {
                let capture_id = log_cap(&cfg, "region", &ctx, None);
                log_recog(&cfg, capture_id, "ocr", &engine, t0.elapsed().as_millis(), None, Some(&e), None);
                post(main, generation, WorkerMsg::Error { msg: e, anchor });
            }
        }
    });
}

/// 解説プロンプトを組み立てる (LLMプロファイル未設定時は None)。
/// アプリ名・UIAパス等のコンテキストはテンプレートのプレースホルダで埋め込む (SPECv0.4 §7.2)。
pub fn build_explain_prompt(cfg: &Config, ctx: &crate::config::PromptContext) -> Option<String> {
    let prof = cfg.active_profile()?;
    Some(cfg.fill_prompt(&prof.explain_prompt, ctx))
}

/// 解説の取得 (SPEC v0.3 §2.2.2 / v0.4 §8.2.4): 成功・失敗ともログへ追記してオーバーレイへ通知する。
/// profile はダイアログで選択されたAPIプロファイル名 (見つからなければアクティブを使用)。
pub fn explain(generation: u64, recog_id: i64, cfg: Config, prompt: String, profile: String, main: isize) {
    std::thread::spawn(move || {
        init_com();
        // 同一送信プロンプト(input_text)の成功済み解説がDBにあれば、APIを呼ばずそれを使う
        // (SPECv0.4.8追補: 別の親であっても検索対象。別の親なら同内容を新規に記帳する)。
        if cfg.log_enabled
            && let Some((cached_rid, cached_profile, cached_text)) = crate::logdb::find_cached_explanation(&prompt)
        {
            if cached_rid != recog_id {
                crate::logdb::log_explanation(
                    recog_id, &cached_profile, 0, &prompt, Some(&cached_text), None, Some(0), Some(0),
                );
            }
            post(main, generation, WorkerMsg::Explanation { text: cached_text });
            return;
        }
        let t0 = Instant::now();
        let Some(prof) = cfg
            .api_profiles
            .iter()
            .find(|p| p.name == profile)
            .or_else(|| cfg.active_profile())
        else {
            post(main, generation, WorkerMsg::Error {
                msg: "解説の取得に失敗しました: LLM APIプロファイルが設定されていません".into(),
                anchor: (0, 0),
            });
            return;
        };
        let result = crate::llm_api::call(prof, &crate::llm_api::LlmRequest::text(&prompt));
        let ms = t0.elapsed().as_millis();
        if cfg.log_enabled {
            match &result {
                Ok(res) => crate::logdb::log_explanation(
                    recog_id, &prof.name, ms, &prompt, Some(&res.text), None,
                    res.tokens_in, res.tokens_out,
                ),
                Err(e) => crate::logdb::log_explanation(
                    recog_id, &prof.name, ms, &prompt, None, Some(e), None, None,
                ),
            }
        }
        match result {
            Ok(res) => {
                post(main, generation, WorkerMsg::Explanation { text: res.text });
            }
            Err(e) => {
                post(main, generation, WorkerMsg::Error {
                    msg: format!("解説の取得に失敗しました: {e}"),
                    anchor: (0, 0),
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
