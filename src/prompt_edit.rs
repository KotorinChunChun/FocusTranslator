// 解説プロンプトの編集ダイアログ (SPEC v0.2 §2.2.2)
// LLMへ送るプロンプトを送信前にユーザーが編集でき、使用するAPIプロファイルも選択できる。
// 送信ボタンで on_submit(プロンプト, プロファイル名) を1回だけ呼ぶ。
use std::ffi::c_void;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_WINDOW, CreateFontW, DEFAULT_CHARSET,
    DEFAULT_PITCH, FONT_OUTPUT_PRECISION, FW_NORMAL, HBRUSH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetDlgItem, GetWindowLongPtrW,
    GetWindowTextLengthW, GetWindowTextW, HMENU, IDC_ARROW, LoadCursorW, RegisterClassW, SW_SHOW,
    SWP_NOZORDER, SendMessageW, SetWindowLongPtrW, SetWindowPos, ShowWindow, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_COMMAND, WM_DESTROY, WM_SETFONT, WM_SETTEXT, WM_SIZE, WNDCLASSW, WS_BORDER,
    WS_CHILD, WS_EX_APPWINDOW, WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
    WS_VSCROLL,
};
use windows::core::w;

const IDC_PROFILE: i32 = 100;
const IDC_EDIT: i32 = 101;
const IDC_SUBMIT: i32 = 102;

// windows クレートに定義がないコントロールスタイル
const ES_MULTILINE: u32 = 0x0004;
const ES_AUTOVSCROLL: u32 = 0x0040;
const ES_WANTRETURN: u32 = 0x1000;
const CBS_DROPDOWNLIST: u32 = 0x0003;

const PAD: i32 = 10;
const COMBO_H: i32 = 26;
const BTN_W: i32 = 100;
const BTN_H: i32 = 30;
const WIN_W: i32 = 600;
const WIN_H: i32 = 430;

struct State {
    profiles: Vec<String>,
    on_submit: Box<dyn FnOnce(String, String)>,
}

/// 編集ダイアログを開く。
/// pos: 表示位置 (スクリーン座標。オーバーレイ近傍を渡す)。None なら既定位置。
/// profiles / active_idx: 選択肢となるAPIプロファイル名と初期選択。
pub fn open(
    inst: HINSTANCE,
    parent: HWND,
    pos: Option<(i32, i32)>,
    initial_text: &str,
    profiles: Vec<String>,
    active_idx: usize,
    on_submit: impl FnOnce(String, String) + 'static,
) {
    let state = Box::new(State { profiles: profiles.clone(), on_submit: Box::new(on_submit) });
    let ptr = Box::into_raw(state) as isize;

    unsafe {
        let class_name = w!("FocusTranslatorPromptEdit");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: inst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: class_name,
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as isize as *mut c_void),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc); // 2回目以降の登録失敗は無視

        let (x, y) = pos.unwrap_or((CW_USEDEFAULT, CW_USEDEFAULT));
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_APPWINDOW,
            class_name,
            w!("解説プロンプトの編集"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            x,
            y,
            WIN_W,
            WIN_H,
            Some(parent),
            None,
            Some(inst),
            None,
        ) else {
            // ウィンドウを作れなければコールバックを解放して終了
            drop(Box::from_raw(ptr as *mut State));
            return;
        };

        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr);

        // モデル (APIプロファイル) 選択コンボ
        let combo = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("COMBOBOX"),
            None,
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST),
            PAD, PAD, 300, 200,
            Some(hwnd),
            Some(HMENU(IDC_PROFILE as usize as *mut c_void)),
            Some(inst),
            None,
        )
        .unwrap_or_default();
        for name in &profiles {
            let wide = crate::util::to_wide(name);
            SendMessageW(combo, CB_ADDSTRING, Some(WPARAM(0)), Some(LPARAM(wide.as_ptr() as isize)));
        }
        SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(active_idx)), Some(LPARAM(0)));

        let edit = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            w!("EDIT"),
            None,
            WS_CHILD | WS_VISIBLE | WS_BORDER | WS_VSCROLL
                | WINDOW_STYLE(ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN),
            PAD, PAD + COMBO_H + 4, 560, 290,
            Some(hwnd),
            Some(HMENU(IDC_EDIT as usize as *mut c_void)),
            Some(inst),
            None,
        )
        .unwrap_or_default();

        let btn = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("BUTTON"),
            w!("送信"),
            WS_CHILD | WS_VISIBLE,
            PAD, 350, BTN_W, BTN_H,
            Some(hwnd),
            Some(HMENU(IDC_SUBMIT as usize as *mut c_void)),
            Some(inst),
            None,
        )
        .unwrap_or_default();

        let font = CreateFontW(
            -14, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, DEFAULT_CHARSET, FONT_OUTPUT_PRECISION(0),
            CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, DEFAULT_PITCH.0.into(), w!("Yu Gothic UI"),
        );
        for ctl in [combo, edit, btn] {
            let _ = SendMessageW(ctl, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(0)));
        }

        let wide = crate::util::to_wide(initial_text);
        let _ = SendMessageW(edit, WM_SETTEXT, Some(WPARAM(0)), Some(LPARAM(wide.as_ptr() as isize)));

        let _ = ShowWindow(hwnd, SW_SHOW);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            if id == IDC_SUBMIT {
                unsafe {
                    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                    if ptr != 0
                        && let Ok(edit) = GetDlgItem(Some(hwnd), IDC_EDIT)
                    {
                        let len = GetWindowTextLengthW(edit);
                        if len > 0 {
                            let mut buf = vec![0u16; (len + 1) as usize];
                            GetWindowTextW(edit, &mut buf);
                            let text = String::from_utf16_lossy(&buf[..len as usize]);
                            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0); // 二重解放防止
                            let state = Box::from_raw(ptr as *mut State);
                            let profile = selected_profile(hwnd, &state.profiles);
                            (state.on_submit)(text, profile);
                        }
                    }
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_SIZE => {
            unsafe {
                let w = (lparam.0 & 0xFFFF) as i32;
                let h = ((lparam.0 >> 16) & 0xFFFF) as i32;
                if let Ok(combo) = GetDlgItem(Some(hwnd), IDC_PROFILE) {
                    let _ = SetWindowPos(combo, None, PAD, PAD, (w - PAD * 2).min(300), 200, SWP_NOZORDER);
                }
                if let Ok(edit) = GetDlgItem(Some(hwnd), IDC_EDIT) {
                    let edit_top = PAD + COMBO_H + 4;
                    let _ = SetWindowPos(edit, None, PAD, edit_top, w - PAD * 2, h - edit_top - BTN_H - PAD * 2, SWP_NOZORDER);
                }
                if let Ok(btn) = GetDlgItem(Some(hwnd), IDC_SUBMIT) {
                    let _ = SetWindowPos(btn, None, PAD, h - BTN_H - PAD, BTN_W, BTN_H, SWP_NOZORDER);
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                // 未送信で閉じられた場合はここでコールバックを解放
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                if ptr != 0 {
                    let _ = Box::from_raw(ptr as *mut State);
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// コンボで選択中のプロファイル名 (取得不能時は先頭または空文字)
fn selected_profile(hwnd: HWND, profiles: &[String]) -> String {
    unsafe {
        let idx = GetDlgItem(Some(hwnd), IDC_PROFILE)
            .map(|cb| SendMessageW(cb, CB_GETCURSEL, Some(WPARAM(0)), Some(LPARAM(0))).0)
            .unwrap_or(-1);
        let idx = if idx < 0 { 0 } else { idx as usize };
        profiles.get(idx).cloned().unwrap_or_default()
    }
}
