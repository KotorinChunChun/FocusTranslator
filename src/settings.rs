// 設定画面 (SPEC §12)
use crate::config::Config;
use crate::util::{self, to_wide};
use crate::ui_helpers::*;
use std::cell::RefCell;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_BTNFACE, CreateFontW, DEFAULT_CHARSET,
    DEFAULT_PITCH, FONT_OUTPUT_PRECISION, FW_NORMAL, HFONT,
};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL,
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
    DestroyWindow, GetSystemMetrics, GetWindowTextLengthW,
    GetWindowTextW, IDC_ARROW, IsWindow, LoadCursorW, MB_ICONINFORMATION, MB_OK,
    MB_YESNO, MessageBoxW, PostMessageW, RegisterClassW, SM_CYSCREEN, SW_SHOW, SW_SHOWNORMAL,
    SendMessageW, SetForegroundWindow, ShowWindow, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND,
    WM_DESTROY, WM_SETFONT, WNDCLASSW, WS_CAPTION, WS_EX_TOPMOST, WS_SYSMENU,
};
use windows::core::{PCWSTR, w};

const IDC_HOLDKEY: i32 = 101;
const IDC_POLL: i32 = 102;
const IDC_HOTKEY: i32 = 103;
const IDC_OCR: i32 = 104;
const IDC_TR: i32 = 105;
const IDC_LANG: i32 = 106;
const IDC_DEEPL: i32 = 107;
const IDC_GOOGLE: i32 = 108;
const IDC_PROF_LIST: i32 = 109;
const IDC_PROF_NEW: i32 = 110;
const IDC_YOMI: i32 = 111;
const IDC_NDL: i32 = 112;
const IDC_AUTOSTART: i32 = 113;
const IDC_PERFLOG: i32 = 114;
const IDC_CONSENT_RESET: i32 = 115;
const IDC_CLOSE: i32 = 118;
const IDC_TEST_YOMI: i32 = 119;
const IDC_TEST_NDL: i32 = 120;
const IDC_PADDLE_STATUS: i32 = 121;
const IDC_PADDLE_INSTALL: i32 = 122;
const IDC_ONNX_STATUS: i32 = 123;
const IDC_ONNX_INSTALL: i32 = 124;
const IDC_DEEPL_URL: i32 = 125;
const IDC_GOOGLE_URL: i32 = 126;
const IDC_PROF_SAVE: i32 = 127;
const IDC_SRCLANG: i32 = 128;
const IDC_LOG_ENABLED: i32 = 129;
const IDC_DEBUG_MODE: i32 = 130;
const IDC_LOG_MAX: i32 = 131;
const IDC_PROF_SAVEAS: i32 = 132;
const IDC_PROF_DEL: i32 = 133;
const IDC_PROF_NAME: i32 = 134;
const IDC_OPEN_LOG: i32 = 135;
const IDC_ONNX_VARIANT: i32 = 136;
const IDC_PROF_MODEL: i32 = 137;
const IDC_PROF_URL: i32 = 138;
const IDC_PROF_KEY: i32 = 139;
const IDC_PROF_TYPE: i32 = 140;
const IDC_GLOSSARY: i32 = 141;
const IDC_DETECT_MODE: i32 = 145;
const IDC_DETECT_KEY: i32 = 146;
const IDC_PREVIEW_DETECT_MODE: i32 = 147;
const IDC_PIN_HOLD: i32 = 151;
/// プロンプト編集ウィンドウを開くボタン (SPECv0.4.7 §6.1)
const IDC_PROMPT_TR_BTN: i32 = 152;
const IDC_PROMPT_OCR_BTN: i32 = 153;
const IDC_PROMPT_EXP_BTN: i32 = 154;

/// エディットコントロールの通知コード (windows クレートに定義がないもの)
const EN_KILLFOCUS: u32 = 0x0200;
const BN_CLICKED: u32 = 0;

/// インストールスレッドからの完了通知 (settings ウィンドウ限定のメッセージ)
const WM_PADDLE_DONE: u32 = WM_APP + 10;
const WM_ONNX_DONE: u32 = WM_APP + 11;
/// 各APIキーの発行ページ(実際に確認済みの現行URL)
const DEEPL_KEY_URL: &str = "https://www.deepl.com/en/your-account/keys";
const GOOGLE_KEY_URL: &str = "https://console.cloud.google.com/apis/credentials";

const HOLD_KEYS: [&str; 5] = ["RCtrl", "LCtrl", "RShift", "RAlt", "F8"];
const OCR_KEYS: [&str; 5] = ["win", "paddle", "yomitoku", "ndl", "llm"];
const OCR_DISP: [&str; 5] = ["Windows OCR", "PaddleOCR", "YomiToku", "NDL-OCR", "LLM(プロファイル)"];
const TR_KEYS: [&str; 4] = ["local", "deepl", "google", "llm"];
const TR_DISP: [&str; 4] = ["ローカルONNX", "DeepL", "Google", "LLM(プロファイル)"];
const LANGS: [&str; 2] = ["ja", "en"];

thread_local! {
    static WND: RefCell<isize> = const { RefCell::new(0) };
    static REGISTERED: RefCell<bool> = const { RefCell::new(false) };
    static FONT: RefCell<isize> = const { RefCell::new(0) };
    static PROFILES: RefCell<Vec<crate::config::ApiProfile>> = const { RefCell::new(Vec::new()) };
    /// 「新規」ボタン直後 (まだプロファイル保存していない) 状態。
    /// プロンプト欄のUIが無くなったため、この状態での保存は既定プロンプトを使う (SPECv0.4.7 §6.1)。
    static PENDING_NEW: RefCell<bool> = const { RefCell::new(false) };
    /// ウィンドウ破棄中フラグ: 子コントロール破棄過程の EN_KILLFOCUS で
    /// 不完全なUI状態が自動保存されるのを防ぐ。
    static CLOSING: RefCell<bool> = const { RefCell::new(false) };
}

pub fn hwnd() -> HWND {
    HWND(WND.with(|w| *w.borrow()) as *mut _)
}

pub fn is_open() -> bool {
    let h = hwnd();
    !h.is_invalid() && unsafe { IsWindow(Some(h)).as_bool() }
}

pub fn open(instance: HINSTANCE, _main: HWND) {
    if is_open() {
        unsafe {
            let _ = SetForegroundWindow(hwnd());
        }
        return;
    }
    unsafe {
        let class = w!("FocusTranslatorSettings");
        REGISTERED.with(|r| {
            if !*r.borrow() {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(wndproc),
                    hInstance: instance,
                    hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                    hIcon: crate::app_state::app_icon(),
                    hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(
                        (COLOR_BTNFACE.0 + 1) as usize as *mut _,
                    ),
                    lpszClassName: class,
                    ..Default::default()
                };
                RegisterClassW(&wc);
                *r.borrow_mut() = true;
            }
        });
        // 3列レイアウト (SPECv0.4 §5): 幅1280px以下・高さ660px以下で
        // HD (1280x720) のワークエリアに収まるサイズとする。
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let (win_y, win_h) = (10, 656.min(screen_h - 40));
        if let Ok(h) = CreateWindowExW(
            WS_EX_TOPMOST,
            class,
            w!("Focus Translator 設定"),
            WS_CAPTION | WS_SYSMENU,
            CW_USEDEFAULT,
            win_y,
            1280,
            win_h,
            None,
            None,
            Some(instance),
            None,
        ) {
            WND.with(|w| *w.borrow_mut() = h.0 as isize);
            build_controls(h, instance);
            populate(h);
            let _ = ShowWindow(h, SW_SHOW);
            let _ = SetForegroundWindow(h);
        }
    }
}

/// BS_GROUPBOX でカテゴリ枠を作る (SPECv0.4 §5.1)
fn group(h: HWND, inst: HINSTANCE, text: &str, x: i32, y: i32, w: i32, ht: i32) {
    const BS_GROUPBOX: u32 = 0x0000_0007;
    ctl(h, inst, w!("BUTTON"), text, WINDOW_STYLE(BS_GROUPBOX), x, y, w, ht, 0);
}

fn build_controls(h: HWND, inst: HINSTANCE) {
    // 3列レイアウト (SPECv0.4 §5.1): 左=操作/OCR、中=翻訳/その他、右=LLMプロファイル
    const PAD: i32 = 10;
    const COL_W: i32 = 408;
    const STEP: i32 = 30;
    const GTOP: i32 = 22; // グループ枠タイトル分のオフセット
    let col_x = [PAD, PAD * 2 + COL_W, PAD * 3 + COL_W * 2];
    let inner = |cx: i32| cx + 12; // グループ内の左端
    let key_w = 160;

    // ---- 左列 グループ1: 操作 ----
    {
        let gx = col_x[0];
        let lx = inner(gx);
        let cx = gx + 152;
        group(h, inst, "1. 操作", gx, 8, COL_W, 184);
        let mut y = 8 + GTOP;
        label(h, inst, "キャプチャキー", lx, y + 2, 130);
        combo(h, inst, cx, y, 90, IDC_HOLDKEY);
        checkbox(h, inst, "領域表示", cx + 98, y + 2, 88, IDC_DETECT_MODE);
        y += STEP;
        label(h, inst, "プレビューキー", lx, y + 2, 130);
        combo(h, inst, cx, y, 90, IDC_DETECT_KEY);
        checkbox(h, inst, "領域表示", cx + 98, y + 2, 88, IDC_PREVIEW_DETECT_MODE);
        y += STEP;
        label(h, inst, "範囲指定ホットキー", lx, y + 2, 130);
        edit(h, inst, cx, y, 120, IDC_HOTKEY);
        y += STEP;
        label(h, inst, "監視周期 (ms)", lx, y + 2, 130);
        edit(h, inst, cx, y, 60, IDC_POLL);
        y += STEP;
        label(h, inst, "ピン留め長押し時間 (秒)", lx, y + 2, 130);
        edit(h, inst, cx, y, 60, IDC_PIN_HOLD);
    }

    // ---- 左列 グループ2: OCR設定 ----
    {
        let gx = col_x[0];
        let lx = inner(gx);
        let cx = gx + 152;
        group(h, inst, "2. OCR設定", gx, 200, COL_W, 176);
        let mut y = 200 + GTOP;
        label(h, inst, "既定OCRエンジン", lx, y + 2, 130);
        combo(h, inst, cx, y, 170, IDC_OCR);
        y += STEP;
        label(h, inst, "PaddleOCR", lx, y + 2, 130);
        ctl(h, inst, w!("STATIC"), "確認中…", WINDOW_STYLE(0), cx, y + 2, 100, 20, IDC_PADDLE_STATUS);
        button(h, inst, "インストール", cx + 106, y - 2, 104, IDC_PADDLE_INSTALL);
        y += STEP;
        label(h, inst, "YomiToku サーバーURL", lx, y + 2, 140);
        edit(h, inst, cx, y, 176, IDC_YOMI);
        button(h, inst, "テスト", cx + 182, y - 2, 54, IDC_TEST_YOMI);
        y += STEP;
        label(h, inst, "NDL-OCR サーバーURL", lx, y + 2, 140);
        edit(h, inst, cx, y, 176, IDC_NDL);
        button(h, inst, "テスト", cx + 182, y - 2, 54, IDC_TEST_NDL);
    }

    // ---- 中列 グループ3: 翻訳設定 ----
    {
        let gx = col_x[1];
        let lx = inner(gx);
        let cx = gx + 152;
        group(h, inst, "3. 翻訳設定", gx, 8, COL_W, 330);
        let mut y = 8 + GTOP;
        label(h, inst, "既定翻訳エンジン", lx, y + 2, 130);
        combo(h, inst, cx, y, 170, IDC_TR);
        y += STEP;
        label(h, inst, "ローカルONNXモデル", lx, y + 2, 130);
        combo(h, inst, cx, y, 226, IDC_ONNX_VARIANT);
        y += STEP;
        ctl(h, inst, w!("STATIC"), "確認中…", WINDOW_STYLE(0), cx, y + 2, 100, 20, IDC_ONNX_STATUS);
        button(h, inst, "インストール", cx + 106, y - 2, 104, IDC_ONNX_INSTALL);
        y += STEP;
        label(h, inst, "翻訳元言語 / 訳先言語", lx, y + 2, 140);
        combo(h, inst, cx, y, 70, IDC_SRCLANG);
        label(h, inst, "→", cx + 76, y + 2, 16);
        combo(h, inst, cx + 94, y, 70, IDC_LANG);
        y += STEP;
        label(h, inst, "DeepL APIキー", lx, y + 2, 130);
        password_edit(h, inst, cx, y, key_w, IDC_DEEPL);
        button(h, inst, "取得ページ", cx + key_w + 6, y - 2, 76, IDC_DEEPL_URL);
        y += STEP;
        label(h, inst, "Google Trans APIキー", lx, y + 2, 140);
        password_edit(h, inst, cx, y, key_w, IDC_GOOGLE);
        button(h, inst, "取得ページ", cx + key_w + 6, y - 2, 76, IDC_GOOGLE_URL);
        y += STEP;
        label(h, inst, "用語集 (1行に 原文=訳文)", lx, y + 2, 170);
        multiline(h, inst, cx, y, 226, 108, IDC_GLOSSARY);
    }

    // ---- 中列 グループ5: その他の設定 ----
    {
        let gx = col_x[1];
        let lx = inner(gx);
        group(h, inst, "5. その他の設定", gx, 346, COL_W, 186);
        let mut y = 346 + GTOP;
        checkbox(h, inst, "起動時に常駐する", lx, y, 170, IDC_AUTOSTART);
        checkbox(h, inst, "計測ログを有効化", lx + 180, y, 160, IDC_PERFLOG);
        y += STEP;
        checkbox(h, inst, "実行ログを記録 (原文/訳文を平文保存)", lx, y, 300, IDC_LOG_ENABLED);
        y += STEP;
        checkbox(h, inst, "デバッグモード (OCR画像をPNG保存)", lx, y, 280, IDC_DEBUG_MODE);
        y += STEP;
        label(h, inst, "保持上限", lx, y + 2, 60);
        edit(h, inst, lx + 66, y, 70, IDC_LOG_MAX);
        button(h, inst, "ログビューアを開く", lx + 150, y - 2, 130, IDC_OPEN_LOG);
        y += STEP;
        button(h, inst, "外部送信の同意状態をリセット", lx, y, 220, IDC_CONSENT_RESET);
    }

    // ---- 右列 グループ4: LLMプロファイル設定 ----
    {
        let gx = col_x[2];
        let lx = inner(gx);
        let cx = gx + 100;
        group(h, inst, "4. LLMプロファイル設定", gx, 8, COL_W, 240);
        let mut y = 8 + GTOP;
        combo(h, inst, lx, y, 150, IDC_PROF_LIST);
        button(h, inst, "新規", lx + 156, y, 46, IDC_PROF_NEW);
        button(h, inst, "保存", lx + 206, y, 46, IDC_PROF_SAVE);
        button(h, inst, "別名保存", lx + 256, y, 66, IDC_PROF_SAVEAS);
        button(h, inst, "削除", lx + 326, y, 46, IDC_PROF_DEL);
        y += STEP;
        label(h, inst, "API登録名", lx, y + 2, 84);
        edit(h, inst, cx, y, 140, IDC_PROF_NAME);
        label(h, inst, "種別", cx + 150, y + 2, 36);
        combo(h, inst, cx + 188, y, 100, IDC_PROF_TYPE);
        y += STEP;
        label(h, inst, "API URL", lx, y + 2, 84);
        edit(h, inst, cx, y, 288, IDC_PROF_URL);
        y += STEP;
        label(h, inst, "APIキー", lx, y + 2, 84);
        password_edit(h, inst, cx, y, key_w, IDC_PROF_KEY);
        y += STEP;
        label(h, inst, "モデル名", lx, y + 2, 84);
        edit(h, inst, cx, y, 180, IDC_PROF_MODEL);
        y += STEP;
        // プロンプトは専用の編集ウィンドウで編集する (SPECv0.4.7 §1)
        label(h, inst, "プロンプト編集", lx, y + 4, 84);
        button(h, inst, "翻訳プロンプト", cx, y, 92, IDC_PROMPT_TR_BTN);
        button(h, inst, "OCRプロンプト", cx + 98, y, 92, IDC_PROMPT_OCR_BTN);
        button(h, inst, "解説プロンプト", cx + 196, y, 92, IDC_PROMPT_EXP_BTN);
    }

    // ---- 下部ボタン領域 (右下; SPECv0.4 §5.2)
    // 設定は変更時に即座に保存されるため【閉じる】のみ (SPECv0.4.7 改)
    let btn_y = 570;
    let right = PAD * 3 + COL_W * 3;
    button(h, inst, "閉じる", right - 86, btn_y, 80, IDC_CLOSE);

    // フォント設定
    unsafe {
        let font: HFONT = CreateFontW(
            -13,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            FONT_OUTPUT_PRECISION(0),
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0.into(),
            w!("Yu Gothic UI"),
        );
        FONT.with(|f| *f.borrow_mut() = font.0 as isize);
        let _ = windows::Win32::UI::WindowsAndMessaging::EnumChildWindows(
            Some(h),
            Some(set_font_proc),
            LPARAM(font.0 as isize),
        );
    }
}

unsafe extern "system" fn set_font_proc(child: HWND, lparam: LPARAM) -> windows::core::BOOL {
    unsafe {
        SendMessageW(child, WM_SETFONT, Some(WPARAM(lparam.0 as usize)), Some(LPARAM(1)));
    }
    true.into()
}

fn get_dlg_item(h: HWND, id: i32) -> HWND {
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetDlgItem(Some(h), id).unwrap_or_default()
    }
}

fn set_text(h: HWND, id: i32, text: &str) {
    unsafe {
        let wide = to_wide(text);
        let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
            get_dlg_item(h, id),
            PCWSTR(wide.as_ptr()),
        );
    }
}

fn get_text(h: HWND, id: i32) -> String {
    unsafe {
        let ctl = get_dlg_item(h, id);
        let len = GetWindowTextLengthW(ctl);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        let n = GetWindowTextW(ctl, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

fn combo_fill(h: HWND, id: i32, items: &[&str], selected: usize) {
    unsafe {
        let ctl = get_dlg_item(h, id);
        for item in items {
            let wide = to_wide(item);
            SendMessageW(
                ctl,
                CB_ADDSTRING,
                Some(WPARAM(0)),
                Some(LPARAM(wide.as_ptr() as isize)),
            );
        }
        SendMessageW(ctl, CB_SETCURSEL, Some(WPARAM(selected)), Some(LPARAM(0)));
    }
}

fn combo_sel(h: HWND, id: i32) -> usize {
    unsafe {
        let r = SendMessageW(get_dlg_item(h, id), CB_GETCURSEL, Some(WPARAM(0)), Some(LPARAM(0)));
        if r.0 < 0 { 0 } else { r.0 as usize }
    }
}

fn check_set(h: HWND, id: i32, checked: bool) {
    unsafe {
        SendMessageW(
            get_dlg_item(h, id),
            BM_SETCHECK,
            Some(WPARAM(if checked { 1 } else { 0 })),
            Some(LPARAM(0)),
        );
    }
}

fn check_get(h: HWND, id: i32) -> bool {
    unsafe {
        SendMessageW(get_dlg_item(h, id), BM_GETCHECK, Some(WPARAM(0)), Some(LPARAM(0))).0 == 1
    }
}

fn populate(h: HWND) {
    let cfg = Config::load();
    combo_fill(
        h,
        IDC_HOLDKEY,
        &HOLD_KEYS,
        HOLD_KEYS.iter().position(|k| *k == cfg.hold_key).unwrap_or(0),
    );
    set_text(h, IDC_POLL, &cfg.poll_ms.to_string());
    set_text(h, IDC_PIN_HOLD, &cfg.pin_hold_seconds.to_string());
    set_text(h, IDC_HOTKEY, &cfg.region_hotkey);
    combo_fill(
        h,
        IDC_OCR,
        &OCR_DISP,
        OCR_KEYS.iter().position(|k| *k == cfg.default_ocr).unwrap_or(0),
    );
    combo_fill(
        h,
        IDC_TR,
        &TR_DISP,
        TR_KEYS.iter().position(|k| *k == cfg.default_translator).unwrap_or(0),
    );
    combo_fill(h, IDC_SRCLANG, &LANGS, LANGS.iter().position(|k| *k == cfg.source_lang).unwrap_or(1));
    combo_fill(h, IDC_LANG, &LANGS, LANGS.iter().position(|k| *k == cfg.target_lang).unwrap_or(0));
    let onnx_disp: Vec<&str> = crate::onnx_translate_install::Variant::ALL.iter().map(|v| v.display()).collect();
    combo_fill(
        h,
        IDC_ONNX_VARIANT,
        &onnx_disp,
        crate::onnx_translate_install::Variant::ALL
            .iter()
            .position(|v| v.key() == cfg.local_model_variant)
            .unwrap_or(0),
    );
    set_text(h, IDC_DEEPL, &cfg.deepl_key());
    set_text(h, IDC_GOOGLE, &cfg.google_key());

    PROFILES.with(|p| *p.borrow_mut() = cfg.api_profiles.clone());
    let sel = cfg.api_profiles.iter().position(|p| p.name == cfg.active_api_profile).unwrap_or(0);
    refill_profile_combo(h, sel);

    combo_reset(h, IDC_PROF_TYPE);
    combo_fill(h, IDC_PROF_TYPE, &API_TYPE_DISP, 0);

    load_profile_to_ui(h, sel);
    set_text(h, IDC_YOMI, &cfg.yomitoku_url);
    set_text(h, IDC_NDL, &cfg.ndl_url);
    check_set(h, IDC_AUTOSTART, cfg.autostart);
    check_set(h, IDC_PERFLOG, cfg.perf_log);
    check_set(h, IDC_LOG_ENABLED, cfg.log_enabled);
    check_set(h, IDC_DEBUG_MODE, cfg.debug_mode);
    check_set(h, IDC_DETECT_MODE, cfg.detect_enabled);
    combo_fill(
        h,
        IDC_DETECT_KEY,
        &HOLD_KEYS,
        HOLD_KEYS.iter().position(|k| *k == cfg.detect_key).unwrap_or(1), // 既定 LCtrl
    );
    check_set(h, IDC_PREVIEW_DETECT_MODE, cfg.preview_detect_enabled);
    set_text(h, IDC_LOG_MAX, &cfg.log_max_records.to_string());
    let glossary_text = cfg.glossary.iter().map(|e| format!("{}={}", e.source, e.target)).collect::<Vec<_>>().join("\r\n");
    set_text(h, IDC_GLOSSARY, &glossary_text);
    refresh_paddle_status(h);
    refresh_onnx_status(h);
}

/// PaddleOCRの導入状況をステータス欄・ボタンに反映する
fn refresh_paddle_status(h: HWND) {
    let installed = crate::paddle_install::installed();
    set_text(h, IDC_PADDLE_STATUS, if installed { "導入済み" } else { "未導入" });
    unsafe {
        let _ = EnableWindow(get_dlg_item(h, IDC_PADDLE_INSTALL), !installed);
    }
}

/// 設定画面で現在選択中のローカル翻訳モデル種別
fn selected_onnx_variant(h: HWND) -> crate::onnx_translate_install::Variant {
    let all = crate::onnx_translate_install::Variant::ALL;
    all[combo_sel(h, IDC_ONNX_VARIANT).min(all.len() - 1)]
}

/// ローカルONNX翻訳モデル(選択中の種別)の導入状況をステータス欄・ボタンに反映する
fn refresh_onnx_status(h: HWND) {
    let installed = crate::onnx_translate_install::installed(selected_onnx_variant(h));
    set_text(h, IDC_ONNX_STATUS, if installed { "導入済み" } else { "未導入" });
    unsafe {
        let _ = EnableWindow(get_dlg_item(h, IDC_ONNX_INSTALL), !installed);
    }
}

/// APIプロファイル種別のコンボ表示順 (IDC_PROF_TYPE の選択indexと対応)
const API_TYPE_ORDER: [crate::config::ApiType; 3] = [
    crate::config::ApiType::Gemini,
    crate::config::ApiType::OpenAI,
    crate::config::ApiType::Claude,
];
const API_TYPE_DISP: [&str; 3] = ["Gemini", "OpenAI", "Claude"];

fn api_type_index(t: &crate::config::ApiType) -> usize {
    API_TYPE_ORDER.iter().position(|x| x == t).unwrap_or(0)
}

/// コンボの内容を全消去する
fn combo_reset(h: HWND, id: i32) {
    unsafe {
        SendMessageW(get_dlg_item(h, id), windows::Win32::UI::WindowsAndMessaging::CB_RESETCONTENT, None, None);
    }
}

fn combo_select(h: HWND, id: i32, idx: usize) {
    unsafe {
        SendMessageW(
            get_dlg_item(h, id),
            windows::Win32::UI::WindowsAndMessaging::CB_SETCURSEL,
            Some(WPARAM(idx)),
            Some(LPARAM(0)),
        );
    }
}

/// PROFILES の内容でプロファイル一覧コンボを再構築する
fn refill_profile_combo(h: HWND, sel: usize) {
    let names: Vec<String> = PROFILES.with(|p| p.borrow().iter().map(|x| x.name.clone()).collect());
    let strs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    combo_reset(h, IDC_PROF_LIST);
    combo_fill(h, IDC_PROF_LIST, &strs, sel);
}

fn load_profile_to_ui(h: HWND, idx: usize) {
    PENDING_NEW.with(|f| *f.borrow_mut() = false);
    PROFILES.with(|p| {
        let profiles = p.borrow();
        if let Some(prof) = profiles.get(idx) {
            set_text(h, IDC_PROF_NAME, &prof.name);
            combo_select(h, IDC_PROF_TYPE, api_type_index(&prof.api_type));
            set_text(h, IDC_PROF_MODEL, &prof.model_name);
            set_text(h, IDC_PROF_URL, &prof.api_url);
            set_text(h, IDC_PROF_KEY, &prof.get_key());
        }
    });
}

/// インストールボタン押下時の共通処理: ボタン無効化→バックグラウンドDL→完了時に done_msg を通知
fn start_install(
    h: HWND,
    status_id: i32,
    button_id: i32,
    done_msg: u32,
    install_fn: impl FnOnce() -> Result<(), String> + Send + 'static,
) {
    unsafe {
        let _ = EnableWindow(get_dlg_item(h, button_id), false);
    }
    set_text(h, status_id, "ダウンロード中…");
    let hwnd_isize = h.0 as isize;
    std::thread::spawn(move || {
        let result = install_fn();
        let (w, l) = match result {
            Ok(()) => (1usize, 0isize),
            Err(e) => (0usize, Box::into_raw(Box::new(e)) as isize),
        };
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd_isize as *mut _)), done_msg, WPARAM(w), LPARAM(l));
        }
    });
}

/// インストール完了通知 (WM_PADDLE_DONE / WM_ONNX_DONE) の共通処理
fn handle_install_done(h: HWND, wparam: WPARAM, lparam: LPARAM, refresh: fn(HWND), success_msg: &str) {
    if wparam.0 == 1 {
        refresh(h);
        unsafe {
            let wide = to_wide(success_msg);
            MessageBoxW(
                Some(h),
                PCWSTR(wide.as_ptr()),
                w!("Focus Translator"),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    } else {
        let msg = unsafe { *Box::from_raw(lparam.0 as *mut String) };
        refresh(h);
        unsafe {
            let wide = to_wide(&msg);
            MessageBoxW(Some(h), PCWSTR(wide.as_ptr()), w!("インストールエラー"), MB_OK);
        }
    }
}

/// プロンプト編集ウィンドウの保存からの同期 (SPECv0.4.7 §4.3):
/// 設定画面が開いていればメモリ上 PROFILES の該当プロファイルの該当プロンプトを更新する。
/// 他フィールドの未保存編集 (UI上の入力) には触れない。
pub fn update_prompt_in_memory(name: &str, kind: crate::prompt_edit::PromptKind, text: &str) {
    if !is_open() {
        return;
    }
    PROFILES.with(|p| {
        if let Some(prof) = p.borrow_mut().iter_mut().find(|x| x.name == name) {
            match kind {
                crate::prompt_edit::PromptKind::Translate => prof.translate_prompt = text.to_string(),
                crate::prompt_edit::PromptKind::Ocr => prof.ocr_prompt = text.to_string(),
                crate::prompt_edit::PromptKind::Explain => prof.explain_prompt = text.to_string(),
            }
        }
    });
}

/// 設定の即時保存 (SPECv0.4.7 改): 変更をディスクへ保存し main へ再読込を通知する
fn auto_save(h: HWND, ask_consent: bool) {
    if CLOSING.with(|f| *f.borrow()) {
        return;
    }
    save(h, ask_consent);
    unsafe {
        let _ = PostMessageW(
            Some(crate::app_state::main_hwnd()),
            crate::app_state::WM_APP_CFG,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

/// プロファイル編集UI (名前/種別/URL/キー/モデル) が PROFILES の保存済み内容と異なるか
fn profile_ui_dirty(h: HWND) -> bool {
    if PENDING_NEW.with(|f| *f.borrow()) {
        return true;
    }
    let name = get_text(h, IDC_PROF_NAME).trim().to_string();
    PROFILES.with(|p| {
        let profiles = p.borrow();
        let Some(prof) = profiles.iter().find(|x| x.name == name) else {
            return true;
        };
        prof.api_type != API_TYPE_ORDER[combo_sel(h, IDC_PROF_TYPE).min(API_TYPE_ORDER.len() - 1)]
            || prof.model_name != get_text(h, IDC_PROF_MODEL).trim()
            || prof.api_url != get_text(h, IDC_PROF_URL).trim()
            || prof.get_key() != get_text(h, IDC_PROF_KEY).trim()
    })
}

/// プロファイル編集UIの内容で PROFILES を更新する (保存/別名保存の共通処理)。
/// プロンプトはUIに無いため、新規なら既定値、既存なら保存済みの値を引き継ぐ (SPECv0.4.7 §6.1)。
/// 成功時は該当プロファイルのindexを返し、コンボを再構築して設定も即保存する。
fn save_profile_from_ui(h: HWND, save_as: bool) -> Option<usize> {
    let name = get_text(h, IDC_PROF_NAME).trim().to_string();
    if name.is_empty() {
        unsafe {
            MessageBoxW(Some(h), w!("API登録名を入力してください"), w!("エラー"), MB_OK);
        }
        return None;
    }
    // プロンプトの引き継ぎ元: 新規=既定値 / 同名の既存=その値 / 別名複製=選択中プロファイルの値
    let (ocr_p, tr_p, exp_p) = PROFILES.with(|p| {
        let profiles = p.borrow();
        let src = if PENDING_NEW.with(|f| *f.borrow()) {
            None
        } else {
            profiles
                .iter()
                .find(|x| x.name == name)
                .or_else(|| profiles.get(combo_sel(h, IDC_PROF_LIST)))
        };
        match src {
            Some(s) => (s.ocr_prompt.clone(), s.translate_prompt.clone(), s.explain_prompt.clone()),
            None => (
                crate::config::DEFAULT_GEMINI_OCR_PROMPT.to_string(),
                crate::config::DEFAULT_GEMINI_TRANSLATE_PROMPT.to_string(),
                crate::config::DEFAULT_GEMINI_EXPLAIN_PROMPT.to_string(),
            ),
        }
    });
    let mut prof = crate::config::ApiProfile {
        name: name.clone(),
        api_type: API_TYPE_ORDER[combo_sel(h, IDC_PROF_TYPE).min(API_TYPE_ORDER.len() - 1)].clone(),
        model_name: get_text(h, IDC_PROF_MODEL).trim().to_string(),
        api_url: get_text(h, IDC_PROF_URL).trim().to_string(),
        api_key_enc: String::new(),
        ocr_prompt: ocr_p,
        translate_prompt: tr_p,
        explain_prompt: exp_p,
    };
    prof.set_key(get_text(h, IDC_PROF_KEY).trim());

    let saved = PROFILES.with(|p| {
        let mut profiles = p.borrow_mut();
        if !save_as {
            if let Some(existing) = profiles.iter_mut().find(|x| x.name == name) {
                *existing = prof.clone();
            } else {
                profiles.push(prof.clone());
            }
        } else {
            // 別名保存: 名前重複は拒否
            if profiles.iter().any(|x| x.name == name) {
                unsafe {
                    MessageBoxW(Some(h), w!("その名前は既に存在します"), w!("エラー"), MB_OK);
                }
                return None;
            }
            profiles.push(prof.clone());
        }
        profiles.iter().position(|p| p.name == name)
    })?;
    PENDING_NEW.with(|f| *f.borrow_mut() = false);
    refill_profile_combo(h, saved);
    auto_save(h, false);
    Some(saved)
}

/// プロンプト編集ボタン (SPECv0.4.7 §6.1): プロファイルが未保存なら保存確認→保存後に
/// プロンプト編集ウィンドウ (モードA) を開く。
fn open_prompt_editor(h: HWND, kind: crate::prompt_edit::PromptKind) {
    if profile_ui_dirty(h) {
        let r = unsafe {
            MessageBoxW(
                Some(h),
                w!("プロファイルが保存されていません。保存してからプロンプト編集を開きますか?"),
                w!("Focus Translator"),
                MB_YESNO,
            )
        };
        if r.0 != 6 {
            // IDYES 以外は開かない
            return;
        }
        if save_profile_from_ui(h, false).is_none() {
            return;
        }
    }
    let name = get_text(h, IDC_PROF_NAME).trim().to_string();
    let (profiles, active_idx) = PROFILES.with(|p| {
        let profiles = p.borrow();
        let list: Vec<crate::prompt_edit::ProfilePrompt> = profiles
            .iter()
            .map(|x| crate::prompt_edit::ProfilePrompt {
                name: x.name.clone(),
                template: match kind {
                    crate::prompt_edit::PromptKind::Translate => x.translate_prompt.clone(),
                    crate::prompt_edit::PromptKind::Ocr => x.ocr_prompt.clone(),
                    crate::prompt_edit::PromptKind::Explain => x.explain_prompt.clone(),
                },
            })
            .collect();
        let idx = profiles.iter().position(|x| x.name == name).unwrap_or(0);
        (list, idx)
    });
    if profiles.is_empty() {
        return;
    }
    // 設定画面の近傍に表示する
    let pos = unsafe {
        let mut r = windows::Win32::Foundation::RECT::default();
        let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(h, &mut r);
        Some((r.left + 60, r.top + 60))
    };
    let inst = unsafe {
        HINSTANCE(
            windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                .map(|m| m.0)
                .unwrap_or(std::ptr::null_mut()),
        )
    };
    crate::prompt_edit::open(
        inst,
        h,
        pos,
        kind,
        profiles,
        active_idx,
        None,
        Box::new(move |n, t| crate::prompt_edit::save_prompt_to_config(kind, n, t)),
        None,
    );
}

/// 既定ブラウザで指定URLを開く
fn open_url(h: HWND, url: &str) {
    unsafe {
        let wide_op = to_wide("open");
        let wide_url = to_wide(url);
        let _ = ShellExecuteW(
            Some(h),
            PCWSTR(wide_op.as_ptr()),
            PCWSTR(wide_url.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

fn save(h: HWND, ask_consent: bool) {
    let mut cfg = Config::load();
    cfg.hold_key = HOLD_KEYS[combo_sel(h, IDC_HOLDKEY).min(HOLD_KEYS.len() - 1)].to_string();
    cfg.poll_ms = get_text(h, IDC_POLL).trim().parse().unwrap_or(100).clamp(20, 1000);
    cfg.pin_hold_seconds = get_text(h, IDC_PIN_HOLD).trim().parse().unwrap_or(3);
    let hk = get_text(h, IDC_HOTKEY);
    if crate::config::parse_hotkey(&hk).is_some() {
        cfg.region_hotkey = hk.trim().to_string();
    }
    cfg.default_ocr = OCR_KEYS[combo_sel(h, IDC_OCR).min(OCR_KEYS.len() - 1)].to_string();
    cfg.default_translator = TR_KEYS[combo_sel(h, IDC_TR).min(TR_KEYS.len() - 1)].to_string();
    cfg.source_lang = LANGS[combo_sel(h, IDC_SRCLANG).min(LANGS.len() - 1)].to_string();
    cfg.target_lang = LANGS[combo_sel(h, IDC_LANG).min(LANGS.len() - 1)].to_string();
    cfg.local_model_variant = selected_onnx_variant(h).key().to_string();
    cfg.deepl_key_enc = util::dpapi_encrypt(get_text(h, IDC_DEEPL).trim());
    cfg.google_key_enc = util::dpapi_encrypt(get_text(h, IDC_GOOGLE).trim());
    
    PROFILES.with(|p| {
        cfg.api_profiles = p.borrow().clone();
    });
    let sel = combo_sel(h, IDC_PROF_LIST);
    if let Some(prof) = cfg.api_profiles.get(sel) {
        cfg.active_api_profile = prof.name.clone();
    }
    
    cfg.yomitoku_url = get_text(h, IDC_YOMI).trim().to_string();
    cfg.ndl_url = get_text(h, IDC_NDL).trim().to_string();
    cfg.autostart = check_get(h, IDC_AUTOSTART);
    cfg.perf_log = check_get(h, IDC_PERFLOG);
    cfg.log_enabled = check_get(h, IDC_LOG_ENABLED);
    cfg.debug_mode = check_get(h, IDC_DEBUG_MODE);
    cfg.detect_enabled = check_get(h, IDC_DETECT_MODE);
    cfg.detect_key = HOLD_KEYS[combo_sel(h, IDC_DETECT_KEY).min(HOLD_KEYS.len() - 1)].to_string();
    cfg.preview_detect_enabled = check_get(h, IDC_PREVIEW_DETECT_MODE);
    cfg.log_max_records = get_text(h, IDC_LOG_MAX).trim().parse().unwrap_or(5000).clamp(100, 100000);
    
    let glos_text = get_text(h, IDC_GLOSSARY);
    cfg.glossary = glos_text.lines().filter_map(|line| {
        let parts: Vec<&str> = line.splitn(2, '=').collect();
        if parts.len() == 2 {
            let s = parts[0].trim();
            let t = parts[1].trim();
            if !s.is_empty() && !t.is_empty() {
                return Some(crate::config::GlossaryEntry { source: s.to_string(), target: t.to_string() });
            }
        }
        None
    }).collect();

    // 既定エンジンがクラウド/外部送信を伴う場合の同意確認 (SPEC §9)。
    // 即時保存化に伴い、既定エンジンのコンボを変更したときだけ確認する
    // (毎回の自動保存でダイアログを出さないため)。
    if ask_consent {
        confirm_default_consents(h, &mut cfg);
    }

    cfg.save();
    apply_autostart(cfg.autostart);
}

fn confirm_default_consents(h: HWND, cfg: &mut Config) {
    unsafe {
        if matches!(cfg.default_translator.as_str(), "deepl" | "google" | "llm")
            && !cfg.consent_text
        {
            let r = MessageBoxW(
                Some(h),
                w!("既定の翻訳エンジンはOCR済みテキストを外部サービスへ送信します。許可しますか?"),
                w!("外部送信の同意"),
                MB_YESNO,
            );
            cfg.consent_text = r.0 == 6; // IDYES
        }
        if cfg.default_ocr == "llm" && !cfg.consent_image {
            let r = MessageBoxW(
                Some(h),
                w!("既定のOCRエンジンはキャプチャ画像を外部サービスへ送信します。許可しますか?"),
                w!("外部送信の同意"),
                MB_YESNO,
            );
            cfg.consent_image = r.0 == 6;
        }
        if matches!(cfg.default_ocr.as_str(), "yomitoku" | "ndl") && !cfg.consent_ext_ocr {
            let url = if cfg.default_ocr == "yomitoku" { &cfg.yomitoku_url } else { &cfg.ndl_url };
            // 127.0.0.1 はローカル送信として同意不要 (SPEC §9.2)
            if !is_localhost(url) {
                let r = MessageBoxW(
                    Some(h),
                    w!("既定のOCRエンジンは画像を外部OCRサーバーへ送信します。許可しますか?"),
                    w!("外部送信の同意"),
                    MB_YESNO,
                );
                cfg.consent_ext_ocr = r.0 == 6;
            } else {
                cfg.consent_ext_ocr = true;
            }
        }
    }
}

pub fn is_localhost(url: &str) -> bool {
    let u = url.trim().trim_start_matches("http://").trim_start_matches("https://");
    u.starts_with("127.0.0.1") || u.starts_with("localhost")
}

fn apply_autostart(enable: bool) {
    unsafe {
        let key = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        let name = w!("FocusTranslator");
        if enable {
            if let Ok(exe) = std::env::current_exe() {
                let wide = to_wide(&exe.to_string_lossy());
                let _ = RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key,
                    name,
                    REG_SZ.0,
                    Some(wide.as_ptr() as *const _),
                    (wide.len() * 2) as u32,
                );
            }
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, key, name);
        }
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let notif = ((wparam.0 >> 16) & 0xFFFF) as u32;
            // 設定の即時保存 (SPECv0.4.7 改): コンボは選択変更時、チェックボックスは
            // クリック時、エディットはフォーカス喪失時に自動保存する。
            // プロファイル編集欄 (名前/種別/URL/キー/モデル) は【保存】ボタンで確定するため対象外。
            match id {
                IDC_HOLDKEY | IDC_DETECT_KEY | IDC_SRCLANG | IDC_LANG
                    if notif == windows::Win32::UI::WindowsAndMessaging::CBN_SELCHANGE =>
                {
                    auto_save(h, false);
                }
                IDC_OCR | IDC_TR
                    if notif == windows::Win32::UI::WindowsAndMessaging::CBN_SELCHANGE =>
                {
                    // 既定エンジンの変更は外部送信の同意確認を伴う
                    auto_save(h, true);
                }
                IDC_AUTOSTART | IDC_PERFLOG | IDC_LOG_ENABLED | IDC_DEBUG_MODE | IDC_DETECT_MODE
                | IDC_PREVIEW_DETECT_MODE
                    if notif == BN_CLICKED =>
                {
                    auto_save(h, false);
                }
                IDC_POLL | IDC_PIN_HOLD | IDC_HOTKEY | IDC_DEEPL | IDC_GOOGLE | IDC_YOMI
                | IDC_NDL | IDC_LOG_MAX | IDC_GLOSSARY
                    if notif == EN_KILLFOCUS =>
                {
                    auto_save(h, false);
                }
                _ => {}
            }
            match id {
                IDC_CLOSE => unsafe {
                    // WM_CLOSE 経由でモードAプロンプト編集ウィンドウの連動クローズ処理を通す
                    let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
                },
                IDC_CONSENT_RESET => {
                    let mut cfg = Config::load();
                    cfg.consent_text = false;
                    cfg.consent_image = false;
                    cfg.consent_ext_ocr = false;
                    cfg.save();
                    unsafe {
                        let _ = PostMessageW(
                            Some(crate::app_state::main_hwnd()),
                            crate::app_state::WM_APP_CFG,
                            WPARAM(0),
                            LPARAM(0),
                        );
                        MessageBoxW(
                            Some(h),
                            w!("外部送信の同意状態をリセットしました。"),
                            w!("Focus Translator"),
                            MB_OK | MB_ICONINFORMATION,
                        );
                    }
                }
                IDC_PADDLE_INSTALL => {
                    start_install(
                        h,
                        IDC_PADDLE_STATUS,
                        IDC_PADDLE_INSTALL,
                        WM_PADDLE_DONE,
                        crate::paddle_install::install,
                    );
                }
                IDC_ONNX_INSTALL => {
                    let variant = selected_onnx_variant(h);
                    start_install(
                        h,
                        IDC_ONNX_STATUS,
                        IDC_ONNX_INSTALL,
                        WM_ONNX_DONE,
                        move || crate::onnx_translate_install::install_variant(variant),
                    );
                }
                IDC_ONNX_VARIANT => {
                    if notif == windows::Win32::UI::WindowsAndMessaging::CBN_SELCHANGE {
                        refresh_onnx_status(h);
                        auto_save(h, false);
                    }
                }
                IDC_DEEPL_URL => open_url(h, DEEPL_KEY_URL),
                IDC_GOOGLE_URL => open_url(h, GOOGLE_KEY_URL),
                IDC_PROMPT_TR_BTN => open_prompt_editor(h, crate::prompt_edit::PromptKind::Translate),
                IDC_PROMPT_OCR_BTN => open_prompt_editor(h, crate::prompt_edit::PromptKind::Ocr),
                IDC_PROMPT_EXP_BTN => open_prompt_editor(h, crate::prompt_edit::PromptKind::Explain),
                IDC_PROF_LIST | IDC_PROF_TYPE => {
                    if notif == windows::Win32::UI::WindowsAndMessaging::CBN_SELCHANGE {
                        if id == IDC_PROF_LIST {
                            load_profile_to_ui(h, combo_sel(h, IDC_PROF_LIST));
                            // アクティブプロファイルの変更を即保存する
                            auto_save(h, false);
                        } else {
                            // 種別切替: モデル名・URLをその種別の既定値に置き換える
                            let t = &API_TYPE_ORDER[combo_sel(h, IDC_PROF_TYPE).min(API_TYPE_ORDER.len() - 1)];
                            set_text(h, IDC_PROF_MODEL, t.default_model());
                            set_text(h, IDC_PROF_URL, t.default_url());
                        }
                    }
                }
                IDC_PROF_NEW => {
                    set_text(h, IDC_PROF_NAME, "");
                    set_text(h, IDC_PROF_URL, "");
                    set_text(h, IDC_PROF_KEY, "");
                    set_text(h, IDC_PROF_MODEL, "");
                    combo_select(h, IDC_PROF_TYPE, 0);
                    // プロンプトはUI欄が無いため、保存時に既定値 (DEFAULT_GEMINI_*) を使う
                    PENDING_NEW.with(|f| *f.borrow_mut() = true);
                }
                IDC_PROF_SAVE | IDC_PROF_SAVEAS => {
                    let _ = save_profile_from_ui(h, id == IDC_PROF_SAVEAS);
                }
                IDC_PROF_DEL => {
                    let deleted = PROFILES.with(|p| {
                        let mut profiles = p.borrow_mut();
                        if profiles.len() <= 1 {
                            unsafe { MessageBoxW(Some(h), w!("最低1つは残す必要があります"), w!("エラー"), MB_OK); }
                            return false;
                        }
                        let sel = combo_sel(h, IDC_PROF_LIST);
                        if sel < profiles.len() {
                            profiles.remove(sel);
                            true
                        } else {
                            false
                        }
                    });
                    if deleted {
                        refill_profile_combo(h, 0);
                        load_profile_to_ui(h, 0);
                        auto_save(h, false);
                    }
                }
                IDC_OPEN_LOG => {
                    let inst = unsafe {
                        windows::Win32::Foundation::HINSTANCE(
                            windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                                .map(|m| m.0)
                                .unwrap_or(std::ptr::null_mut()),
                        )
                    };
                    crate::logviewer::open(inst);
                }
                IDC_TEST_YOMI | IDC_TEST_NDL => {
                    let url =
                        get_text(h, if id == IDC_TEST_YOMI { IDC_YOMI } else { IDC_NDL });
                    let ok = crate::ocr::health_check(&url);
                    unsafe {
                        if ok {
                            MessageBoxW(
                                Some(h),
                                w!("接続に成功しました。"),
                                w!("接続テスト"),
                                MB_OK | MB_ICONINFORMATION,
                            );
                        } else {
                            MessageBoxW(
                                Some(h),
                                w!("接続できませんでした。サーバーが起動しているか確認してください。"),
                                w!("接続テスト"),
                                MB_OK,
                            );
                        }
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_PADDLE_DONE => {
            handle_install_done(
                h,
                wparam,
                lparam,
                refresh_paddle_status,
                "PaddleOCRのモデルをインストールしました。",
            );
            LRESULT(0)
        }
        WM_ONNX_DONE => {
            handle_install_done(
                h,
                wparam,
                lparam,
                refresh_onnx_status,
                "ローカルONNX翻訳モデルをインストールしました。",
            );
            LRESULT(0)
        }
        WM_CLOSE => {
            // モードAのプロンプト編集ウィンドウを連動して閉じる (SPECv0.4.7)。
            // 未保存テンプレートの破棄をユーザーがキャンセルしたら設定画面も閉じない。
            if !crate::prompt_edit::close_for_settings() {
                return LRESULT(0);
            }
            CLOSING.with(|f| *f.borrow_mut() = true);
            unsafe {
                let _ = DestroyWindow(h);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            WND.with(|w| *w.borrow_mut() = 0);
            CLOSING.with(|f| *f.borrow_mut() = false);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(h, msg, wparam, lparam) },
    }
}
