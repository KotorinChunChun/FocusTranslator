// Focus Translator v0.1 — カーソル位置翻訳ツール (FocusTranslator_SPECv0.1.md 準拠)
// 右Ctrlホールド中のみ、マウスポインタ直下のテキスト1行を認識・翻訳して
// カーソル近傍にオーバーレイ表示するタスクトレイ常駐ツール。
use crate::capture;
use crate::config::Config;
use crate::detect;
use crate::overlay;
use crate::logviewer;
use crate::region;
use crate::settings;
use crate::tray;
use crate::util;
use crate::worker;


use overlay::OverlayContent;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Arc;
use windows::Win32::Foundation::{
    HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetCursorPos, GetForegroundWindow, MessageBoxW,
    WindowFromPoint, KillTimer, SetTimer, MB_OK, MB_ICONWARNING,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, HOT_KEY_MODIFIERS, RegisterHotKey, UnregisterHotKey, VK_ESCAPE,
};
use windows::core::{w, PCWSTR};

/// 埋め込みリソース(build.rs で ID "1" として同梱)からアプリアイコンを取得する。
/// exeアイコン・通知領域アイコン・各ウィンドウのタイトルバーアイコンで共用する。
#[allow(clippy::manual_dangling_ptr)] // MAKEINTRESOURCE(1) 相当。実在するリソースIDを指すため意図的
pub fn app_icon() -> windows::Win32::UI::WindowsAndMessaging::HICON {
    unsafe {
        let inst = HINSTANCE(windows::Win32::System::LibraryLoader::GetModuleHandleW(None).map(|m| m.0).unwrap_or(std::ptr::null_mut()));
        windows::Win32::UI::WindowsAndMessaging::LoadIconW(Some(inst), PCWSTR(1usize as *const u16)).unwrap_or_default()
    }
}

// アプリ内メッセージ
pub const WM_APP_TRAY: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;
pub const WM_APP_WORKER: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 2;
pub const WM_APP_CHIP: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 3;
pub const WM_APP_REGION: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 4;
pub const WM_APP_CFG: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 5;
pub const WM_APP_DETECT: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 6;

pub const TIMER_POLL: usize = 1;
pub const HOTKEY_REGION: i32 = 1;

#[derive(PartialEq, Clone, Copy)]
pub enum Mode {
    Idle,
    Recognizing,
    ShowingHold,
    Pinned,
}

pub struct App {
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
    /// 保持画像の行選択モード (再OCR時に同じモードで認識する)
    last_focus: crate::ocr::Focus,
    anchor: (i32, i32),
    pub error_only: bool,
    failed_ocr: HashSet<String>,
    failed_tr: HashSet<String>,
    recog_id: Option<i64>,
    explanation: Option<String>,
    /// 解説をLLMへ問い合わせ中 (オーバーレイに取得中表示を出す)
    explaining: bool,
    /// 時間のかかる処理(再認識・再翻訳・解説)の実行中。オーバーレイの操作をロックする。
    busy: bool,
    app_title: String,
    uia_path: String,
    /// 直近の認識が UIA 経路(OCR不要)で得られたか
    via_uia: bool,
    pub scroll_y: i32,
    /// 領域検出モード: 検出キー押下中でオーバーレイ表示中か
    detect_on: bool,
    /// 領域検出モード: 検出スレッドの実行中 (多重起動防止)
    detect_busy: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static MAIN_HWND: RefCell<isize> = const { RefCell::new(0) };
}

pub fn main_hwnd() -> HWND {
    HWND(MAIN_HWND.with(|h| *h.borrow()) as *mut _)
}

pub fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| {
        let Ok(mut guard) = a.try_borrow_mut() else {
            return None;
        };
        guard.as_mut().map(f)
    })
}

pub fn init(cfg: Config, instance: HINSTANCE, main: HWND, overlay: HWND) {
    MAIN_HWND.with(|h| *h.borrow_mut() = main.0 as isize);
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
            last_focus: crate::ocr::Focus::All,
            anchor: (0, 0),
            error_only: false,
            failed_ocr: HashSet::new(),
            failed_tr: HashSet::new(),
            recog_id: None,
            explanation: None,
            explaining: false,
            busy: false,
            app_title: String::new(),
            uia_path: String::new(),
            via_uia: false,
            scroll_y: 0,
            detect_on: false,
            detect_busy: false,
        });
    });
}

#[allow(clippy::missing_safety_doc)]
pub unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        windows::Win32::UI::WindowsAndMessaging::WM_TIMER if wparam.0 == TIMER_POLL => {
            tick();
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_HOTKEY if wparam.0 as i32 == HOTKEY_REGION => {
            let inst = with_app(|app| app.instance).unwrap_or_default();
            region::start(inst, hwnd);
            LRESULT(0)
        }
        WM_APP_TRAY => {
            let ev = (lparam.0 & 0xFFFF) as u32;
            if ev == windows::Win32::UI::WindowsAndMessaging::WM_RBUTTONUP || ev == windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONUP {
                let cmd = tray::show_menu(hwnd);
                handle_command(hwnd, cmd);
            }
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_COMMAND => {
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
        WM_APP_DETECT => {
            let info = unsafe { Box::from_raw(lparam.0 as *mut detect::DetectInfo) };
            // 検出スレッド完了。キーが既に離されていれば結果は捨てる。
            let showing = with_app(|app| {
                app.detect_busy = false;
                app.detect_on
            })
            .unwrap_or(false);
            if showing {
                detect::update(*info);
            }
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_DESTROY => {
            tray::remove_icon(hwnd);
            unsafe {
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey(Some(hwnd), HOTKEY_REGION);
                let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(hwnd), TIMER_POLL);
                windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

pub fn handle_command(hwnd: HWND, cmd: usize) {
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
pub fn tick() {
    let action = with_app(|app| {
        let down = unsafe { (GetAsyncKeyState(app.cfg.hold_vk()) as u16 & 0x8000) != 0 };
        let esc = unsafe { (GetAsyncKeyState(VK_ESCAPE.0 as i32) as u16 & 0x8000) != 0 };

        // ピン留め中のEscで閉じる (SPEC §2.1)。
        // ただし対象アプリがアクティブ(フォアグラウンド)なときのみ。設定・解説編集など
        // 自アプリの別ウィンドウや無関係な他アプリがアクティブな間のEscでは閉じない。
        let target_active = unsafe { GetForegroundWindow() } == HWND(app.target as *mut _);
        if app.mode == Mode::Pinned && esc && target_active {
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

    tick_detect();
}

/// 領域検出モード (デバッグ) のポーリング: プレビューキー(既定LCtrl)または
/// キャプチャキー(既定RCtrl。実際の翻訳ホールドと兼用)の押下中はオーバーレイを表示し、
/// 検出スレッドを1本ずつ回して結果 (WM_APP_DETECT) で枠表示を更新し続ける。
/// それぞれ独立した「領域表示」チェックボックスでON/OFFする(既定OFF)。
fn tick_detect() {
    // (表示開始するインスタンス, 検出を開始する main HWND, 非表示にするか)
    let action = with_app(|app| {
        let down = unsafe {
            (app.cfg.detect_enabled && (GetAsyncKeyState(app.cfg.hold_vk()) as u16 & 0x8000) != 0)
                || (app.cfg.preview_detect_enabled
                    && (GetAsyncKeyState(app.cfg.detect_vk()) as u16 & 0x8000) != 0)
        };
        if !down {
            if app.detect_on {
                app.detect_on = false;
                app.detect_busy = false;
                return Some((None, None, true));
            }
            return None;
        }
        let show = if !app.detect_on {
            app.detect_on = true;
            Some(app.instance)
        } else {
            None
        };
        let probe = if !app.detect_busy {
            app.detect_busy = true;
            Some(app.main.0 as isize)
        } else {
            None
        };
        Some((show, probe, false))
    })
    .flatten();

    if let Some((show, probe, hide)) = action {
        if hide {
            detect::hide();
        }
        if let Some(inst) = show {
            detect::show(inst);
        }
        if let Some(main) = probe {
            detect::probe(main);
        }
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
    app.last_focus = crate::ocr::Focus::All;
    app.recog_id = None;
    app.explanation = None;
    app.explaining = false;
    app.busy = false;
    app.via_uia = false;
}

/// ワーカー結果の受信 (世代番号が古いものは破棄; SPEC §6.4)
pub fn handle_worker(generation: u64, lparam: LPARAM) {
    let msg = unsafe { *Box::from_raw(lparam.0 as *mut worker::WorkerMsg) };
    with_app(|app| {
        if generation != app.generation {
            return;
        }
        app.busy = false; // ワーカー完了 (成否問わずロック解除)
        match msg {
            worker::WorkerMsg::Source { text, method, engine, img, pin, anchor, focus, ms, recog_id, app_title, uia_path } => {
                if !app.error_only && app.status.is_none() && !text.is_empty() && text == app.source {
                    return;
                }
                app.source = text;
                app.via_uia = method == "UIA";
                if let Some(e) = engine {
                    app.cur_ocr = e; // 実際に使ったOCRエンジン (UIA経路では変更しない)
                }
                app.last_img = img;
                app.last_focus = focus;
                app.status = None;
                app.badge = None;
                app.error_only = false;
                app.anchor = anchor;
                app.failed_ocr.clear();
                app.failed_tr.clear();
                app.recog_id = recog_id;
                app.app_title = app_title;
                app.uia_path = uia_path;
                app.scroll_y = 0; // 新しいテキストの時はスクロールをリセット

                if pin {
                    app.mode = Mode::Pinned;
                } else if app.mode == Mode::Recognizing {
                    app.mode = Mode::ShowingHold;
                }
                util::perf_log(app.cfg.perf_log, &format!("show-source {method} total={ms}ms"));
                sync_overlay(app);
            }
            worker::WorkerMsg::Translation { text, badge, ms, recog_id } => {
                app.failed_tr.remove(&app.cur_tr);
                app.translation = Some(text);
                app.status = None;
                app.error_only = false;
                app.badge = badge;
                app.recog_id = recog_id;
                util::perf_log(app.cfg.perf_log, &format!("show-translation total={ms}ms"));
                sync_overlay(app);
            }
            worker::WorkerMsg::TranslationFailed { msg, engine } => {
                app.status = Some(msg);
                app.failed_tr.insert(engine);
                sync_overlay(app);
            }
            worker::WorkerMsg::Error { msg, anchor, engine } => {
                if let Some(e) = engine {
                    app.failed_ocr.insert(e);
                }
                app.explaining = false;
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
            worker::WorkerMsg::Explanation { text } => {
                app.mode = Mode::Pinned;
                app.status = None;
                app.error_only = false;
                app.explaining = false;
                app.explanation = Some(text);
                sync_overlay(app);
            }
        }
    });
}

/// チップ押下 (SPEC §8): 押下時点でピン留めし、再OCR/再翻訳を実行
pub fn handle_chip(id: usize) {
    // フェーズ1: 状態取得(借用を解放してから同意ダイアログを出す)
    let Some(info) = with_app(|app| {
        (
            app.cfg.clone(),
            app.source.clone(),
            app.last_img.clone(),
            app.last_focus,
            app.origin,
            app.target,
            app.main.0 as isize,
            app.anchor,
            app.cur_tr.clone(),
            app.recog_id,
        )
    }) else {
        return;
    };
    let (cfg, source, last_img, last_focus, origin, target, main, anchor, cur_tr, recog_id) = info;

    match id {
        overlay::CHIP_COPY => {
            // 解説コピー: 解説文をコピーする (ボタンは解説表示中のみ出る)
            let text = with_app(|app| app.explanation.clone()).flatten().unwrap_or_default();
            if !text.is_empty() {
                util::set_clipboard_text(main_hwnd(), &text);
                with_app(|app| {
                    app.badge = Some("解説をコピーしました".into());
                    sync_overlay(app);
                });
            }
        }
        overlay::CHIP_COPY_SRC => {
            let (src, _) = overlay::current_text();
            if !src.is_empty() {
                util::set_clipboard_text(main_hwnd(), &src);
                with_app(|app| {
                    app.badge = Some("原文をコピーしました".into());
                    sync_overlay(app);
                });
            }
        }
        overlay::CHIP_COPY_TR => {
            let (_, tr) = overlay::current_text();
            if let Some(t) = tr {
                util::set_clipboard_text(main_hwnd(), &t);
                with_app(|app| {
                    app.badge = Some("訳文をコピーしました".into());
                    sync_overlay(app);
                });
            }
        }
        overlay::CHIP_COPY_INFO => {
            let (title, path) =
                with_app(|app| (app.app_title.clone(), app.uia_path.clone())).unwrap_or_default();
            let mut text = title;
            if !path.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&path);
            }
            if !text.is_empty() {
                util::set_clipboard_text(main_hwnd(), &text);
                with_app(|app| {
                    app.badge = Some("対象情報をコピーしました".into());
                    sync_overlay(app);
                });
            }
        }
        overlay::CHIP_CLOSE => {
            with_app(close_overlay);
            return;
        }
        overlay::CHIP_PIN => {
            with_app(|app| {
                if app.mode != Mode::Pinned {
                    app.mode = Mode::Pinned;
                } else {
                    app.mode = Mode::ShowingHold;
                }
                sync_overlay(app);
            });
            return;
        }
        overlay::CHIP_IMAGE => {
            let img = with_app(|app| app.last_img.clone()).flatten();
            if let Some(i) = img {
                let png = crate::capture::to_png(&i);
                let path = crate::logdb::logs_dir().join("preview.png");
                if std::fs::write(&path, &png).is_ok() {
                    unsafe {
                        let wide = crate::util::to_wide(&path.to_string_lossy());
                        let _ = windows::Win32::UI::Shell::ShellExecuteW(
                            None,
                            windows::core::w!("open"),
                            windows::core::PCWSTR(wide.as_ptr()),
                            windows::core::PCWSTR::null(),
                            windows::core::PCWSTR::null(),
                            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
                        );
                    }
                }
            }
            return;
        }
        overlay::CHIP_EXPLAIN => {
            // 押した瞬間にピン留め状態にする(ダイアログ表示中にホールド解除で閉じないように)
            with_app(|app| {
                app.mode = Mode::Pinned;
                sync_overlay(app);
            });
            if let Some(r_id) = recog_id {
                let text = overlay::current_text().1.unwrap_or(source.clone());
                if !text.is_empty() {
                    if let Some(expl) = crate::logdb::get_explanation(r_id) {
                        with_app(|app| {
                            app.mode = Mode::Pinned;
                            app.status = None;
                            app.error_only = false;
                            app.explanation = Some(expl);
                            sync_overlay(app);
                        });
                        return;
                    }

                    // アプリ名・UIAパスを前提情報としてプロンプトへ、
                    // 編集ダイアログはオーバーレイの近くに表示する
                    let (app_title, uia_path, dialog_pos) = with_app(|app| {
                        let mut r = RECT::default();
                        unsafe {
                            let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(app.overlay, &mut r);
                        }
                        (app.app_title.clone(), app.uia_path.clone(), (r.left + 24, r.top + 24))
                    })
                    .unwrap_or_default();

                    let initial_prompt =
                        crate::worker::build_explain_prompt(&cfg, &text, &app_title, &uia_path).unwrap_or_default();
                    if initial_prompt.is_empty() {
                        with_app(|app| {
                            app.badge = Some("LLM APIが設定されていません".into());
                            sync_overlay(app);
                        });
                        return;
                    }

                    let profiles: Vec<String> = cfg.api_profiles.iter().map(|p| p.name.clone()).collect();
                    let active_idx = cfg
                        .api_profiles
                        .iter()
                        .position(|p| p.name == cfg.active_api_profile)
                        .unwrap_or(0);

                    let main_hwnd_val = main;
                    let inst = with_app(|app| app.instance).unwrap_or_default();
                    crate::prompt_edit::open(
                        inst,
                        main_hwnd(),
                        Some(dialog_pos),
                        &initial_prompt,
                        profiles,
                        active_idx,
                        move |edited_prompt, profile| {
                            let new_gen = with_app(|app| {
                                app.generation += 1;
                                app.mode = Mode::Pinned;
                                app.status = Some("解説を取得中…".into());
                                app.explaining = true;
                                app.busy = true;
                                sync_overlay(app);
                                app.generation
                            })
                            .unwrap_or(0);
                            let cfg2 = crate::config::Config::load();
                            crate::worker::explain(new_gen, r_id, cfg2, edited_prompt, profile, main_hwnd_val);
                        },
                    );
                }
            }
            return;
        }
        overlay::CHIP_SETTINGS => {
            let inst = with_app(|app| app.instance).unwrap_or_default();
            crate::settings::open(inst, main_hwnd());
            return;
        }
        _ => {}
    }

    if id < overlay::CHIP_OCR_BASE + overlay::OCR_KEYS.len() {
        // OCRエンジン切替: 保持画像で再認識→現行エンジンで再翻訳 (SPEC §8)
        let key = overlay::OCR_KEYS[id - overlay::CHIP_OCR_BASE].to_string();
        with_app(|app| { app.failed_ocr.remove(&key); });
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
            app.busy = true;
            sync_overlay(app);
            app.generation
        })
        .unwrap_or(0);
        let cfg2 = Config::load();
        worker::reocr(
            new_gen, last_img, last_focus, origin.x, origin.y, target, key, cur_tr, cfg2, main, anchor,
        );
    } else if id >= overlay::CHIP_TR_BASE && id < overlay::CHIP_TR_BASE + overlay::TR_KEYS.len() {
        // 翻訳エンジン切替: 現在の原文を選択エンジンで再翻訳 (SPEC §8)
        let key = overlay::TR_KEYS[id - overlay::CHIP_TR_BASE].to_string();
        with_app(|app| { app.failed_tr.remove(&key); });
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
            app.busy = true;
            sync_overlay(app);
            app.generation
        })
        .unwrap_or(0);
        let cfg2 = Config::load();
        worker::retranslate(new_gen, key, cfg2, source, main, recog_id);
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
        "llm" => (!cfg.consent_image || !cfg.consent_text, "image"),
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
pub fn handle_region(rect: RECT) {
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

pub fn reload_config(hwnd: HWND) {
    with_app(|app| {
        app.failed_ocr.clear();
        app.failed_tr.clear();
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
        sync_overlay(app);
    });
}

/// App の状態をオーバーレイへ反映
fn sync_overlay(app: &mut App) {
    let mut ocr_enabled = [false; overlay::OCR_KEYS.len()];
    for (i, k) in overlay::OCR_KEYS.iter().enumerate() {
        ocr_enabled[i] = app.cfg.engine_available(k) && !app.failed_ocr.contains(*k);
    }
    let mut tr_enabled = [false; overlay::TR_KEYS.len()];
    for (i, k) in overlay::TR_KEYS.iter().enumerate() {
        tr_enabled[i] = app.cfg.engine_available(k) && !app.failed_tr.contains(*k);
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
        via_uia: app.via_uia,
        ocr_enabled,
        tr_enabled,
        explanation: app.explanation.clone(),
        explaining: app.explaining,
        error_only: app.error_only,
        app_title: app.app_title.clone(),
        uia_path: app.uia_path.clone(),
        scroll_y: app.scroll_y,
        has_image: app.last_img.is_some(),
        busy: app.busy,
    };
    overlay::update(app.overlay, content);
}
