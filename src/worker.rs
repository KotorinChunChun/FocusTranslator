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
    /// 対象アプリ全体のキャプチャ画像 (画像編集の「全体画像の復元」用。SPECv0.5.2追補)
    pub full_img: Option<Arc<Captured>>,
    /// full_img 内での img の位置 (物理ピクセル座標)
    pub crop_rect: Option<RECT>,
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
        /// true: 既存の原文表示を消してからエラーを出す (OCRエンジン切替・画像編集後の再認識失敗)。
        /// false: 原文はそのまま残しステータス行にのみエラーを出す (解説失敗など)。
        clear_source: bool,
    },
    Explanation {
        text: String,
        /// 実際に解説を生成したLLMプロファイル名 (SPECv0.5.2追補: プロファイル別チップ用)
        profile: String,
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
    /// uia_nodes をJSON化したもの (SPECv0.5.2追補: ControlType/AutomationId/Name/矩形等を
    /// 落とさずログDBへ記録するため)
    pub uia_json: String,
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
            uia_json: uia::nodes_to_json(&uia_nodes),
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
        full_img: Option<Arc<Captured>>,
        crop_rect: Option<RECT>,
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
            full_img,
            crop_rect,
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

/// log_cap に渡すOCR抽出範囲情報 (全体画像・全体内での位置・行選択基準)。
/// 値が無い(全体画像を持たない/失敗時など)場合は Extent::none() を渡す (SPECv0.5.2追補)。
#[derive(Default, Clone, Copy)]
struct Extent<'a> {
    full: Option<&'a Captured>,
    rect: Option<RECT>,
    focus_kind: Option<&'static str>,
    focus_y: Option<f32>,
}

impl<'a> Extent<'a> {
    fn none() -> Self {
        Extent::default()
    }
}

/// 入力(キャプチャ)ログを記録し capture_id を返す(ログOFF時は None)。
/// 画像はデバッグモード時のみPNG保存される。ローテーションもここで行う。
fn log_cap(cfg: &Config, mode: &str, ctx: &AppContext, image: Option<&Captured>, extent: Extent) -> Option<i64> {
    if !cfg.log_enabled {
        return None;
    }
    let crop_rect = extent.rect.map(|r| (r.left, r.top, r.right - r.left, r.bottom - r.top));
    let id = crate::logdb::log_capture(
        mode, ctx.exe.as_deref(), Some(&ctx.title), Some(&ctx.uia_path), Some(&ctx.uia_json), ctx.control_type.as_deref(),
        image, cfg.debug_mode,
        crate::logdb::CaptureExtent {
            full_image: extent.full,
            crop_rect,
            focus_kind: extent.focus_kind,
            focus_y: extent.focus_y,
        },
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

/// 既定OCRエンジンが未導入等で使えない場合のみローカルへ置き換える。
/// 同意の有無によるフォールバックは行わない (SPECv0.5.3: 同意が無ければ
/// ensure_ocr_consent がその場で同意を求め、拒否されたら実行自体を中断する)。
fn effective_ocr(cfg: &Config) -> String {
    let e = cfg.default_ocr.as_str();
    if cfg.engine_available(e) {
        e.to_string()
    } else if cfg.engine_available("oneocr") {
        "oneocr".to_string()
    } else {
        "win".to_string()
    }
}

/// 自動サイクルの同意ダイアログをこのセッションで拒否済みか。
/// ホールドキーを押すたびにダイアログが連発しないよう、拒否は起動中のみ記憶する
/// (チップ等の明示操作による同意確認には影響しない)。
static AUTO_CONSENT_DECLINED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// ワーカースレッドから外部送信の同意を確認する (SPECv0.5.3)。
/// 最新の設定を読み直して判定し、未同意なら MessageBox で確認する。同意されたら設定へ
/// 永続化し、メインスレッドへ WM_APP_CFG で再読込を通知する。拒否されたらセッション中は
/// 再度尋ねず false を返す。
fn request_consent_blocking(main: isize, want_image: bool) -> bool {
    use std::sync::atomic::Ordering;
    use windows::Win32::UI::WindowsAndMessaging::{
        IDYES, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, MessageBoxW,
    };
    let fresh = Config::load();
    let need_text = !fresh.consent_text;
    let need_image = want_image && !fresh.consent_image;
    if !need_text && !need_image {
        return true;
    }
    if AUTO_CONSENT_DECLINED.load(Ordering::Relaxed) {
        return false;
    }
    let msg = if need_image {
        "既定のエンジンはキャプチャ画像とテキスト・言語設定を外部サービスへ送信します。\n初回のみ確認しています。許可しますか?\n(拒否した場合、既定エンジンを変更するまでこの機能は実行されません)"
    } else {
        "既定のエンジンは読み取ったテキストと関連情報(アプリ名・ウィンドウタイトル等)を外部サービスへ送信します。\n初回のみ確認しています。許可しますか?\n(拒否した場合、既定エンジンを変更するまでこの機能は実行されません)"
    };
    let msg_w = util::to_wide(msg);
    let r = unsafe {
        MessageBoxW(
            None,
            windows::core::PCWSTR(msg_w.as_ptr()),
            windows::core::PCWSTR(util::to_wide("外部送信の同意").as_ptr()),
            MB_YESNO | MB_SETFOREGROUND | MB_TOPMOST,
        )
    };
    if r == IDYES {
        let mut c = Config::load();
        if need_text {
            c.consent_text = true;
        }
        if need_image {
            c.consent_image = true;
            // 画像には文字が写るため、画像同意はテキスト同意を内包する
            c.consent_text = true;
        }
        c.save();
        unsafe {
            let _ = PostMessageW(
                Some(HWND(main as *mut _)),
                crate::app_state::WM_APP_CFG,
                WPARAM(0),
                LPARAM(0),
            );
        }
        true
    } else {
        AUTO_CONSENT_DECLINED.store(true, Ordering::Relaxed);
        false
    }
}

/// 自動サイクルでOCRエンジンが外部送信を要する場合の同意確認 (SPECv0.5.3)。
/// LLM統合OCRかつアクティブプロファイルが外部URLのときのみ確認する。拒否なら Err。
fn ensure_ocr_consent(cfg: &Config, engine: &str, main: isize) -> Result<(), String> {
    if engine != "llm" {
        return Ok(());
    }
    if !cfg.active_profile().is_some_and(|p| p.is_external()) {
        return Ok(());
    }
    if request_consent_blocking(main, true) {
        Ok(())
    } else {
        Err("外部送信の同意が得られないため、LLMによる画面読み取りを実行できません。設定画面で既定OCRエンジンを変更するか、同意を許可してください。".into())
    }
}

/// 翻訳エンジンが外部送信を要する場合の同意確認 (SPECv0.5.3)。拒否なら Err。
/// 自動・明示を問わず翻訳実行の直前に呼ぶ (明示操作はチップ側で同意済みのため、
/// ここでは追加のダイアログは出ない)。
fn ensure_tr_consent(cfg: &Config, engine: &str, main: isize) -> Result<(), String> {
    let external = match engine {
        "deepl" | "google" => true,
        "llm" => cfg.active_profile().is_some_and(|p| p.is_external()),
        _ => false,
    };
    if !external {
        return Ok(());
    }
    if request_consent_blocking(main, false) {
        Ok(())
    } else {
        Err("外部送信の同意が得られないため翻訳をスキップしました。設定画面で既定翻訳エンジンを変更するか、同意を許可してください。".into())
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

/// 帯画像(img)を focus に応じてログ保存用に切り出し、全体画像(band_rect の元)内での
/// 位置も算出する (SPECv0.5.2追補: OCR抽出範囲の記録)。
/// 戻り値: (保存用画像, 全体画像内での矩形)
fn crop_for_log(band_rect: RECT, img: &Captured, focus: ocr::Focus) -> (Captured, RECT) {
    let (cropped, sub) = ocr::crop_for_focus_rect(img, focus);
    let full_rect = RECT {
        left: band_rect.left + sub.left,
        top: band_rect.top + sub.top,
        right: band_rect.left + sub.right,
        bottom: band_rect.top + sub.bottom,
    };
    (cropped.into_owned(), full_rect)
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
    let mut log_img: Option<Captured> = None;
    let mut extent_rect: Option<RECT> = None;
    let mut focus_y_full: Option<f32> = None;
    if let Some((b, _)) = &cap {
        let (cropped, full_rect) = crop_for_log(b.rect, &b.img, focus);
        log_img = Some(cropped);
        extent_rect = Some(full_rect);
        focus_y_full = focus.y().map(|fy| b.rect.top as f32 + fy);
    }
    let (img, full_img) = match cap {
        Some((b, _)) => (Some(Arc::new(b.img)), Some(Arc::new(b.full))),
        None => (None, None),
    };

    let capture_id = log_cap(&cfg, "hold", ctx, log_img.as_ref(), Extent {
        full: full_img.as_deref(),
        rect: extent_rect,
        focus_kind: Some(focus.kind_str()),
        focus_y: focus_y_full,
    });
    let hash = log_img.as_ref().map(crate::capture::hash_hex);
    let recog_id = log_recog(&cfg, capture_id, "uia", "uia", ms, Some(&text), None, hash.as_deref());
    post(main, generation, ctx.source_msg(
        text.clone(), "UIA", None, img, false, (x, y), focus, ms, capture_id, recog_id, full_img, extent_rect,
    ));
    // UIA経路なので ocr_engine は空文字 (SPECv0.4 §7.1)
    let pc = prompt_ctx(ctx, "");
    translate(generation, cfg, text, tr_engine, main, recog_id, pc, false);
}

/// ホールドモードの認識サイクル: 選択文字列 → UIA → WGCキャプチャOCR (SPEC §6.4, SPECv0.5追補)
/// キャプチャ領域は UIA検出結果 (行矩形/要素/直下要素) を優先し、無ければ既定帯。
pub fn recognize_cycle(generation: u64, x: i32, y: i32, target: isize, cfg: Config, main: isize) {
    std::thread::spawn(move || {
        let tr_engine = cfg.default_translator.clone();
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
        // 外部LLMへの画像送信となる場合は同意を確認し、拒否なら実行しない (SPECv0.5.3)
        if let Err(e) = ensure_ocr_consent(&cfg, &engine, main) {
            post(main, generation, WorkerMsg::Error { msg: e, anchor: (x, y), clear_source: false });
            return;
        }
        let cap = match capture_plan::capture_probe(x, y, target, &probe) {
            Ok(c) => c,
            Err(e) => {
                let capture_id = log_cap(&cfg, "hold", &ctx, None, Extent::none());
                log_recog(&cfg, capture_id, "ocr", &engine, t0.elapsed().as_millis(), None, Some(&e), None);
                post(main, generation, WorkerMsg::Error { msg: e, anchor: (x, y), clear_source: false });
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
                let (log_img, full_rect) = crop_for_log(used.rect, &used.img, focus);
                let hash = crate::capture::hash_hex(&log_img);
                let capture_id = log_cap(&cfg, "hold", &ctx, Some(&log_img), Extent {
                    full: Some(&used.full),
                    rect: Some(full_rect),
                    focus_kind: Some(focus.kind_str()),
                    focus_y: focus.y().map(|fy| used.rect.top as f32 + fy),
                });
                let recog_id = log_recog(&cfg, capture_id, "ocr", &engine, ms, Some(&o.text), None, Some(&hash));
                let pin = false;
                let full_img = Arc::new(used.full);
                post(main, generation, ctx.source_msg(
                    o.text.clone(), "OCR", Some(engine.clone()), Some(Arc::new(used.img)), pin,
                    (x, y), focus, ms, capture_id, recog_id, Some(full_img), Some(full_rect),
                ));
                dispatch_translation(generation, cfg, o, tr_engine, main, recog_id, pc, &t0);
            }
            Err(e) => {
                let ms = t0.elapsed().as_millis();
                let capture_id = log_cap(&cfg, "hold", &ctx, None, Extent::none());
                let recog_id = log_recog(&cfg, capture_id, "ocr", &engine, ms, None, Some(&e), None);
                // OCRエンジン切替チップで再試行できるよう、キャプチャした画像はエラーでも
                // 保持画像として送っておく (原文は空欄のまま; SPECv0.5.2追補)。
                post(main, generation, ctx.source_msg(
                    String::new(), "OCR", Some(engine.clone()), Some(Arc::new(used.img)), false,
                    (x, y), focus, ms, capture_id, recog_id, Some(Arc::new(used.full)), None,
                ));
                post(main, generation, WorkerMsg::Error { msg: e, anchor: (x, y), clear_source: false });
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
        // 外部送信を要するエンジンは実行直前に同意を確認する (SPECv0.5.3:
        // 同意なしのローカルへの無言フォールバックは廃止。拒否時はスキップ表示)。
        if let Err(msg) = ensure_tr_consent(&cfg, &engine, main) {
            post(main, generation, WorkerMsg::TranslationSkipped { msg });
            return;
        }
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
    /// 保持画像(img=Some)のときに使う、対象アプリ全体のキャプチャ画像 (SPECv0.5.2追補)。
    /// 再キャプチャ時 (img=None) は無視され、新しく撮影したものに置き換わる。
    pub held_full_img: Option<Arc<Captured>>,
    /// held_full_img 内での img の位置。画像編集で確定した場合は呼び出し側が新しい矩形を
    /// 渡す (あらかじめ DB へも反映しておくこと)。
    pub held_crop_rect: Option<RECT>,
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
            anchor, ctx: held_ctx, force_pin, perf_label, held_full_img, held_crop_rect,
        } = job;
        let t0 = Instant::now();
        let mut hover_text: Option<String> = None;
        let mut fresh_selected_text: Option<String> = None;
        let held = img.is_some();
        // full_img/band_rect: 全体画像とその内での image の位置 (無ければ全体画像機能は使えない)
        let (image, focus, full_img, band_rect) = match img {
            Some(i) => (i, focus, held_full_img, held_crop_rect),
            None => {
                // 保持画像なし: 初回と同じ基準 (UIA検出領域優先) で再キャプチャする。
                // 対象が変わっている可能性があるため、この場合のみ新しいcaptureとして扱う。
                let probe = uia::probe_at_point(x, y);
                match capture_plan::capture_probe(x, y, target, &probe) {
                    Ok((b, kind)) => {
                        let f = capture_plan::focus_for(kind, b.focus_y);
                        hover_text = probe.hover_text;
                        fresh_selected_text = probe.selected_text;
                        (Arc::new(b.img), f, Some(Arc::new(b.full)), Some(b.rect))
                    }
                    Err(e) => {
                        post(main, generation, WorkerMsg::Error { msg: e, anchor, clear_source: true });
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
        // ログにはOCR対象領域だけを保存する (Focus::All なら全体のまま)。band_rect が分かって
        // いれば、全体画像内での位置も合わせて算出する (SPECv0.5.2追補)。
        let (log_img, full_rect) = match band_rect {
            Some(r) => {
                let (li, fr) = crop_for_log(r, &image, focus);
                (li, Some(fr))
            }
            None => (ocr::crop_for_focus(&image, focus).into_owned(), None),
        };
        let hash = crate::capture::hash_hex(&log_img);
        let extent = Extent {
            full: full_img.as_deref(),
            rect: full_rect,
            focus_kind: Some(focus.kind_str()),
            focus_y: band_rect.zip(focus.y()).map(|(r, fy)| r.top as f32 + fy),
        };

        // 同一画像+同一エンジンの既存認識結果があれば再利用する (再OCRなし・ログ追記なし)
        // 保持画像があれば既存captureへ追記、無ければ(=対象が変わりうる)新規captureを作る。
        // ここでは画像ありのcapture_idを先に決める(エラー時は画像なしで作り直す)。
        let capture_with_img = || if held { capture_id } else { log_cap(&cfg, "chip", &ctx, Some(&log_img), extent) };

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
                use_capture_id, rid, full_img.clone(), full_rect,
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
                    ms, use_capture_id, recog_id, full_img, full_rect,
                ));
                dispatch_translation(generation, cfg, o, tr_engine, main, recog_id, pc, &t0);
            }
            Err(e) => {
                let use_capture_id = if held { capture_id } else { log_cap(&cfg, "chip", &ctx, None, Extent::none()) };
                log_recog(&cfg, use_capture_id, "ocr", &ocr_engine, t0.elapsed().as_millis(), None, Some(&e), Some(&hash));
                post(main, generation, WorkerMsg::Error { msg: e, anchor, clear_source: true });
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
        let tr_engine = cfg.default_translator.clone();
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
                clear_source: false,
            });
            return;
        }
        let full = match crate::capture::capture_window(root) {
            Ok(f) => f,
            Err(e) => {
                post(main, generation, WorkerMsg::Error { msg: e, anchor, clear_source: false });
                return;
            }
        };
        let r = crate::capture::window_frame_rect(root);
        let rw = (r.right - r.left).max(1);
        let rh = (r.bottom - r.top).max(1);
        let sx = full.width as f32 / rw as f32;
        let sy = full.height as f32 / rh as f32;
        let crop = crate::capture::crop_with_rect(
            &full,
            (((rect.left - r.left) as f32) * sx) as i32,
            (((rect.top - r.top) as f32) * sy) as i32,
            (((rect.right - rect.left) as f32) * sx) as i32,
            (((rect.bottom - rect.top) as f32) * sy) as i32,
        );
        let Some((img, img_rect)) = crop else {
            post(main, generation, WorkerMsg::Error {
                msg: "選択範囲を切り出せませんでした".into(),
                anchor,
                clear_source: false,
            });
            return;
        };

        let engine = effective_ocr(&cfg);
        // 外部LLMへの画像送信となる場合は同意を確認し、拒否なら実行しない (SPECv0.5.3)
        if let Err(e) = ensure_ocr_consent(&cfg, &engine, main) {
            post(main, generation, WorkerMsg::Error { msg: e, anchor, clear_source: false });
            return;
        }
        // Focus::All → 全行を段落結合
        let ctx = AppContext::capture(cx, cy, root);
        let pc = prompt_ctx(&ctx, &engine);
        match ocr::run(&engine, &cfg, &img, ocr::Focus::All, &pc) {
            Ok(o) => {
                let ms = t0.elapsed().as_millis();
                util::perf_log(cfg.perf_log, &format!("region OCR({engine}) {ms}ms"));
                let hash = crate::capture::hash_hex(&img);
                let capture_id = log_cap(&cfg, "region", &ctx, Some(&img), Extent {
                    full: Some(&full),
                    rect: Some(img_rect),
                    focus_kind: Some(ocr::Focus::All.kind_str()),
                    focus_y: None,
                });
                let recog_id = log_recog(&cfg, capture_id, "ocr", &engine, ms, Some(&o.text), None, Some(&hash));
                let full_img = Arc::new(full);
                post(main, generation, ctx.source_msg(
                    o.text.clone(), "OCR", Some(engine.clone()), Some(Arc::new(img)), true, anchor,
                    ocr::Focus::All, ms, capture_id, recog_id, Some(full_img), Some(img_rect),
                ));
                dispatch_translation(generation, cfg, o, tr_engine, main, recog_id, pc, &t0);
            }
            Err(e) => {
                let ms = t0.elapsed().as_millis();
                let capture_id = log_cap(&cfg, "region", &ctx, None, Extent::none());
                let recog_id = log_recog(&cfg, capture_id, "ocr", &engine, ms, None, Some(&e), None);
                // OCRエンジン切替チップで再試行できるよう、キャプチャした画像はエラーでも
                // 保持画像として送っておく (原文は空欄のまま; SPECv0.5.2追補)。
                let full_img = Arc::new(full);
                post(main, generation, ctx.source_msg(
                    String::new(), "OCR", Some(engine.clone()), Some(Arc::new(img)), false, anchor,
                    ocr::Focus::All, ms, capture_id, recog_id, Some(full_img), Some(img_rect),
                ));
                post(main, generation, WorkerMsg::Error { msg: e, anchor, clear_source: false });
            }
        }
    });
}

/// 指定プロファイル名の解説プロンプトを組み立てる (SPECv0.5.2追補: 解説チップの
/// プロファイル別ボタン用。指定プロファイルが見つからなければ None)。
pub fn build_explain_prompt_for(cfg: &Config, profile: &str, ctx: &crate::config::PromptContext) -> Option<String> {
    let prof = cfg.api_profiles.iter().find(|p| p.name == profile)?;
    Some(cfg.fill_prompt(&prof.explain_prompt, ctx))
}

/// 解説に添付するアプリ全体キャプチャ (SPECv0.5.3)。
/// rect は full 内での解説対象範囲 (物理ピクセル座標)。赤枠を描いてから送信する。
pub struct ExplainImage {
    pub full: Arc<Captured>,
    pub rect: Option<RECT>,
}

/// 解説送信用画像の長辺上限 (px)。全体キャプチャは大きくなりがちなため縮小して送る。
const EXPLAIN_IMAGE_MAX_DIM: u32 = 1600;

/// 解説用の送信画像を組み立てる: 縮小 → 対象範囲へ赤枠描画 → PNG。
/// 戻り値: (PNGバイト列, 画像ハッシュ)。
fn build_explain_image(img: &ExplainImage) -> (Vec<u8>, String) {
    let full = &*img.full;
    let scale_src = full.width.max(full.height).max(1);
    let mut prepared = crate::capture::downscale_max(full, EXPLAIN_IMAGE_MAX_DIM);
    if let Some(r) = img.rect {
        // 縮小後の座標系へ矩形を変換してから赤枠を描く (先に描くと縮小で枠が痩せるため)
        let s = prepared.width.max(prepared.height) as f32 / scale_src as f32;
        let scaled = RECT {
            left: (r.left as f32 * s) as i32,
            top: (r.top as f32 * s) as i32,
            right: (r.right as f32 * s) as i32,
            bottom: (r.bottom as f32 * s) as i32,
        };
        crate::capture::draw_red_frame(&mut prepared, scaled, 3);
    }
    let hash = crate::capture::hash_hex(&prepared);
    (crate::capture::to_png(&prepared), hash)
}

/// 解説の取得 (SPEC v0.3 §2.2.2 / v0.4 §8.2.4): 成功・失敗ともログへ追記してオーバーレイへ通知する。
/// profile はダイアログで選択されたAPIプロファイル名 (見つからなければアクティブを使用)。
/// image が Some なら、赤枠でマークした全体キャプチャを添付して送信する (SPECv0.5.3)。
pub fn explain(
    generation: u64,
    recog_id: i64,
    cfg: Config,
    prompt: String,
    profile: String,
    main: isize,
    image: Option<ExplainImage>,
) {
    std::thread::spawn(move || {
        init_com();
        // 画像添付時は送信画像を先に確定し、キャッシュキー(input_text)へ画像ハッシュを
        // 含める (SPECv0.5.3: 同一プロンプトでも別画面のキャッシュを誤って流用しないため)。
        let prepared = image.as_ref().map(build_explain_image);
        let cache_text = match &prepared {
            Some((_, hash)) => format!("{prompt}\n[添付画像hash: {}]", &hash[..16]),
            None => prompt.clone(),
        };
        // 同一プロファイル+同一送信プロンプト(input_text)の成功済み解説がDBにあれば、
        // APIを呼ばずそれを使う (SPECv0.5.2追補: プロファイル別チップのため、テンプレートが
        // 同一で input_text が一致していても別プロファイルのキャッシュは流用しない)。
        if cfg.log_enabled
            && let Some((cached_rid, cached_text)) = crate::logdb::find_cached_explanation_for_profile(&profile, &cache_text)
        {
            if cached_rid != recog_id {
                crate::logdb::log_explanation(
                    recog_id, &profile, 0, &cache_text, Some(&cached_text), None, Some(0), Some(0),
                );
            }
            post(main, generation, WorkerMsg::Explanation { text: cached_text, profile });
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
                clear_source: false,
            });
            return;
        };
        let png_b64 = prepared.as_ref().map(|(png, _)| {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(png)
        });
        let req = crate::llm_api::LlmRequest {
            prompt: &prompt,
            image_png_b64: png_b64.as_deref(),
            json_mode: false,
        };
        let result = crate::llm_api::call(prof, &req)
            // エラーメッセージにAPIキーが混入しても表示・ログへ漏れないよう伏字化する (SPECv0.5.3)
            .map_err(|e| crate::translate::mask_keys(&cfg, &e));
        let ms = t0.elapsed().as_millis();
        if cfg.log_enabled {
            match &result {
                Ok(res) => crate::logdb::log_explanation(
                    recog_id, &prof.name, ms, &cache_text, Some(&res.text), None,
                    res.tokens_in, res.tokens_out,
                ),
                Err(e) => crate::logdb::log_explanation(
                    recog_id, &prof.name, ms, &cache_text, None, Some(e), None, None,
                ),
            }
        }
        match result {
            Ok(res) => {
                post(main, generation, WorkerMsg::Explanation { text: res.text, profile: prof.name.clone() });
            }
            Err(e) => {
                post(main, generation, WorkerMsg::Error {
                    msg: format!("解説の取得に失敗しました: {e}"),
                    anchor: (0, 0),
                    clear_source: false,
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
