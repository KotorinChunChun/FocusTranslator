use std::cell::RefCell;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, CreateFontW, DEFAULT_CHARSET, FW_NORMAL, OUT_DEFAULT_PRECIS,
    CLIP_DEFAULT_PRECIS, DEFAULT_QUALITY, DEFAULT_PITCH,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetWindowTextLengthW, GetWindowTextW, LoadCursorW, RegisterClassW, SetForegroundWindow,
    SetWindowTextW, ShowWindow, TranslateMessage, MSG, WINDOW_STYLE, WM_CLOSE, WM_COMMAND,
    WM_DESTROY, WM_SETFONT, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_EX_TOPMOST,
    WS_POPUPWINDOW, WS_SYSMENU, WS_VISIBLE, WS_BORDER, WS_VSCROLL, CW_USEDEFAULT, IDC_ARROW,
    SW_SHOW, ES_MULTILINE, ES_AUTOVSCROLL, GetDlgItem, HMENU, SendMessageW, WM_KEYDOWN
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_RETURN, VK_ESCAPE};
use windows::core::{PCWSTR, w};
use crate::util::to_wide;

const IDC_EDIT: i32 = 101;
const IDC_SAVE: i32 = 102;
const IDC_CANCEL: i32 = 103;

thread_local! {
    static DIALOG_RESULT: RefCell<Option<String>> = RefCell::new(None);
    static HAS_RESULT: RefCell<bool> = RefCell::new(false);
}

pub fn show(parent: HWND, title: &str, initial_text: &str) -> Option<String> {
    DIALOG_RESULT.with(|r| *r.borrow_mut() = None);
    HAS_RESULT.with(|r| *r.borrow_mut() = false);

    unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class_name = w!("FocusTranslatorInputClass");
        
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH((COLOR_BTNFACE.0 + 1) as usize as *mut _),
            lpszClassName: class_name,
            ..Default::default()
        };
        // 既に登録済みでも無視
        let _ = RegisterClassW(&wc);

        let wide_title = to_wide(title);
        
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST,
            class_name,
            PCWSTR(wide_title.as_ptr()),
            WS_POPUPWINDOW | WS_CAPTION | WS_SYSMENU,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            600,
            400,
            Some(parent),
            None,
            Some(instance.into()),
            None,
        );

        if hwnd.is_ok() {
            let hwnd = hwnd.unwrap();
            let _ = EnableWindow(parent, false);

            let font = CreateFontW(
                20, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS, DEFAULT_QUALITY, DEFAULT_PITCH.0 as u32, w!("Meiryo"),
            );

            let h_edit = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                PCWSTR::null(),
                WS_CHILD | WS_VISIBLE | WS_BORDER | WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE as u32) | (ES_AUTOVSCROLL as u32)),
                10, 10, 560, 290,
                Some(hwnd),
                Some(HMENU(IDC_EDIT as *mut _)),
                Some(instance.into()),
                None,
            ).unwrap();
            let _ = SendMessageW(h_edit, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(0)));
            
            let wide_initial = to_wide(&initial_text.replace("\n", "\r\n"));
            let _ = SetWindowTextW(h_edit, PCWSTR(wide_initial.as_ptr()));

            let h_save = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE::default(),
                w!("BUTTON"),
                w!("保存"),
                WS_CHILD | WS_VISIBLE,
                350, 315, 100, 35,
                Some(hwnd),
                Some(HMENU(IDC_SAVE as *mut _)),
                Some(instance.into()),
                None,
            ).unwrap();
            let _ = SendMessageW(h_save, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(0)));

            let h_cancel = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE::default(),
                w!("BUTTON"),
                w!("キャンセル"),
                WS_CHILD | WS_VISIBLE,
                460, 315, 100, 35,
                Some(hwnd),
                Some(HMENU(IDC_CANCEL as *mut _)),
                Some(instance.into()),
                None,
            ).unwrap();
            let _ = SendMessageW(h_cancel, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(0)));

            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);

            let mut msg = MSG::default();
            while unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(hwnd)).as_bool() && GetMessageW(&mut msg, None, 0, 0).as_bool() } {
                if msg.message == WM_DESTROY && msg.hwnd == hwnd {
                    break;
                }
                if msg.message == WM_KEYDOWN {
                    let key = msg.wParam.0 as u16;
                    if key == VK_RETURN.0 {
                        let ctrl = GetAsyncKeyState(VK_CONTROL.0 as i32) as i16;
                        if ctrl < 0 {
                            let _ = SendMessageW(hwnd, WM_COMMAND, Some(WPARAM(IDC_SAVE as usize)), Some(LPARAM(0)));
                            continue;
                        }
                    } else if key == VK_ESCAPE.0 {
                        let _ = SendMessageW(hwnd, WM_COMMAND, Some(WPARAM(IDC_CANCEL as usize)), Some(LPARAM(0)));
                        continue;
                    }
                }
                
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
                
                if HAS_RESULT.with(|r| *r.borrow()) {
                    let _ = DestroyWindow(hwnd);
                    HAS_RESULT.with(|r| *r.borrow_mut() = false);
                }
            }

            let _ = EnableWindow(parent, true);
            let _ = SetForegroundWindow(parent);
        }
    }
    DIALOG_RESULT.with(|r| r.borrow_mut().take())
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            if id == IDC_SAVE {
                unsafe {
                    let h_edit = GetDlgItem(Some(hwnd), IDC_EDIT).unwrap_or_default();
                    let len = GetWindowTextLengthW(h_edit) as usize;
                    let mut buf = vec![0u16; len + 1];
                    GetWindowTextW(h_edit, &mut buf);
                    if let Some(pos) = buf.iter().position(|&c| c == 0) {
                        buf.truncate(pos);
                    }
                    let text = String::from_utf16_lossy(&buf).replace("\r\n", "\n");
                    DIALOG_RESULT.with(|r| *r.borrow_mut() = Some(text));
                    HAS_RESULT.with(|r| *r.borrow_mut() = true);
                }
                return LRESULT(0);
            } else if id == IDC_CANCEL {
                unsafe { let _ = DestroyWindow(hwnd); }
                return LRESULT(0);
            }
        }
        WM_CLOSE => {
            unsafe { let _ = DestroyWindow(hwnd); }
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
