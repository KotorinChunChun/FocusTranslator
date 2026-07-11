// 領域表示 (デバッグ用): プレビューキー(既定 左Ctrl。実際の翻訳は行わない)、
// またはキャプチャキー(既定 右Ctrl。実際の翻訳ホールドと兼用)を押している間、
// プログラムが認識に使う領域 — 対象ウィンドウ / OCRキャプチャ帯 / UIA要素・行矩形 — を
// クリック透過・最前面の全画面オーバーレイに枠表示する。
// 実際の認識経路と同じ uia::probe_at_point / capture_plan::plan_capture_rect を使い、
// 検出結果を忠実に可視化する。
use crate::{capture, capture_plan, uia};
use std::cell::RefCell;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush,
    DEFAULT_CHARSET, DEFAULT_PITCH, DeleteObject, EndPaint, FONT_OUTPUT_PRECISION, FW_NORMAL,
    FillRect, FrameRect, GetTextExtentPoint32W, HGDIOBJ, InvalidateRect, PAINTSTRUCT,
    SelectObject, SetBkMode, SetTextColor, TRANSPARENT, TextOutW,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GA_ROOT, GetAncestor, GetCursorPos,
    GetSystemMetrics, IsWindow, LWA_COLORKEY, PostMessageW, RegisterClassW, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_SHOWNOACTIVATE,
    SetLayeredWindowAttributes, ShowWindow, WM_PAINT, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP, WindowFromPoint,
};
use windows::core::w;

/// 枠の色 (COLORREF は 0x00BBGGRR)
const COL_WINDOW: COLORREF = COLORREF(0x00FF6020); // 対象ウィンドウ: 青
const COL_BAND: COLORREF = COLORREF(0x000080FF); // OCRキャプチャ帯: 橙
const COL_ELEMENT: COLORREF = COLORREF(0x0000C000); // UIA要素 (TextPatternあり): 緑
const COL_LINE: COLORREF = COLORREF(0x0000FFFF); // UIA行矩形: 黄
const COL_HOVER: COLORREF = COLORREF(0x00FF00FF); // 直下要素 (TextPatternなし): 紫
const COL_LABEL_BG: COLORREF = COLORREF(0x00303030);
const COL_LABEL_FG: COLORREF = COLORREF(0x00FFFFFF);

/// 1回の検出結果 (probe スレッド → main へ WM_APP_DETECT で送る)
pub struct DetectInfo {
    pub cursor: POINT,
    /// 対象ウィンドウの矩形 (DWM拡張フレーム境界)
    pub window_rect: Option<RECT>,
    /// 実際にキャプチャされる領域 (capture_plan::plan_capture_rect の決定結果)
    pub band_rect: Option<RECT>,
    /// TextPattern が見つかった UIA 要素の矩形
    pub uia_element: Option<RECT>,
    /// カーソル行テキスト範囲の矩形群
    pub uia_lines: Vec<RECT>,
    /// TextPattern が無かった場合の直下要素の矩形
    pub hover_rect: Option<RECT>,
    /// カーソル近傍に表示する説明ラベル
    pub label: String,
}

struct State {
    hwnd: isize,
    /// 仮想スクリーン原点 (スクリーン座標 → ウィンドウ座標の変換用)
    origin: (i32, i32),
    info: Option<DetectInfo>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static REGISTERED: RefCell<bool> = const { RefCell::new(false) };
}

/// 検出オーバーレイを表示する (検出キー押下開始時)
pub fn show(instance: HINSTANCE) {
    let alive = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|st| unsafe { IsWindow(Some(HWND(st.hwnd as *mut _))).as_bool() })
            .unwrap_or(false)
    });
    if alive {
        return;
    }
    unsafe {
        let class = w!("FocusTranslatorDetect");
        REGISTERED.with(|r| {
            if !*r.borrow() {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(wndproc),
                    hInstance: instance,
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

        // クリック透過(WS_EX_TRANSPARENT)にすることで、WindowFromPoint / UIA の
        // ヒットテストから自身を除外し、検出対象へ影響を与えない。
        if let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
            class,
            w!("領域検出"),
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
            // 黒をカラーキーにして背景を透過し、色付きの枠だけを重ねる
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_COLORKEY);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            STATE.with(|s| {
                *s.borrow_mut() = Some(State { hwnd: hwnd.0 as isize, origin: (vx, vy), info: None })
            });
        }
    }
}

/// 検出オーバーレイを閉じる (検出キー解放時)
pub fn hide() {
    if let Some(st) = STATE.with(|s| s.borrow_mut().take()) {
        unsafe {
            let _ = DestroyWindow(HWND(st.hwnd as *mut _));
        }
    }
}

/// 検出結果を反映して再描画する
pub fn update(info: DetectInfo) {
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            st.info = Some(info);
            unsafe {
                let _ = InvalidateRect(Some(HWND(st.hwnd as *mut _)), None, false);
            }
        }
    });
}

/// カーソル位置の検出をワーカースレッドで実行し、結果を WM_APP_DETECT で main へ送る。
/// 認識経路と同じ判定 (uia::find_text_hit / OCR帯ジオメトリ) を使う。
pub fn probe(main: isize) {
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        let mut info = DetectInfo {
            cursor: pt,
            window_rect: None,
            band_rect: None,
            uia_element: None,
            uia_lines: Vec::new(),
            hover_rect: None,
            label: String::new(),
        };
        let root = unsafe { GetAncestor(WindowFromPoint(pt), GA_ROOT) };
        let p = uia::probe_at_point(pt.x, pt.y);
        let mut kind = None;
        if !root.is_invalid() {
            let wr = capture::window_frame_rect(root);
            // 橙枠 = 実際にキャプチャされる領域 (認識経路と同じ判定を共用)
            let (rect, k) = capture_plan::plan_capture_rect(&p, &wr, pt.x, pt.y);
            info.band_rect = Some(rect);
            info.window_rect = Some(wr);
            kind = Some(k);
        }
        if let Some(text) = &p.text {
            info.uia_element = p.element_rect;
            info.uia_lines = p.line_rects;
            info.label = format!("UIA: {} | {}", p.node, crate::util::truncate_chars(text, 40));
        } else if p.hover_rect.is_some() {
            info.hover_rect = p.hover_rect;
            info.label = format!("UIA: {} (TextPatternなし)", p.node);
        } else {
            info.label = "UIA要素なし".into();
        }
        if let Some(k) = kind {
            info.label.push_str(&format!(" | 取込:{}", k.label()));
        }
        let ptr = Box::into_raw(Box::new(info)) as isize;
        unsafe {
            let _ = PostMessageW(
                Some(HWND(main as *mut _)),
                crate::app_state::WM_APP_DETECT,
                WPARAM(0),
                LPARAM(ptr),
            );
        }
    });
}

/// スクリーン座標の矩形をウィンドウ(仮想スクリーン)座標へ変換
fn to_local(r: &RECT, origin: (i32, i32)) -> RECT {
    RECT {
        left: r.left - origin.0,
        top: r.top - origin.1,
        right: r.right - origin.0,
        bottom: r.bottom - origin.1,
    }
}

/// 2px の色枠を描く
fn frame(hdc: windows::Win32::Graphics::Gdi::HDC, r: &RECT, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        FrameRect(hdc, r, brush);
        let inner = RECT {
            left: r.left + 1,
            top: r.top + 1,
            right: r.right - 1,
            bottom: r.bottom - 1,
        };
        FrameRect(hdc, &inner, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                // 黒(=カラーキー)で塗り潰し、前回の枠を消す
                let bg = CreateSolidBrush(COLORREF(0));
                FillRect(hdc, &ps.rcPaint, bg);
                let _ = DeleteObject(HGDIOBJ(bg.0));

                STATE.with(|s| {
                    let borrow = s.borrow();
                    let Some(st) = borrow.as_ref() else { return };
                    let Some(info) = st.info.as_ref() else { return };
                    let o = st.origin;

                    if let Some(r) = &info.window_rect {
                        frame(hdc, &to_local(r, o), COL_WINDOW);
                    }
                    if let Some(r) = &info.band_rect {
                        frame(hdc, &to_local(r, o), COL_BAND);
                    }
                    if let Some(r) = &info.hover_rect {
                        frame(hdc, &to_local(r, o), COL_HOVER);
                    }
                    if let Some(r) = &info.uia_element {
                        frame(hdc, &to_local(r, o), COL_ELEMENT);
                    }
                    for r in &info.uia_lines {
                        frame(hdc, &to_local(r, o), COL_LINE);
                    }

                    // カーソル近傍に説明ラベル (濃灰の下地 + 白文字)
                    if !info.label.is_empty() {
                        let font = CreateFontW(
                            -14, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0,
                            DEFAULT_CHARSET, FONT_OUTPUT_PRECISION(0), CLIP_DEFAULT_PRECIS,
                            CLEARTYPE_QUALITY, DEFAULT_PITCH.0.into(), w!("Yu Gothic UI"),
                        );
                        let old = SelectObject(hdc, HGDIOBJ(font.0));
                        let wide: Vec<u16> = info.label.encode_utf16().collect();
                        let mut size = SIZE::default();
                        let _ = GetTextExtentPoint32W(hdc, &wide, &mut size);
                        let x = info.cursor.x - o.0 + 18;
                        let y = info.cursor.y - o.1 + 22;
                        let pad = 4;
                        let bgr = RECT {
                            left: x - pad,
                            top: y - pad,
                            right: x + size.cx + pad,
                            bottom: y + size.cy + pad,
                        };
                        let lb = CreateSolidBrush(COL_LABEL_BG);
                        FillRect(hdc, &bgr, lb);
                        let _ = DeleteObject(HGDIOBJ(lb.0));
                        SetBkMode(hdc, TRANSPARENT);
                        SetTextColor(hdc, COL_LABEL_FG);
                        let _ = TextOutW(hdc, x, y, &wide);
                        SelectObject(hdc, old);
                        let _ = DeleteObject(HGDIOBJ(font.0));
                    }
                });
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
