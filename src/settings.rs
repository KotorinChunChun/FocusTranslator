// 設定画面 (SPEC §12)
use crate::config::Config;
use crate::util::{self, to_wide};
use std::cell::RefCell;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_BTNFACE, CreateFontW, DEFAULT_CHARSET,
    DEFAULT_PITCH, FONT_OUTPUT_PRECISION, FW_NORMAL, HFONT,
};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW,
};
use windows::Win32::UI::Controls::EM_SETPASSWORDCHAR;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL,
    CBS_DROPDOWNLIST, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
    DestroyWindow, ES_AUTOHSCROLL, ES_PASSWORD, GetWindowTextLengthW,
    GetWindowTextW, HMENU, IDC_ARROW, IsWindow, LoadCursorW, MB_ICONINFORMATION, MB_OK,
    MB_YESNO, MessageBoxW, PostMessageW, RegisterClassW, SW_SHOW, SendMessageW,
    SetForegroundWindow, ShowWindow, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_DESTROY,
    WM_SETFONT, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_EX_TOPMOST, WS_SYSMENU,
    WS_TABSTOP, WS_VISIBLE,
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
const IDC_GEMINI: i32 = 109;
const IDC_GMODEL: i32 = 110;
const IDC_YOMI: i32 = 111;
const IDC_NDL: i32 = 112;
const IDC_AUTOSTART: i32 = 113;
const IDC_PERFLOG: i32 = 114;
const IDC_CONSENT_RESET: i32 = 115;
const IDC_SAVE: i32 = 116;
const IDC_CLOSE: i32 = 117;
const IDC_TEST_YOMI: i32 = 118;
const IDC_TEST_NDL: i32 = 119;
const IDC_PADDLE_STATUS: i32 = 120;
const IDC_PADDLE_INSTALL: i32 = 121;

/// PaddleOCRインストールスレッドからの完了通知 (settings ウィンドウ限定のメッセージ)
const WM_PADDLE_DONE: u32 = WM_APP + 10;
/// ● (U+25CF): APIキー入力欄のマスク文字
const PASSWORD_CHAR: usize = 0x25CF;

const HOLD_KEYS: [&str; 5] = ["RCtrl", "LCtrl", "RShift", "RAlt", "F8"];
const OCR_KEYS: [&str; 5] = ["win", "paddle", "yomitoku", "ndl", "gemini"];
const OCR_DISP: [&str; 5] = ["Windows OCR", "PaddleOCR", "YomiToku", "NDL-OCR", "Gemini"];
const TR_KEYS: [&str; 4] = ["local", "deepl", "google", "gemini"];
const TR_DISP: [&str; 4] = ["ローカルONNX", "DeepL", "Google", "Gemini"];
const LANGS: [&str; 2] = ["ja", "en"];

thread_local! {
    static WND: RefCell<isize> = const { RefCell::new(0) };
    static REGISTERED: RefCell<bool> = const { RefCell::new(false) };
    static FONT: RefCell<isize> = const { RefCell::new(0) };
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
        if let Ok(h) = CreateWindowExW(
            WS_EX_TOPMOST,
            class,
            w!("Focus Translator 設定"),
            WS_CAPTION | WS_SYSMENU,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            520,
            636,
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

#[allow(clippy::too_many_arguments)]
fn ctl(parent: HWND, instance: HINSTANCE, class: PCWSTR, text: &str, style: WINDOW_STYLE, x: i32, y: i32, w: i32, h: i32, id: i32) -> HWND {
    unsafe {
        let wide = to_wide(text);
        CreateWindowExW(
            Default::default(),
            class,
            PCWSTR(wide.as_ptr()),
            WS_CHILD | WS_VISIBLE | style,
            x,
            y,
            w,
            h,
            Some(parent),
            Some(HMENU(id as usize as *mut _)),
            Some(instance),
            None,
        )
        .unwrap_or_default()
    }
}

fn label(parent: HWND, instance: HINSTANCE, text: &str, x: i32, y: i32, w: i32) {
    ctl(parent, instance, w!("STATIC"), text, WINDOW_STYLE(0), x, y, w, 20, 0);
}

fn edit(parent: HWND, instance: HINSTANCE, x: i32, y: i32, w: i32, id: i32) -> HWND {
    ctl(
        parent,
        instance,
        w!("EDIT"),
        "",
        WS_BORDER | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        x,
        y,
        w,
        22,
        id,
    )
}

/// APIキー入力欄。安全のため ● でマスク表示する(内部の実値はそのまま保持される)
fn password_edit(parent: HWND, instance: HINSTANCE, x: i32, y: i32, w: i32, id: i32) -> HWND {
    let h = ctl(
        parent,
        instance,
        w!("EDIT"),
        "",
        WS_BORDER | WS_TABSTOP | WINDOW_STYLE((ES_AUTOHSCROLL | ES_PASSWORD) as u32),
        x,
        y,
        w,
        22,
        id,
    );
    unsafe {
        SendMessageW(h, EM_SETPASSWORDCHAR, Some(WPARAM(PASSWORD_CHAR)), Some(LPARAM(0)));
    }
    h
}

fn combo(parent: HWND, instance: HINSTANCE, x: i32, y: i32, w: i32, id: i32) -> HWND {
    ctl(
        parent,
        instance,
        w!("COMBOBOX"),
        "",
        WS_TABSTOP | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
        x,
        y,
        w,
        200,
        id,
    )
}

fn button(parent: HWND, instance: HINSTANCE, text: &str, x: i32, y: i32, w: i32, id: i32) -> HWND {
    ctl(parent, instance, w!("BUTTON"), text, WS_TABSTOP, x, y, w, 26, id)
}

fn checkbox(parent: HWND, instance: HINSTANCE, text: &str, x: i32, y: i32, w: i32, id: i32) -> HWND {
    ctl(
        parent,
        instance,
        w!("BUTTON"),
        text,
        WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
        x,
        y,
        w,
        22,
        id,
    )
}

fn build_controls(h: HWND, inst: HINSTANCE) {
    let lx = 16;
    let cx = 180;
    let cw = 250;
    let mut y = 14;
    let step = 32;

    label(h, inst, "ホールドキー", lx, y + 2, 150);
    combo(h, inst, cx, y, 120, IDC_HOLDKEY);
    y += step;
    label(h, inst, "監視周期 (ms)", lx, y + 2, 150);
    edit(h, inst, cx, y, 80, IDC_POLL);
    y += step;
    label(h, inst, "範囲指定ホットキー", lx, y + 2, 160);
    edit(h, inst, cx, y, 120, IDC_HOTKEY);
    y += step;
    label(h, inst, "既定OCRエンジン", lx, y + 2, 150);
    combo(h, inst, cx, y, 150, IDC_OCR);
    y += step;
    // PaddleOCR 導入状況 + ワンクリックインストール (SPEC §7.1, §13)
    label(h, inst, "PaddleOCR", lx, y + 2, 150);
    ctl(h, inst, w!("STATIC"), "確認中…", WINDOW_STYLE(0), cx, y + 2, 140, 20, IDC_PADDLE_STATUS);
    button(h, inst, "インストール", cx + 146, y - 2, 104, IDC_PADDLE_INSTALL);
    y += step;
    label(h, inst, "既定翻訳エンジン", lx, y + 2, 150);
    combo(h, inst, cx, y, 150, IDC_TR);
    y += step;
    label(h, inst, "訳先言語", lx, y + 2, 150);
    combo(h, inst, cx, y, 80, IDC_LANG);
    y += step;
    label(h, inst, "DeepL APIキー", lx, y + 2, 150);
    password_edit(h, inst, cx, y, cw, IDC_DEEPL);
    y += step;
    label(h, inst, "Google APIキー", lx, y + 2, 150);
    password_edit(h, inst, cx, y, cw, IDC_GOOGLE);
    y += step;
    label(h, inst, "Gemini APIキー", lx, y + 2, 150);
    password_edit(h, inst, cx, y, cw, IDC_GEMINI);
    y += step;
    label(h, inst, "Geminiモデル", lx, y + 2, 150);
    edit(h, inst, cx, y, cw, IDC_GMODEL);
    y += step;
    label(h, inst, "YomiToku サーバーURL", lx, y + 2, 160);
    edit(h, inst, cx, y, 190, IDC_YOMI);
    button(h, inst, "テスト", cx + 196, y - 2, 54, IDC_TEST_YOMI);
    y += step;
    label(h, inst, "NDL-OCR サーバーURL", lx, y + 2, 160);
    edit(h, inst, cx, y, 190, IDC_NDL);
    button(h, inst, "テスト", cx + 196, y - 2, 54, IDC_TEST_NDL);
    y += step;
    checkbox(h, inst, "起動時に常駐する", lx, y, 200, IDC_AUTOSTART);
    checkbox(h, inst, "計測ログを有効化", cx + 40, y, 200, IDC_PERFLOG);
    y += step;
    button(h, inst, "外部送信の同意状態をリセット", lx, y, 220, IDC_CONSENT_RESET);
    y += step + 10;
    button(h, inst, "保存", cx + 60, y, 90, IDC_SAVE);
    button(h, inst, "閉じる", cx + 160, y, 90, IDC_CLOSE);

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
    combo_fill(h, IDC_LANG, &LANGS, LANGS.iter().position(|k| *k == cfg.target_lang).unwrap_or(0));
    set_text(h, IDC_DEEPL, &cfg.deepl_key());
    set_text(h, IDC_GOOGLE, &cfg.google_key());
    set_text(h, IDC_GEMINI, &cfg.gemini_key());
    set_text(h, IDC_GMODEL, &cfg.gemini_model);
    set_text(h, IDC_YOMI, &cfg.yomitoku_url);
    set_text(h, IDC_NDL, &cfg.ndl_url);
    check_set(h, IDC_AUTOSTART, cfg.autostart);
    check_set(h, IDC_PERFLOG, cfg.perf_log);
    refresh_paddle_status(h);
}

/// PaddleOCRの導入状況をステータス欄・ボタンに反映する
fn refresh_paddle_status(h: HWND) {
    let installed = crate::paddle_install::installed();
    set_text(h, IDC_PADDLE_STATUS, if installed { "導入済み" } else { "未導入" });
    unsafe {
        let _ = EnableWindow(get_dlg_item(h, IDC_PADDLE_INSTALL), !installed);
    }
}

fn save(h: HWND) {
    let mut cfg = Config::load();
    cfg.hold_key = HOLD_KEYS[combo_sel(h, IDC_HOLDKEY).min(HOLD_KEYS.len() - 1)].to_string();
    cfg.poll_ms = get_text(h, IDC_POLL).trim().parse().unwrap_or(100).clamp(20, 1000);
    let hk = get_text(h, IDC_HOTKEY);
    if crate::config::parse_hotkey(&hk).is_some() {
        cfg.region_hotkey = hk.trim().to_string();
    }
    cfg.default_ocr = OCR_KEYS[combo_sel(h, IDC_OCR).min(OCR_KEYS.len() - 1)].to_string();
    cfg.default_translator = TR_KEYS[combo_sel(h, IDC_TR).min(TR_KEYS.len() - 1)].to_string();
    cfg.target_lang = LANGS[combo_sel(h, IDC_LANG).min(LANGS.len() - 1)].to_string();
    cfg.deepl_key_enc = util::dpapi_encrypt(get_text(h, IDC_DEEPL).trim());
    cfg.google_key_enc = util::dpapi_encrypt(get_text(h, IDC_GOOGLE).trim());
    cfg.gemini_key_enc = util::dpapi_encrypt(get_text(h, IDC_GEMINI).trim());
    let gm = get_text(h, IDC_GMODEL).trim().to_string();
    if !gm.is_empty() {
        cfg.gemini_model = gm;
    }
    cfg.yomitoku_url = get_text(h, IDC_YOMI).trim().to_string();
    cfg.ndl_url = get_text(h, IDC_NDL).trim().to_string();
    cfg.autostart = check_get(h, IDC_AUTOSTART);
    cfg.perf_log = check_get(h, IDC_PERFLOG);

    // 既定エンジンがクラウド/外部送信を伴う場合はここで同意を確認 (SPEC §9)
    confirm_default_consents(h, &mut cfg);

    cfg.save();
    apply_autostart(cfg.autostart);
}

fn confirm_default_consents(h: HWND, cfg: &mut Config) {
    unsafe {
        if matches!(cfg.default_translator.as_str(), "deepl" | "google" | "gemini")
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
        if cfg.default_ocr == "gemini" && !cfg.consent_image {
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
            match id {
                IDC_SAVE => {
                    save(h);
                    // main へ設定再読込を通知
                    unsafe {
                        let _ = PostMessageW(
                            Some(crate::main_hwnd()),
                            crate::WM_APP_CFG,
                            WPARAM(0),
                            LPARAM(0),
                        );
                        MessageBoxW(
                            Some(h),
                            w!("設定を保存しました。"),
                            w!("Focus Translator"),
                            MB_OK | MB_ICONINFORMATION,
                        );
                        let _ = DestroyWindow(h);
                    }
                }
                IDC_CLOSE => unsafe {
                    let _ = DestroyWindow(h);
                },
                IDC_CONSENT_RESET => {
                    let mut cfg = Config::load();
                    cfg.consent_text = false;
                    cfg.consent_image = false;
                    cfg.consent_ext_ocr = false;
                    cfg.save();
                    unsafe {
                        let _ = PostMessageW(
                            Some(crate::main_hwnd()),
                            crate::WM_APP_CFG,
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
                    unsafe {
                        let _ = EnableWindow(get_dlg_item(h, IDC_PADDLE_INSTALL), false);
                    }
                    set_text(h, IDC_PADDLE_STATUS, "ダウンロード中…");
                    let hwnd_isize = h.0 as isize;
                    std::thread::spawn(move || {
                        let result = crate::paddle_install::install();
                        let (w, l) = match result {
                            Ok(()) => (1usize, 0isize),
                            Err(e) => (0usize, Box::into_raw(Box::new(e)) as isize),
                        };
                        unsafe {
                            let _ = PostMessageW(
                                Some(HWND(hwnd_isize as *mut _)),
                                WM_PADDLE_DONE,
                                WPARAM(w),
                                LPARAM(l),
                            );
                        }
                    });
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
            if wparam.0 == 1 {
                refresh_paddle_status(h);
                unsafe {
                    MessageBoxW(
                        Some(h),
                        w!("PaddleOCRのモデルをインストールしました。"),
                        w!("Focus Translator"),
                        MB_OK | MB_ICONINFORMATION,
                    );
                }
            } else {
                let msg = unsafe { *Box::from_raw(lparam.0 as *mut String) };
                refresh_paddle_status(h);
                unsafe {
                    let wide = to_wide(&msg);
                    MessageBoxW(Some(h), PCWSTR(wide.as_ptr()), w!("インストールエラー"), MB_OK);
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(h);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            WND.with(|w| *w.borrow_mut() = 0);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(h, msg, wparam, lparam) },
    }
}
