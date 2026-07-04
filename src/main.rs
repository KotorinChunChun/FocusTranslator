// Focus Translator v0.1 — カーソル位置翻訳ツール (FocusTranslator_SPECv0.1.md 準拠)
// 右Ctrlホールド中のみ、マウスポインタ直下のテキスト1行を認識・翻訳して
// カーソル近傍にオーバーレイ表示するタスクトレイ常駐ツール。
#![windows_subsystem = "windows"]

mod capture;
mod config;
mod logdb;
mod logviewer;
mod ocr;
mod onnx_translate;
mod onnx_translate_install;
mod overlay;
mod paddle_install;
mod region;
mod settings;
mod translate;
mod tray;
mod uia;
mod util;
mod worker;

use config::Config;
use overlay::OverlayContent;
use std::cell::RefCell;
use std::sync::Arc;
use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, HOT_KEY_MODIFIERS, RegisterHotKey, UnregisterHotKey, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW, GA_ROOT, GetAncestor,
    GetCursorPos, GetMessageW, HICON, IsDialogMessageW, KillTimer, LoadIconW, MB_ICONWARNING,
    MB_OK, MSG, MessageBoxW, PostQuitMessage, RegisterClassW, SetTimer, TranslateMessage, WM_APP,
    WM_COMMAND, WM_DESTROY, WM_HOTKEY, WM_LBUTTONUP, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
    WindowFromPoint, WS_OVERLAPPED,
};
use windows::core::{PCWSTR, w};

/// 埋め込みリソース(build.rs で ID "1" として同梱)からアプリアイコンを取得する。
/// exeアイコン・通知領域アイコン・各ウィンドウのタイトルバーアイコンで共用する。
#[allow(clippy::manual_dangling_ptr)] // MAKEINTRESOURCE(1) 相当。実在するリソースIDを指すため意図的
pub fn app_icon() -> HICON {
    unsafe {
        let inst = HINSTANCE(GetModuleHandleW(None).map(|m| m.0).unwrap_or(std::ptr::null_mut()));
        LoadIconW(Some(inst), PCWSTR(1usize as *const u16)).unwrap_or_default()
    }
}

// アプリ内メッセージ
pub const WM_APP_TRAY: u32 = WM_APP + 1;
pub const WM_APP_WORKER: u32 = WM_APP + 2;
pub const WM_APP_CHIP: u32 = WM_APP + 3;
pub const WM_APP_REGION: u32 = WM_APP + 4;
pub const WM_APP_CFG: u32 = WM_APP + 5;

const TIMER_POLL: usize = 1;
const HOTKEY_REGION: i32 = 1;

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Idle,
    Recognizing,
    ShowingHold,
    Pinned,
}

struct App {
    cfg: Config,
    instance: HINSTANCE,
    main: HWND,
    overlay: HWND,
    generation: u64,
    hold: bool,
    mode: Mode,
    /// サイクル開始時のカーソル位置(行逸脱監視用)
    origin: POINT,
    /// サイクル開始時の対象ウィンドウ(再OCR用)
    target: isize,
    source: String,
    translation: Option<String>,
    status: Option<String>,
    badge: Option<String>,
    cur_ocr: String,
    cur_tr: String,
    last_img: Option<Arc<capture::Captured>>,
    anchor: (i32, i32),
    error_only: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static MAIN_HWND: RefCell<isize> = const { RefCell::new(0) };
}

pub fn main_hwnd() -> HWND {
    HWND(MAIN_HWND.with(|h| *h.borrow()) as *mut _)
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| {
        let Ok(mut guard) = a.try_borrow_mut() else {
            return None;
        };
        guard.as_mut().map(f)
    })
}

fn main() {
    // 多重起動防止
    unsafe {
        let _mutex = CreateMutexW(None, false, w!("FocusTranslator_Singleton"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    std::panic::set_hook(Box::new(|info| {
        util::app_log(&format!("panic: {info}"));
    }));

    let instance: HINSTANCE = unsafe {
        HINSTANCE(GetModuleHandleW(None).map(|m| m.0).unwrap_or(std::ptr::null_mut()))
    };
    let cfg = Config::load();

    // メイン(非表示)ウィンドウ
    let main = unsafe {
        let class = w!("FocusTranslatorMain");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hIcon: app_icon(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        CreateWindowExW(
            Default::default(),
            class,
            w!("FocusTranslator"),
            WS_OVERLAPPED,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
        .expect("メインウィンドウの作成に失敗")
    };
    MAIN_HWND.with(|h| *h.borrow_mut() = main.0 as isize);

    let overlay = overlay::create(instance);
    tray::add_icon(main);

    // 範囲指定ホットキー (SPEC §11: 衝突時は通知)
    let (mods, vk) = cfg.region_hotkey_parsed();
    unsafe {
        if RegisterHotKey(Some(main), HOTKEY_REGION, HOT_KEY_MODIFIERS(mods), vk).is_err() {
            MessageBoxW(
                Some(main),
                w!("範囲指定ホットキーを登録できませんでした。他のアプリと衝突しています。設定画面で変更してください。"),
                w!("Focus Translator"),
                MB_OK | MB_ICONWARNING,
            );
        }
        SetTimer(Some(main), TIMER_POLL, cfg.poll_ms, None);
    }

    let cur_ocr = cfg.default_ocr.clone();
    let cur_tr = cfg.default_translator.clone();
    APP.with(|a| {
        *a.borrow_mut() = Some(App {
            cfg,
            instance,
            main,
            overlay,
            generation: 0,
            hold: false,
            mode: Mode::Idle,
            origin: POINT::default(),
            target: 0,
            source: String::new(),
            translation: None,
            status: None,
            badge: None,
            cur_ocr,
            cur_tr,
            last_img: None,
            anchor: (0, 0),
            error_only: false,
        });
    });

    // メッセージループ
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if settings::is_open() && IsDialogMessageW(settings::hwnd(), &msg).as_bool() {
                continue;
            }
            if logviewer::is_open() && IsDialogMessageW(logviewer::hwnd(), &msg).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if wparam.0 == TIMER_POLL => {
            tick();
            LRESULT(0)
        }
        WM_HOTKEY if wparam.0 as i32 == HOTKEY_REGION => {
            let inst = with_app(|app| app.instance).unwrap_or_default();
            region::start(inst, hwnd);
            LRESULT(0)
        }
        WM_APP_TRAY => {
            let ev = (lparam.0 & 0xFFFF) as u32;
            if ev == WM_RBUTTONUP || ev == WM_LBUTTONUP {
                let cmd = tray::show_menu(hwnd);
                handle_command(hwnd, cmd);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            handle_command(hwnd, wparam.0 & 0xFFFF);
            LRESULT(0)
        }
        WM_APP_WORKER => {
            handle_worker(wparam.0 as u64, lparam);
            LRESULT(0)
        }
        WM_APP_CHIP => {
            handle_chip(wparam.0);
            LRESULT(0)
        }
        WM_APP_REGION => {
            let rect = unsafe { *Box::from_raw(lparam.0 as *mut RECT) };
            handle_region(rect);
            LRESULT(0)
        }
        WM_APP_CFG => {
            reload_config(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            tray::remove_icon(hwnd);
            unsafe {
                let _ = UnregisterHotKey(Some(hwnd), HOTKEY_REGION);
                let _ = KillTimer(Some(hwnd), TIMER_POLL);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn handle_command(hwnd: HWND, cmd: usize) {
    match cmd {
        tray::CMD_SETTINGS => {
            let inst = with_app(|app| app.instance).unwrap_or_default();
            settings::open(inst, hwnd);
        }
        tray::CMD_REGION => {
            let inst = with_app(|app| app.instance).unwrap_or_default();
            region::start(inst, hwnd);
        }
        tray::CMD_LOGVIEWER => {
            let inst = with_app(|app| app.instance).unwrap_or_default();
            logviewer::open(inst);
        }
        tray::CMD_EXIT => unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
        },
        _ => {}
    }
}

/// 100ms周期のポーリング (SPEC §4): 右Ctrl状態遷移と行逸脱・Esc監視
fn tick() {
    let action = with_app(|app| {
        let down = unsafe { (GetAsyncKeyState(app.cfg.hold_vk()) as u16 & 0x8000) != 0 };
        let esc = unsafe { (GetAsyncKeyState(VK_ESCAPE.0 as i32) as u16 & 0x8000) != 0 };

        // ピン留め中のEscで閉じる (SPEC §2.1)
        if app.mode == Mode::Pinned && esc {
            close_overlay(app);
            app.hold = down;
            return None;
        }

        let prev = app.hold;
        app.hold = down;
        match (prev, down) {
            (false, true) => {
                // OFF→ON: ホールド開始 → 認識サイクル開始
                start_cycle_params(app)
            }
            (true, false) => {
                // ON→OFF: 未ピン留め表示を閉じる
                if app.mode != Mode::Pinned {
                    close_overlay(app);
                }
                None
            }
            (true, true) => {
                // ホールド継続中は、マウスが動いても再認識・表示位置の移動を行わない。
                // キーを離すまで表示はその場に固定する(ユーザー要望)。
                None
            }
            _ => None,
        }
    })
    .flatten();

    if let Some((generation, x, y, target, cfg, main)) = action {
        worker::recognize_cycle(generation, x, y, target, cfg, main);
    }
}

/// 認識サイクルの開始準備(世代番号更新・対象HWND解決)を行い、ワーカー引数を返す
fn start_cycle_params(app: &mut App) -> Option<(u64, i32, i32, isize, Config, isize)> {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    // ポインタ直下が自分のオーバーレイなら一旦隠して取り直す
    unsafe {
        let hit = WindowFromPoint(pt);
        let root = GetAncestor(hit, GA_ROOT);
        if root == app.overlay {
            overlay::hide(app.overlay);
        }
    }
    let target = unsafe {
        let hit = WindowFromPoint(pt);
        let root = GetAncestor(hit, GA_ROOT);
        if root.is_invalid() {
            return None;
        }
        root.0 as isize
    };
    app.generation += 1;
    app.mode = Mode::Recognizing;
    app.origin = pt;
    app.target = target;
    Some((app.generation, pt.x, pt.y, target, app.cfg.clone(), app.main.0 as isize))
}

fn close_overlay(app: &mut App) {
    overlay::hide(app.overlay);
    app.mode = Mode::Idle;
    app.generation += 1; // 進行中ワーカーの結果を無効化
    app.source.clear();
    app.translation = None;
    app.status = None;
    app.badge = None;
    app.error_only = false;
    app.last_img = None; // OCR画像はサイクル終了後に破棄 (SPEC §9.3)
}

/// ワーカー結果の受信 (世代番号が古いものは破棄; SPEC §6.4)
fn handle_worker(generation: u64, lparam: LPARAM) {
    let msg = unsafe { *Box::from_raw(lparam.0 as *mut worker::WorkerMsg) };
    with_app(|app| {
        if generation != app.generation {
            return;
        }
        match msg {
            worker::WorkerMsg::Source { text, method, img, pin, anchor, ms } => {
                app.source = text;
                app.translation = None;
                app.status = Some("翻訳中…".into());
                app.badge = None;
                app.error_only = false;
                if img.is_some() {
                    app.last_img = img;
                } else if method == "UIA" {
                    app.last_img = None;
                }
                app.anchor = anchor;
                if pin {
                    app.mode = Mode::Pinned;
                } else if app.mode == Mode::Recognizing {
                    app.mode = Mode::ShowingHold;
                }
                util::perf_log(app.cfg.perf_log, &format!("show-source {method} total={ms}ms"));
                sync_overlay(app);
            }
            worker::WorkerMsg::Translation { text, badge, ms } => {
                app.translation = Some(text);
                app.status = None;
                app.badge = badge;
                util::perf_log(app.cfg.perf_log, &format!("show-translation total={ms}ms"));
                sync_overlay(app);
            }
            worker::WorkerMsg::TranslationFailed { msg } => {
                app.status = Some(msg);
                sync_overlay(app);
            }
            worker::WorkerMsg::Error { msg, anchor } => {
                // 短時間のエラー表示 (SPEC §5.3, §11)
                app.source.clear();
                app.translation = None;
                app.status = Some(msg);
                app.anchor = anchor;
                app.error_only = true;
                if app.mode == Mode::Recognizing {
                    app.mode = Mode::ShowingHold;
                }
                sync_overlay(app);
            }
        }
    });
}

/// チップ押下 (SPEC §8): 押下時点でピン留めし、再OCR/再翻訳を実行
fn handle_chip(id: usize) {
    // フェーズ1: 状態取得(借用を解放してから同意ダイアログを出す)
    let Some(info) = with_app(|app| {
        (
            app.cfg.clone(),
            app.source.clone(),
            app.last_img.clone(),
            app.origin,
            app.target,
            app.main.0 as isize,
            app.anchor,
            app.cur_tr.clone(),
        )
    }) else {
        return;
    };
    let (cfg, source, last_img, origin, target, main, anchor, cur_tr) = info;

    match id {
        overlay::CHIP_COPY => {
            let (src, tr) = overlay::current_text();
            let text = tr.unwrap_or(src);
            if !text.is_empty() {
                util::set_clipboard_text(main_hwnd(), &text);
                with_app(|app| {
                    app.badge = Some("コピーしました".into());
                    sync_overlay(app);
                });
            }
            return;
        }
        overlay::CHIP_CLOSE => {
            with_app(close_overlay);
            return;
        }
        _ => {}
    }

    if id < overlay::CHIP_OCR_BASE + overlay::OCR_KEYS.len() {
        // OCRエンジン切替: 保持画像で再認識→現行エンジンで再翻訳 (SPEC §8)
        let key = overlay::OCR_KEYS[id - overlay::CHIP_OCR_BASE].to_string();
        if !cfg.engine_available(&key) {
            with_app(|app| {
                app.status = Some(engine_unavailable_msg(&key));
                sync_overlay(app);
            });
            return;
        }
        if !ensure_consent(&key, &cfg) {
            return;
        }
        let new_gen = with_app(|app| {
            app.generation += 1;
            app.mode = Mode::Pinned;
            app.cur_ocr = key.clone();
            app.status = Some("再認識中…".into());
            sync_overlay(app);
            app.generation
        })
        .unwrap_or(0);
        let cfg2 = Config::load();
        worker::reocr(
            new_gen, last_img, origin.x, origin.y, target, key, cur_tr, cfg2, main, anchor,
        );
    } else if id >= overlay::CHIP_TR_BASE && id < overlay::CHIP_TR_BASE + overlay::TR_KEYS.len() {
        // 翻訳エンジン切替: 現在の原文を選択エンジンで再翻訳 (SPEC §8)
        let key = overlay::TR_KEYS[id - overlay::CHIP_TR_BASE].to_string();
        if source.is_empty() {
            return;
        }
        if !cfg.engine_available(&key) {
            with_app(|app| {
                app.status = Some(engine_unavailable_msg(&key));
                sync_overlay(app);
            });
            return;
        }
        if !ensure_consent(&key, &cfg) {
            return;
        }
        let new_gen = with_app(|app| {
            app.generation += 1;
            app.mode = Mode::Pinned;
            app.cur_tr = key.clone();
            app.status = Some("翻訳中…".into());
            app.translation = None;
            sync_overlay(app);
            app.generation
        })
        .unwrap_or(0);
        let cfg2 = Config::load();
        worker::retranslate(new_gen, key, cfg2, source, main);
    }
}

fn engine_unavailable_msg(key: &str) -> String {
    match key {
        "paddle" => "PaddleOCRのモデルが未導入です。設定画面からインストールしてください".into(),
        "local" => "ローカル翻訳モデルが未導入です。設定画面からインストールしてください".into(),
        "yomitoku" | "ndl" => {
            "サーバーURLが未設定です。設定画面で接続テストを実行してください".into()
        }
        _ => "APIキーが未設定です。設定画面で設定してください".into(),
    }
}

/// 初回同意ダイアログ (SPEC §9.2)。同意済みなら true。
fn ensure_consent(engine: &str, cfg: &Config) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{IDYES, MB_YESNO};
    let (need, kind): (bool, &str) = match engine {
        "deepl" | "google" => (!cfg.consent_text, "text"),
        // Gemini はOCR統合(画像送信)としても翻訳(テキスト送信)としても使う
        "gemini" => (!cfg.consent_image || !cfg.consent_text, "image"),
        "yomitoku" => {
            if settings::is_localhost(&cfg.yomitoku_url) {
                (false, "ext") // 127.0.0.1 はローカル送信 (SPEC §9.2)
            } else {
                (!cfg.consent_ext_ocr, "ext")
            }
        }
        "ndl" => {
            if settings::is_localhost(&cfg.ndl_url) {
                (false, "ext")
            } else {
                (!cfg.consent_ext_ocr, "ext")
            }
        }
        _ => (false, ""),
    };
    if !need {
        return true;
    }
    let msg = match kind {
        "text" => w!(
            "このエンジンはOCR済みの原文テキストを外部サービスへ送信します。\n初回のみ確認しています。許可しますか?"
        ),
        "image" => w!(
            "このエンジンはキャプチャ画像と言語設定を外部サービスへ送信する可能性があります。\n初回のみ確認しています。許可しますか?"
        ),
        _ => w!(
            "このエンジンは設定されたサーバーURLへキャプチャ画像を送信します。\n初回のみ確認しています。許可しますか?"
        ),
    };
    let r = unsafe { MessageBoxW(Some(main_hwnd()), msg, w!("外部送信の同意"), MB_YESNO) };
    if r == IDYES {
        let mut c = Config::load();
        match kind {
            "text" => c.consent_text = true,
            "image" => {
                c.consent_image = true;
                c.consent_text = true;
            }
            _ => c.consent_ext_ocr = true,
        }
        c.save();
        with_app(|app| app.cfg = c);
        true
    } else {
        false
    }
}

/// 範囲指定の選択結果 (SPEC §3.2): 最初からピン留めで表示
fn handle_region(rect: RECT) {
    let Some((generation, cfg, main)) = with_app(|app| {
        overlay::hide(app.overlay);
        app.generation += 1;
        app.mode = Mode::Recognizing;
        app.origin = POINT { x: (rect.left + rect.right) / 2, y: (rect.top + rect.bottom) / 2 };
        (app.generation, app.cfg.clone(), app.main.0 as isize)
    }) else {
        return;
    };
    worker::region_cycle(generation, rect, cfg, main);
}

fn reload_config(hwnd: HWND) {
    with_app(|app| {
        app.cfg = Config::load();
        app.cur_ocr = app.cfg.default_ocr.clone();
        app.cur_tr = app.cfg.default_translator.clone();
        unsafe {
            // タイマー周期・ホットキーを再適用
            let _ = KillTimer(Some(hwnd), TIMER_POLL);
            SetTimer(Some(hwnd), TIMER_POLL, app.cfg.poll_ms, None);
            let _ = UnregisterHotKey(Some(hwnd), HOTKEY_REGION);
            let (mods, vk) = app.cfg.region_hotkey_parsed();
            if RegisterHotKey(Some(hwnd), HOTKEY_REGION, HOT_KEY_MODIFIERS(mods), vk).is_err() {
                MessageBoxW(
                    Some(hwnd),
                    w!("範囲指定ホットキーを登録できませんでした。他のアプリと衝突しています。"),
                    w!("Focus Translator"),
                    MB_OK | MB_ICONWARNING,
                );
            }
        }
    });
}

/// App の状態をオーバーレイへ反映
fn sync_overlay(app: &mut App) {
    let mut ocr_enabled = [false; 5];
    for (i, k) in overlay::OCR_KEYS.iter().enumerate() {
        ocr_enabled[i] = app.cfg.engine_available(k);
    }
    let mut tr_enabled = [false; 4];
    for (i, k) in overlay::TR_KEYS.iter().enumerate() {
        tr_enabled[i] = app.cfg.engine_available(k);
    }
    let content = OverlayContent {
        main_hwnd: app.main.0 as isize,
        anchor: app.anchor,
        source: app.source.clone(),
        translation: app.translation.clone(),
        status: app.status.clone(),
        badge: app.badge.clone(),
        pinned: app.mode == Mode::Pinned,
        cur_ocr: app.cur_ocr.clone(),
        cur_tr: app.cur_tr.clone(),
        ocr_enabled,
        tr_enabled,
        error_only: app.error_only,
    };
    overlay::update(app.overlay, content);
}
