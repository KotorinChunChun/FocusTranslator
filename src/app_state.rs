// アプリケーション状態管理 (SPEC v0.3)
// メインウィンドウプロシージャ・状態遷移・ポーリング・UI同期を担う。
// チップ操作は chip_handler モジュールに委譲する。
use crate::capture;
use crate::config::Config;
use crate::detect;
use crate::engine;
use crate::overlay;
use crate::logviewer;
use crate::region;
use crate::settings;
use crate::tray;
use crate::util;
use crate::worker;

use overlay::OverlayContent;
use std::cell::RefCell;
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
#[allow(clippy::manual_dangling_ptr)]
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
    pub cfg: Config,
    pub instance: HINSTANCE,
    pub main: HWND,
    pub overlay: HWND,
    pub generation: u64,
    hold: bool,
    pub mode: Mode,
    /// サイクル開始時のカーソル位置(行逸脱監視用)
    pub origin: POINT,
    /// サイクル開始時の対象ウィンドウ(再OCR用)
    pub target: isize,
    pub source: String,
    pub translation: Option<String>,
    pub status: Option<String>,
    pub badge: Option<String>,
    pub cur_ocr: String,
    pub cur_tr: String,
    pub last_img: Option<Arc<capture::Captured>>,
    /// 保持画像の行選択モード (再OCR時に同じモードで認識する)
    pub last_focus: crate::ocr::Focus,
    /// 対象アプリ全体のキャプチャ画像 (画像編集の「全体画像の復元」用。SPECv0.5.2追補)
    pub last_full_img: Option<Arc<capture::Captured>>,
    /// last_full_img 内での last_img の位置 (物理ピクセル座標)
    pub last_crop_rect: Option<RECT>,
    pub anchor: (i32, i32),
    pub error_only: bool,
    pub capture_id: Option<i64>,
    pub recog_id: Option<i64>,
    pub explanation: Option<String>,
    /// 現在の explanation を生成したLLMプロファイル名 (SPECv0.5.2追補: プロファイル別ボタンの
    /// 選択状態表示・キャッシュ判定に使う)
    pub explain_profile: String,
    /// 解説をLLMへ問い合わせ中
    pub explaining: bool,
    /// 時間のかかる処理の実行中。オーバーレイの操作をロックする。
    pub busy: bool,
    pub app_title: String,
    pub app_exe: String,
    pub uia_path: String,
    /// uia_nodes のJSON表現 (SPECv0.5.2追補: ログDB記録用)
    pub uia_json: String,
    /// UIAパスの各ノード
    pub uia_nodes: Vec<crate::uia::UiaPathNode>,
    /// カーソル位置要素のUIA ControlType名 (入力内容ログ用; SPECv0.4.8追補)
    pub control_type: Option<String>,
    /// カーソル位置要素で選択中のテキスト (SPECv0.5追補)。
    /// 「選択中の文字列」チップの活性判定・ツールチップ・手動再採用に使う。
    pub selected_text: Option<String>,
    /// 直近の認識が UIA 経路で得られたか
    pub via_uia: bool,
    pub scroll_y: i32,
    /// 領域検出モード: 検出キー押下中でオーバーレイ表示中か
    detect_on: bool,
    /// 領域検出処理が現在実行中か
    pub detect_busy: bool,
    /// 直前のループでCtrl+Zが押されていたか (エッジ検出用)
    pub ctrl_z_prev: bool,
    /// 現在インライン編集中の対象ブロック (SPECv0.4)
    pub editing_block: overlay::EditBlock,
    /// ホールド開始時刻 (pin_hold_seconds 秒の長押しでピン留め。既定3秒)
    pub hold_start: Option<std::time::Instant>,
}

impl App {
    /// 保持中の対象アプリ情報からキャプチャ時点の AppContext を復元する
    /// (再認識ジョブへ渡す用。exe は空文字を None として扱う)。
    pub fn held_ctx(&self) -> worker::AppContext {
        worker::AppContext {
            exe: if self.app_exe.is_empty() { None } else { Some(self.app_exe.clone()) },
            title: self.app_title.clone(),
            uia_path: self.uia_path.clone(),
            uia_json: self.uia_json.clone(),
            uia_nodes: self.uia_nodes.clone(),
            control_type: self.control_type.clone(),
            selected_text: self.selected_text.clone(),
        }
    }
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
            hold_start: None,
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
            last_full_img: None,
            last_crop_rect: None,
            anchor: (0, 0),
            error_only: false,
            capture_id: None,
            recog_id: None,
            explanation: None,
            explain_profile: String::new(),
            explaining: false,
            busy: false,
            app_title: String::new(),
            app_exe: String::new(),
            uia_path: String::new(),
            uia_json: String::new(),
            uia_nodes: Vec::new(),
            control_type: None,
            selected_text: None,
            via_uia: false,
            scroll_y: 0,
            detect_on: false,
            detect_busy: false,
            ctrl_z_prev: false,
            editing_block: overlay::EditBlock::None,
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
            crate::chip_handler::handle_chip(wparam.0);
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
        windows::Win32::UI::WindowsAndMessaging::WM_SETTINGCHANGE => {
            // Windowsのライト/ダークモード切替に追従する
            // (テーマ設定が「Windows既定」のときだけ apply_theme の結果が変わる)。
            with_app(|app| {
                if crate::overlay_layout::apply_theme(&app.cfg.overlay_theme)
                    && unsafe {
                        windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(app.overlay)
                    }
                    .as_bool()
                {
                    overlay::refresh(app.overlay);
                }
            });
            unsafe { windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_APP_DETECT => {
            let info = unsafe { Box::from_raw(lparam.0 as *mut detect::DetectInfo) };
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

/// 画像編集パネル表示中の Ctrl+Z (元に戻す) をエッジ検出する。オーバーレイは
/// WS_EX_NOACTIVATE でキーボードフォーカスを持てないため、他のホットキーと同様に
/// GetAsyncKeyState のポーリングで検出する (SPECv0.4追補)。
fn tick_edit_undo_hotkey() {
    if !overlay::is_editing_image() {
        with_app(|app| app.ctrl_z_prev = false);
        return;
    }
    const VK_CONTROL: i32 = 0x11;
    const VK_Z: i32 = 0x5A;
    let down = unsafe {
        (GetAsyncKeyState(VK_CONTROL) as u16 & 0x8000) != 0
            && (GetAsyncKeyState(VK_Z) as u16 & 0x8000) != 0
    };
    let edge = with_app(|app| {
        let prev = app.ctrl_z_prev;
        app.ctrl_z_prev = down;
        !prev && down
    })
    .unwrap_or(false);
    if edge {
        crate::chip_handler::perform_edit_undo();
    }
}

/// 100ms周期のポーリング (SPEC §4)
pub fn tick() {
    tick_edit_undo_hotkey();
    let action = with_app(|app| {
        let down = unsafe { (GetAsyncKeyState(app.cfg.hold_vk()) as u16 & 0x8000) != 0 };
        let esc = unsafe { (GetAsyncKeyState(VK_ESCAPE.0 as i32) as u16 & 0x8000) != 0 };

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
                app.hold_start = Some(std::time::Instant::now());
                start_cycle_params(app)
            }
            (true, true) => {
                if let Some(start) = app.hold_start
                    && start.elapsed().as_secs() >= app.cfg.pin_hold_seconds as u64
                    && app.mode != Mode::Pinned
                {
                    app.mode = Mode::Pinned;
                    sync_overlay(app);
                }
                None
            }
            (true, false) => {
                app.hold_start = None;
                if app.mode != Mode::Pinned {
                    close_overlay(app);
                }
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

/// 領域検出モード (デバッグ) のポーリング
fn tick_detect() {
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

/// 認識サイクルの開始準備
fn start_cycle_params(app: &mut App) -> Option<(u64, i32, i32, isize, Config, isize)> {
    // 画像編集画面を開いたまま新規キャプチャした場合は、古い編集セッションを破棄してから開始する。
    overlay::exit_edit_mode();
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
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
    // 既定値の適用はこの右Ctrl起動時のみ。現在エンジン/プロファイルを既定へリセットしてから
    // 認識サイクルを開始する (設定画面での既定変更が現行オーバーレイに波及しないのはこのため)。
    app.cur_ocr = app.cfg.default_ocr.clone();
    app.cur_tr = app.cfg.default_translator.clone();
    app.cfg.active_api_profile = app.cfg.default_api_profile.clone();
    Some((app.generation, pt.x, pt.y, target, app.cfg.clone(), app.main.0 as isize))
}

pub fn close_overlay(app: &mut App) {
    overlay::hide(app.overlay);
    app.mode = Mode::Idle;
    app.generation += 1;
    app.source.clear();
    app.translation = None;
    app.status = None;
    app.badge = None;
    app.error_only = false;
    app.last_img = None;
    app.last_focus = crate::ocr::Focus::All;
    app.last_full_img = None;
    app.last_crop_rect = None;
    app.capture_id = None;
    app.recog_id = None;
    app.explanation = None;
    app.explain_profile = String::new();
    app.explaining = false;
    app.busy = false;
    app.via_uia = false;
    app.control_type = None;
    app.selected_text = None;
}

/// ワーカー結果の受信 (世代番号が古いものは破棄; SPEC §6.4)
pub fn handle_worker(generation: u64, lparam: LPARAM) {
    let msg = unsafe { *Box::from_raw(lparam.0 as *mut worker::WorkerMsg) };
    with_app(|app| {
        if generation != app.generation {
            return;
        }
        app.busy = false;
        match msg {
            worker::WorkerMsg::Source(src) => {
                let worker::SourceMsg {
                    text, method, engine, img, pin, anchor, focus, ms, capture_id, recog_id, ctx,
                    full_img, crop_rect,
                } = *src;
                if !app.error_only && app.status.is_none() && !text.is_empty() && text == app.source {
                    return;
                }
                app.source = text;
                app.via_uia = method == "UIA";
                if let Some(e) = engine {
                    app.cur_ocr = e;
                }
                app.last_img = img;
                app.last_focus = focus;
                app.last_full_img = full_img;
                app.last_crop_rect = crop_rect;
                // 原文が空のSourceは「OCR失敗だが再試行用に画像だけ保持したい」ケース
                // (SPECv0.5.2追補)。この直後に届く WorkerMsg::Error が実際のエラー文言を
                // status へ入れるため、ここでは誤った成功メッセージを出さない。
                if app.source.is_empty() {
                    // no-op: 直後のErrorメッセージがstatusを設定する
                } else if app.via_uia {
                    app.status = Some("UIからの文字抽出に成功しました。".to_string());
                } else {
                    app.status = Some("画像認識により文字起こししました。".to_string());
                }
                app.badge = None;
                app.error_only = false;
                app.anchor = anchor;
                app.capture_id = capture_id;
                app.recog_id = recog_id;
                app.app_title = ctx.title;
                app.app_exe = ctx.exe.unwrap_or_default();
                app.uia_path = ctx.uia_path;
                app.uia_json = ctx.uia_json;
                app.uia_nodes = ctx.uia_nodes;
                app.control_type = ctx.control_type;
                app.selected_text = ctx.selected_text;
                app.scroll_y = 0;
                // キャプチャ内容が変わったら解説を初期化する (SPECv0.4.8追補: 開く度に解説をリセット)
                app.explanation = None;
                app.explain_profile = String::new();
                app.explaining = false;

                if pin {
                    app.mode = Mode::Pinned;
                } else if app.mode == Mode::Recognizing {
                    app.mode = Mode::ShowingHold;
                }
                util::perf_log(app.cfg.perf_log, &format!("show-source {method} total={ms}ms"));
                sync_overlay(app);
            }
            worker::WorkerMsg::Translation { text, badge, ms, recog_id } => {
                app.translation = Some(text);
                app.status = None;
                app.error_only = false;
                app.badge = badge;
                app.recog_id = recog_id;
                util::perf_log(app.cfg.perf_log, &format!("show-translation total={ms}ms"));
                sync_overlay(app);
            }
            worker::WorkerMsg::TranslationSkipped { msg } => {
                app.translation = None;
                app.status = Some(msg);
                app.error_only = false;
                sync_overlay(app);
            }
            worker::WorkerMsg::TranslationFailed { msg } => {
                // 見出し・エンジン切替チップは残したいので translation を None にはしない。
                // 本文だけ空にし、エラー内容はシステムメッセージ行 (status) へ出す。
                app.translation = Some(String::new());
                app.status = Some(msg);
                sync_overlay(app);
            }
            worker::WorkerMsg::Error { msg, anchor, clear_source } => {
                app.explaining = false;
                if clear_source {
                    // OCRエンジン切替・画像編集後の再認識失敗: 古い認識結果を残さず消す
                    app.source.clear();
                    app.translation = None;
                    app.status = Some(msg);
                    app.error_only = false;
                } else if !app.source.is_empty() || app.last_img.is_some() {
                    // 原文が空でも保持画像があれば(直前にOCR失敗を示すSourceを受信済みの場合)
                    // ブロックごと消す単一行モードにはせず、OCRエンジン切替チップを残す
                    // (SPECv0.5.2追補)。
                    app.status = Some(msg);
                    app.error_only = false;
                } else {
                    app.translation = None;
                    app.status = Some(msg);
                    app.anchor = anchor;
                    app.error_only = true;
                    if app.mode == Mode::Recognizing {
                        app.mode = Mode::ShowingHold;
                    }
                }
                sync_overlay(app);
            }
            worker::WorkerMsg::Explanation { text, profile } => {
                app.mode = Mode::Pinned;
                app.status = None;
                app.error_only = false;
                app.explaining = false;
                app.explanation = Some(text);
                app.explain_profile = profile;
                sync_overlay(app);
            }
        }
    });
}

/// 範囲指定の選択結果 (SPEC §3.2)
pub fn handle_region(rect: RECT) {
    // 画像編集画面を開いたまま範囲指定した場合も、ホールドキー起動時と同様に
    // 古い編集セッションを破棄してから開始する (展開したまま前の画像が残るのを防ぐ)。
    overlay::exit_edit_mode();
    let Some((generation, cfg, main)) = with_app(|app| {
        overlay::hide(app.overlay);
        app.generation += 1;
        app.mode = Mode::Recognizing;
        app.origin = POINT { x: (rect.left + rect.right) / 2, y: (rect.top + rect.bottom) / 2 };
        // 範囲指定も起動経路なので、既定エンジン/プロファイルをこの時点で適用する。
        app.cur_ocr = app.cfg.default_ocr.clone();
        app.cur_tr = app.cfg.default_translator.clone();
        app.cfg.active_api_profile = app.cfg.default_api_profile.clone();
        (app.generation, app.cfg.clone(), app.main.0 as isize)
    }) else {
        return;
    };
    worker::region_cycle(generation, rect, cfg, main);
}

pub fn reload_config(hwnd: HWND) {
    with_app(|app| {
        // 既定エンジン/プロファイルの変更は現行オーバーレイへ波及させない。既定値は
        // 右Ctrl起動時 (start_cycle_params) にのみ適用される役割なので、ここでは
        // セッションで使用中のエンジン (cur_ocr/cur_tr) とアクティブプロファイルを据え置く。
        let keep_profile = app.cfg.active_api_profile.clone();
        app.cfg = Config::load();
        app.cfg.active_api_profile = keep_profile;
        // オーバーレイのテーマ設定を反映する (sync_overlay の再描画で配色が切り替わる)
        crate::overlay_layout::apply_theme(&app.cfg.overlay_theme);
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_POLL);
            SetTimer(Some(hwnd), TIMER_POLL, app.cfg.poll_ms, None);
            let _ = UnregisterHotKey(Some(hwnd), HOTKEY_REGION);
            let (mods, vk) = app.cfg.region_hotkey_parsed();
            if RegisterHotKey(Some(hwnd), HOTKEY_REGION, HOT_KEY_MODIFIERS(mods), vk).is_err() {
                MessageBoxW(
                    Some(hwnd),
                    w!("範囲指定ホットキーを登録できませんでした。他のアプリと衝突しています。"),
                    crate::util::display_name_pcwstr(),
                    MB_OK | MB_ICONWARNING,
                );
            }
        }
        sync_overlay(app);
    });
}

/// App の状態をオーバーレイへ反映
pub fn sync_overlay(app: &mut App) {
    let mut ocr_keys = Vec::new();
    let mut ocr_labels = Vec::new();
    let mut ocr_enabled = Vec::new();
    for (i, k) in engine::OCR_KEYS.iter().enumerate() {
        if *k != "llm" {
            ocr_keys.push(k.to_string());
            ocr_labels.push(engine::OCR_LABELS[i].to_string());
            ocr_enabled.push(app.cfg.engine_available(k));
        }
    }
    for prof in app.cfg.ready_api_profiles() {
        ocr_keys.push(prof.name.clone());
        ocr_labels.push(prof.name.clone());
        ocr_enabled.push(true);
    }
    let mut tr_keys = Vec::new();
    let mut tr_labels = Vec::new();
    let mut tr_enabled = Vec::new();
    for (i, k) in engine::TR_KEYS.iter().enumerate() {
        if *k != "llm" {
            tr_keys.push(k.to_string());
            tr_labels.push(engine::TR_LABELS[i].to_string());
            tr_enabled.push(app.cfg.engine_available(k));
        }
    }
    for prof in app.cfg.ready_api_profiles() {
        tr_keys.push(prof.name.clone());
        tr_labels.push(prof.name.clone());
        tr_enabled.push(true);
    }
    let explain_keys: Vec<String> = app.cfg.ready_api_profiles().map(|p| p.name.clone()).collect();
    let explain_labels = explain_keys.clone();
    let explain_enabled = vec![true; explain_keys.len()];
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
        source_lang: app.cfg.source_lang.clone(),
        target_lang: app.cfg.target_lang.clone(),
        tr_engine_detail: if app.cur_tr == "llm" {
            app.cfg.active_profile().map(|p| format!("{} {}", p.name, p.model_name))
        } else {
            None
        },
        explain_engine: app.explain_profile.clone(),
        explain_keys,
        explain_labels,
        explain_enabled,
        cur_explain_chip_key: app.explain_profile.clone(),
        via_uia: app.via_uia,
        ocr_keys,
        ocr_labels,
        ocr_enabled,
        cur_ocr_chip_key: if app.cur_ocr == "llm" { app.cfg.active_api_profile.clone() } else { app.cur_ocr.clone() },
        tr_keys,
        tr_labels,
        tr_enabled,
        cur_tr_chip_key: if app.cur_tr == "llm" { app.cfg.active_api_profile.clone() } else { app.cur_tr.clone() },
        explanation: app.explanation.clone(),
        explaining: app.explaining,
        error_only: app.error_only,
        app_title: app.app_title.clone(),
        uia_nodes: app.uia_nodes.clone(),
        scroll_y: app.scroll_y,
        has_image: app.last_img.is_some(),
        selected_text: app.selected_text.clone(),
        busy: app.busy,
        // overlay::update 内で EDIT (overlay.rs内) の実データから都度上書きされる
        edit: None,
        editing_block: app.editing_block,
    };
    overlay::update(app.overlay, content);
}
