// 範囲指定モード (SPEC §3.2): Ctrl+Alt+T で半透明オーバーレイを表示し、
// ドラッグ選択した矩形を main へ WM_APP_REGION で通知する。
use std::cell::RefCell;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, EndPaint, FillRect, FrameRect, HGDIOBJ, InvalidateRect,
    PAINTSTRUCT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, IDC_CROSS,
    LWA_ALPHA, LoadCursorW, PostMessageW, RegisterClassW, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_SHOW, SetForegroundWindow,
    SetLayeredWindowAttributes, ShowWindow, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_PAINT, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::w;

struct SelState {
    main_hwnd: isize,
    origin: (i32, i32), // 仮想スクリーン原点
    dragging: bool,
    start: POINT,
    cur: POINT,
}

thread_local! {
    static STATE: RefCell<Option<SelState>> = const { RefCell::new(None) };
    static REGISTERED: RefCell<bool> = const { RefCell::new(false) };
}

/// 範囲指定オーバーレイを開始する
pub fn start(instance: HINSTANCE, main_hwnd: HWND) {
    unsafe {
        let class = w!("FocusTranslatorRegion");
        REGISTERED.with(|r| {
            if !*r.borrow() {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(wndproc),
                    hInstance: instance,
                    hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
                    lpszClassName: class,
                    ..Default::default()
                };
                RegisterClassW(&wc);
                *r.borrow_mut() = true;
            }
        });

        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);

        STATE.with(|s| {
            *s.borrow_mut() = Some(SelState {
                main_hwnd: main_hwnd.0 as isize,
                origin: (vx, vy),
                dragging: false,
                start: POINT::default(),
                cur: POINT::default(),
            })
        });

        if let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            class,
            w!("範囲を選択"),
            WS_POPUP,
            vx,
            vy,
            vw,
            vh,
            None,
            None,
            Some(instance),
            None,
        ) {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 70, LWA_ALPHA);
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            unsafe {
                SetCapture(hwnd);
            }
            STATE.with(|s| {
                if let Some(st) = s.borrow_mut().as_mut() {
                    st.dragging = true;
                    st.start = POINT { x, y };
                    st.cur = POINT { x, y };
                }
            });
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let dragging = STATE.with(|s| {
                if let Some(st) = s.borrow_mut().as_mut()
                    && st.dragging {
                        st.cur = POINT { x, y };
                        return true;
                    }
                false
            });
            if dragging {
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, true);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            let info = STATE.with(|s| s.borrow_mut().take());
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            if let Some(st) = info
                && st.dragging {
                    let x = (lparam.0 & 0xFFFF) as i16 as i32;
                    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                    let left = st.start.x.min(x) + st.origin.0;
                    let top = st.start.y.min(y) + st.origin.1;
                    let right = st.start.x.max(x) + st.origin.0;
                    let bottom = st.start.y.max(y) + st.origin.1;
                    if right - left >= 8 && bottom - top >= 8 {
                        let rect = Box::new(RECT { left, top, right, bottom });
                        unsafe {
                            let _ = PostMessageW(
                                Some(HWND(st.main_hwnd as *mut _)),
                                crate::app_state::WM_APP_REGION,
                                WPARAM(0),
                                LPARAM(Box::into_raw(rect) as isize),
                            );
                        }
                    }
                }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if wparam.0 == VK_ESCAPE.0 as usize {
                STATE.with(|s| *s.borrow_mut() = None);
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_PAINT => {
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let bg = CreateSolidBrush(COLORREF(0x00201810));
                FillRect(hdc, &ps.rcPaint, bg);
                let _ = windows::Win32::Graphics::Gdi::DeleteObject(HGDIOBJ(bg.0));
                STATE.with(|s| {
                    if let Some(st) = s.borrow().as_ref()
                        && st.dragging {
                            let r = RECT {
                                left: st.start.x.min(st.cur.x),
                                top: st.start.y.min(st.cur.y),
                                right: st.start.x.max(st.cur.x),
                                bottom: st.start.y.max(st.cur.y),
                            };
                            let sel = CreateSolidBrush(COLORREF(0x00E0B060));
                            FrameRect(hdc, &r, sel);
                            let inner = RECT {
                                left: r.left + 1,
                                top: r.top + 1,
                                right: r.right - 1,
                                bottom: r.bottom - 1,
                            };
                            FrameRect(hdc, &inner, sel);
                            let _ = windows::Win32::Graphics::Gdi::DeleteObject(HGDIOBJ(sel.0));
                        }
                });
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
