use std::ffi::c_void;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowTextLengthW, GetWindowTextW,
    RegisterClassW, SetWindowLongPtrW, GetWindowLongPtrW, ShowWindow,
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GWLP_USERDATA, IDC_ARROW, SW_SHOW,
    WM_COMMAND, WM_DESTROY, WM_SIZE, WNDCLASSW, WS_BORDER, WS_CHILD,
    WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL,
};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH, FW_NORMAL, CLIP_DEFAULT_PRECIS,
    CLEARTYPE_QUALITY, FONT_OUTPUT_PRECISION, COLOR_WINDOW,
};
use windows::core::{w, PCWSTR};

const IDC_EDIT: u32 = 101;
const IDC_SUBMIT: u32 = 102;

struct State {
    on_submit: Box<dyn FnOnce(String)>,
}

pub fn open(inst: HINSTANCE, parent: HWND, initial_text: &str, on_submit: impl FnOnce(String) + 'static) {
    let state = Box::new(State { on_submit: Box::new(on_submit) });
    let ptr = Box::into_raw(state) as isize;

    unsafe {
        let class_name = w!("FocusTranslatorPromptEdit");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: inst,
            hCursor: windows::Win32::UI::WindowsAndMessaging::LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: class_name,
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH((COLOR_WINDOW.0 + 1) as isize as *mut c_void),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc); // 無視

        let hwnd = CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WS_EX_APPWINDOW,
            class_name,
            w!("解説プロンプトの編集"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            600,
            400,
            Some(parent),
            None,
            Some(inst),
            None,
        ).unwrap();

        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr);

        let edit = CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WS_EX_CLIENTEDGE,
            w!("EDIT"),
            None,
            WS_CHILD | WS_VISIBLE | WS_BORDER | WS_VSCROLL | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(0x0004 | 0x0040 | 0x1000), // ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN
            10, 10, 560, 300,
            Some(hwnd),
            Some(windows::Win32::UI::WindowsAndMessaging::HMENU(IDC_EDIT as usize as *mut c_void)),
            Some(inst),
            None,
        ).unwrap();

        let btn = CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE::default(),
            w!("BUTTON"),
            w!("送信"),
            WS_CHILD | WS_VISIBLE | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(0x0000), // BS_PUSHBUTTON
            10, 320, 100, 30,
            Some(hwnd),
            Some(windows::Win32::UI::WindowsAndMessaging::HMENU(IDC_SUBMIT as usize as *mut c_void)),
            Some(inst),
            None,
        ).unwrap();

        let font = CreateFontW(
            -14, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, DEFAULT_CHARSET, FONT_OUTPUT_PRECISION(0),
            CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, DEFAULT_PITCH.0.into(), w!("Yu Gothic UI"),
        );
        let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(edit, windows::Win32::UI::WindowsAndMessaging::WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(0)));
        let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(btn, windows::Win32::UI::WindowsAndMessaging::WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(0)));

        let mut wide: Vec<u16> = initial_text.encode_utf16().collect();
        wide.push(0);
        let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(edit, windows::Win32::UI::WindowsAndMessaging::WM_SETTEXT, Some(WPARAM(0)), Some(LPARAM(wide.as_ptr() as isize)));

        let _ = ShowWindow(hwnd, SW_SHOW);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as u32;
            if id == IDC_SUBMIT {
                unsafe {
                    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                    if ptr != 0 {
                        let edit = windows::Win32::UI::WindowsAndMessaging::GetDlgItem(Some(hwnd), IDC_EDIT as i32).unwrap();
                        let len = GetWindowTextLengthW(edit);
                        if len > 0 {
                            let mut buf = vec![0u16; (len + 1) as usize];
                            GetWindowTextW(edit, &mut buf);
                            let text = String::from_utf16_lossy(&buf[..len as usize]);
                            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0); // clear
                            let state = Box::from_raw(ptr as *mut State);
                            (state.on_submit)(text);
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
                if let Ok(edit) = windows::Win32::UI::WindowsAndMessaging::GetDlgItem(Some(hwnd), IDC_EDIT as i32) {
                    let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(edit, None, 10, 10, w - 20, h - 60, windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER);
                }
                if let Ok(btn) = windows::Win32::UI::WindowsAndMessaging::GetDlgItem(Some(hwnd), IDC_SUBMIT as i32) {
                    let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(btn, None, 10, h - 40, 100, 30, windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER);
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
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