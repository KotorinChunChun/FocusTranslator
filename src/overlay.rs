// 結果オーバーレイ (SPEC v0.3 §3)
// - カーソル近傍に原文小・訳文大・エンジン切替チップをコンパクト表示
// - ピン留め時はコピー・閉じるボタンを表示
// - 余白部分は WM_NCHITTEST で HTTRANSPARENT を返し背面へクリック透過
// - レイアウト計算は overlay_layout モジュールに委譲
use crate::engine;
use crate::overlay_layout::{self, Item, Layout};
use std::cell::RefCell;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateCompatibleBitmap, CreatePen,
    CreateCompatibleDC, CreateSolidBrush, DT_NOPREFIX,
    DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, DeleteDC, DeleteObject, DrawTextW,
    EndPaint, FillRect, FrameRect,
    GetMonitorInfoW, HGDIOBJ, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, PAINTSTRUCT, PS_SOLID, RoundRect, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, GetClientRect, GetWindowRect,
    IsWindowVisible, HTCLIENT,
    HTTRANSPARENT, HWND_TOPMOST, IDC_ARROW, KillTimer, LoadCursorW, MA_NOACTIVATE, PostMessageW,
    RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SetTimer, SetWindowPos,
    ShowWindow, WM_LBUTTONDOWN, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT, WM_TIMER,
    WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    SendMessageW, WM_NCLBUTTONDOWN, HTCAPTION,
};
use windows::core::w;

// チップID (main へ WM_APP_CHIP で通知)
pub const CHIP_OCR_BASE: usize = 0; // 0..=4
pub const CHIP_TR_BASE: usize = 10; // 10..=13
pub const CHIP_COPY: usize = 100;
pub const CHIP_CLOSE: usize = 101;
pub const CHIP_COPY_SRC: usize = 102;
pub const CHIP_COPY_TR: usize = 103;
pub const CHIP_EXPLAIN: usize = 104;
pub const CHIP_SETTINGS: usize = 105;
pub const CHIP_PIN: usize = 106;
pub const CHIP_IMAGE: usize = 107;
pub const CHIP_COPY_INFO: usize = 108;
/// 解説(即時): 既定プロンプトを編集ダイアログ無しでそのまま送信する
pub const CHIP_EXPLAIN_QUICK: usize = 109;
/// 翻訳方向の反転 (source_lang ⇄ target_lang)
pub const CHIP_SWAP_LANG: usize = 110;
/// ログビューアを開く
pub const CHIP_OPEN_LOG: usize = 111;
/// UIAパスノードのボタンID基点(祖先ノード最大5 + 子孫連結ノード1の範囲を確保)
pub const CHIP_UIA_NODE_BASE: usize = 200;

#[derive(Default, Clone)]
pub struct OverlayContent {
    pub main_hwnd: isize,
    pub anchor: (i32, i32),
    pub source: String,
    pub translation: Option<String>,
    pub status: Option<String>,
    pub badge: Option<String>,
    pub pinned: bool,
    pub cur_ocr: String,
    pub cur_tr: String,
    /// 現在の翻訳方向 (翻訳結果ブロックの反転ボタン表示用)
    pub source_lang: String,
    pub target_lang: String,
    /// LLM翻訳時の詳細(プロファイル名とモデル名)。例: "Gemini Default gemini-3.5-flash"
    pub tr_engine_detail: Option<String>,
    /// 解説を生成するLLMの表示名 (解説結果ブロックの見出し用。例: "Gemini")
    pub explain_engine: String,
    /// 直近の認識が UIA 経路(OCR不要)で得られたか
    pub via_uia: bool,
    pub ocr_enabled: [bool; engine::OCR_KEYS.len()],
    pub tr_enabled: [bool; engine::TR_KEYS.len()],
    pub explanation: Option<String>,
    pub explaining: bool,
    pub error_only: bool,
    pub app_title: String,
    /// UIAパスの各ノード。クリックでOCRの代わりにそのノードのテキストを原文として採用する
    pub uia_nodes: Vec<crate::uia::UiaPathNode>,
    pub scroll_y: i32,
    /// OCR対象画像を保持しているか (「OCR対象画像」ボタンの表示条件)
    pub has_image: bool,
    /// 時間のかかる処理(再認識・再翻訳・解説取得)の実行中。
    pub busy: bool,
}

const TIMER_AUTOHIDE: usize = 7;
const TIMER_ANIMATION: usize = 8;

thread_local! {
    static CONTENT: RefCell<OverlayContent> = RefCell::new(OverlayContent::default());
    static LAYOUT: RefCell<Layout> = const { RefCell::new(Layout { w: 0, h: 0, content_h: 0, items: Vec::new(), panels: Vec::new() }) };
    /// マウスカーソルが乗っているチップID (✕ボタンのホバー強調に使用)
    static HOVER_ID: RefCell<Option<usize>> = const { RefCell::new(None) };
    /// 直近に表示したアンカー。同一アンカーでの再描画では実際のウィンドウ位置を維持する。
    static LAST_ANCHOR: RefCell<Option<(i32, i32)>> = const { RefCell::new(None) };
}

pub fn create(instance: windows::Win32::Foundation::HINSTANCE) -> HWND {
    unsafe {
        let class = w!("FocusTranslatorOverlay");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!("FocusTranslator"),
            WS_POPUP,
            0,
            0,
            10,
            10,
            None,
            None,
            Some(instance),
            None,
        )
        .unwrap_or_default()
    }
}

/// 内容を更新し、アンカー位置に合わせて表示する
pub fn update(hwnd: HWND, content: OverlayContent) {
    let error_only = content.error_only;
    let pinned = content.pinned;
    let anchor = content.anchor;
    let has_progress = content.status.as_deref().is_some_and(|s| s.ends_with('…'));
    CONTENT.with(|c| *c.borrow_mut() = content);

    let same_session = LAST_ANCHOR.with(|a| *a.borrow() == Some(anchor));
    let kept = if same_session {
        unsafe {
            let mut r = RECT::default();
            if IsWindowVisible(hwnd).as_bool() && GetWindowRect(hwnd, &mut r).is_ok() {
                Some((r.left, r.top))
            } else {
                None
            }
        }
    } else {
        None
    };
    let layout = CONTENT.with(|c| overlay_layout::compute_layout(hwnd, &c.borrow()));
    let (w, h) = (layout.w, layout.h);
    LAYOUT.with(|l| *l.borrow_mut() = layout);

    let (x, y) = place(anchor, w, h, kept);
    LAST_ANCHOR.with(|a| *a.borrow_mut() = Some(anchor));
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = InvalidateRect(Some(hwnd), None, true);
        if error_only && !pinned {
            SetTimer(Some(hwnd), TIMER_AUTOHIDE, 1800, None);
        } else {
            let _ = KillTimer(Some(hwnd), TIMER_AUTOHIDE);
        }
        if has_progress && !error_only {
            SetTimer(Some(hwnd), TIMER_ANIMATION, 300, None);
        } else {
            let _ = KillTimer(Some(hwnd), TIMER_ANIMATION);
        }
    }
}

pub fn hide(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
    LAST_ANCHOR.with(|a| *a.borrow_mut() = None);
}

/// 表示位置を決める
fn place(anchor: (i32, i32), w: i32, h: i32, kept: Option<(i32, i32)>) -> (i32, i32) {
    unsafe {
        let pt = POINT { x: anchor.0, y: anchor.1 };
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(hmon, &mut mi);
        let wa = mi.rcWork;

        let (mut x, mut y) = match kept {
            Some(xy) => xy,
            None => {
                let x = anchor.0 - 16;
                let mut y = anchor.1 + 28;
                if y + h > wa.bottom {
                    y = anchor.1 - h - 28;
                }
                (x, y)
            }
        };
        if x + w > wa.right {
            x = wa.right - w;
        }
        if x < wa.left {
            x = wa.left;
        }
        if y + h > wa.bottom {
            y = wa.bottom - h;
        }
        if y < wa.top {
            y = wa.top;
        }
        (x, y)
    }
}

/// チップのヒットテスト (WM_MOUSEMOVE / WM_LBUTTONDOWN で共用)
fn hit_test_chip(x: i32, y: i32) -> Option<usize> {
    LAYOUT.with(|l| {
        let sy = CONTENT.with(|c| c.borrow().scroll_y);
        l.borrow().items.iter().find_map(|it| match it {
            Item::Chip { rect, id, enabled, .. } => {
                let mut r = *rect;
                let off = if *id == CHIP_CLOSE || *id == CHIP_PIN { 0 } else { sy };
                r.top -= off;
                r.bottom -= off;
                if *enabled && x >= r.left && x < r.right && y >= r.top && y < r.bottom {
                    Some(*id)
                } else {
                    None
                }
            }
            _ => None,
        })
    })
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as i16;
            CONTENT.with(|c| {
                let mut content = c.borrow_mut();
                LAYOUT.with(|l| {
                    let layout = l.borrow();
                    let max_scroll = (layout.content_h - layout.h).max(0);
                    if max_scroll > 0 {
                        content.scroll_y -= (delta as i32) / 2;
                        if content.scroll_y < 0 { content.scroll_y = 0; }
                        if content.scroll_y > max_scroll { content.scroll_y = max_scroll; }
                        unsafe { let _ = InvalidateRect(Some(hwnd), None, true); }
                    }
                });
            });
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let hit = hit_test_chip(x, y);
            let changed = HOVER_ID.with(|h| {
                let mut h = h.borrow_mut();
                if *h != hit {
                    *h = hit;
                    true
                } else {
                    false
                }
            });
            if changed {
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, true);
                }
            }
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            let changed = HOVER_ID.with(|h| {
                let mut h = h.borrow_mut();
                if h.is_some() {
                    *h = None;
                    true
                } else {
                    false
                }
            });
            if changed {
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, true);
                }
            }
            LRESULT(0)
        }
        WM_NCHITTEST => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut rect = RECT::default();
            unsafe {
                let _ = GetClientRect(hwnd, &mut rect);
                let mut pt = POINT { x, y };
                let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt);
                if pt.x < 4 || pt.y < 4 || pt.x > rect.right - 4 || pt.y > rect.bottom - 4 {
                    return LRESULT(HTTRANSPARENT as isize);
                }
            }
            LRESULT(HTCLIENT as isize)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let hit = hit_test_chip(x, y);
            if let Some(id) = hit {
                let main = CONTENT.with(|c| c.borrow().main_hwnd);
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(main as *mut _)),
                        crate::app_state::WM_APP_CHIP,
                        WPARAM(id),
                        LPARAM(0),
                    );
                }
            } else {
                let pinned = CONTENT.with(|c| c.borrow().pinned);
                if pinned {
                    unsafe {
                        let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                        let _ = SendMessageW(
                            hwnd,
                            WM_NCLBUTTONDOWN,
                            Some(WPARAM(HTCAPTION as usize)),
                            Some(LPARAM(0)),
                        );
                    }
                }
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_AUTOHIDE {
                unsafe {
                    let _ = KillTimer(Some(hwnd), TIMER_AUTOHIDE);
                }
                hide(hwnd);
            } else if wparam.0 == TIMER_ANIMATION {
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, true);
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let w = rect.right;
        let h = rect.bottom;

        // ダブルバッファ
        let mem = CreateCompatibleDC(Some(hdc));
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let oldbmp = SelectObject(mem, HGDIOBJ(bmp.0));

        let bg = CreateSolidBrush(COLORREF(overlay_layout::COL_BG));
        FillRect(mem, &rect, bg);
        let border = CreateSolidBrush(COLORREF(overlay_layout::COL_BORDER));
        FrameRect(mem, &rect, border);
        SetBkMode(mem, TRANSPARENT);

        let sy = CONTENT.with(|c| c.borrow().scroll_y);

        // ブロック(カード)の背景を先に描画
        LAYOUT.with(|l| {
            for panel in &l.borrow().panels {
                let mut r = panel.rect;
                r.top -= sy;
                r.bottom -= sy;
                let panel_bg = CreateSolidBrush(COLORREF(overlay_layout::COL_PANEL_BG));
                let panel_pen = CreatePen(PS_SOLID, 1, COLORREF(overlay_layout::COL_PANEL_BORDER));
                let old_brush = SelectObject(mem, HGDIOBJ(panel_bg.0));
                let old_pen = SelectObject(mem, HGDIOBJ(panel_pen.0));
                let _ = RoundRect(mem, r.left, r.top, r.right, r.bottom, overlay_layout::PANEL_RADIUS, overlay_layout::PANEL_RADIUS);
                SelectObject(mem, old_brush);
                SelectObject(mem, old_pen);
                let _ = DeleteObject(HGDIOBJ(panel_bg.0));
                let _ = DeleteObject(HGDIOBJ(panel_pen.0));

                // 左端のアクセントバー
                let accent_rect = RECT {
                    left: r.left + 2,
                    top: r.top + 5,
                    right: r.left + 2 + overlay_layout::ACCENT_W,
                    bottom: r.bottom - 5,
                };
                let accent_brush = CreateSolidBrush(COLORREF(panel.accent));
                FillRect(mem, &accent_rect, accent_brush);
                let _ = DeleteObject(HGDIOBJ(accent_brush.0));
            }
        });

        LAYOUT.with(|l| {
            for item in &l.borrow().items {
                match item {
                    Item::Text { rect, text, size, color, bold } => {
                        let mut r = *rect;
                        r.top -= sy;
                        r.bottom -= sy;
                        let font = overlay_layout::make_font(*size, *bold);
                        let old = SelectObject(mem, HGDIOBJ(font.0));
                        SetTextColor(mem, COLORREF(*color));
                        let mut wide: Vec<u16> = text.encode_utf16().collect();
                        if !wide.is_empty() {
                            DrawTextW(mem, &mut wide, &mut r, DT_WORDBREAK | DT_NOPREFIX);
                        }
                        SelectObject(mem, old);
                        let _ = DeleteObject(HGDIOBJ(font.0));
                    }
                    Item::Chip { rect, label, active, enabled, id } => {
                        let mut r = *rect;
                        let off = if *id == CHIP_CLOSE || *id == CHIP_PIN { 0 } else { sy };
                        r.top -= off;
                        r.bottom -= off;
                        let hovered = HOVER_ID.with(|h| *h.borrow() == Some(*id));
                        let outlined = *id == CHIP_IMAGE;
                        let text_col = if !*enabled {
                            overlay_layout::COL_CHIP_DISABLED
                        } else if outlined {
                            overlay_layout::COL_ACCENT_INFO
                        } else {
                            overlay_layout::COL_CHIP_TEXT
                        };
                        if outlined {
                            let fill = CreateSolidBrush(COLORREF(overlay_layout::COL_PANEL_BG));
                            FillRect(mem, &r, fill);
                            let _ = DeleteObject(HGDIOBJ(fill.0));
                            let border = CreateSolidBrush(COLORREF(overlay_layout::COL_ACCENT_INFO));
                            FrameRect(mem, &r, border);
                            let _ = DeleteObject(HGDIOBJ(border.0));
                        } else {
                            let bgc = if *id == CHIP_CLOSE && hovered {
                                overlay_layout::COL_CLOSE_HOVER
                            } else if *active {
                                overlay_layout::COL_CHIP_ACTIVE
                            } else {
                                overlay_layout::COL_CHIP
                            };
                            let brush = CreateSolidBrush(COLORREF(bgc));
                            FillRect(mem, &r, brush);
                            let _ = DeleteObject(HGDIOBJ(brush.0));
                        }
                        let font = overlay_layout::make_font(overlay_layout::FONT_CHIP, *active);
                        let old = SelectObject(mem, HGDIOBJ(font.0));
                        SetTextColor(mem, COLORREF(text_col));
                        let mut wide: Vec<u16> = label.encode_utf16().collect();
                        DrawTextW(
                            mem,
                            &mut wide,
                            &mut r,
                            DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX
                                | windows::Win32::Graphics::Gdi::DT_CENTER,
                        );
                        SelectObject(mem, old);
                        let _ = DeleteObject(HGDIOBJ(font.0));
                    }
                }
            }
        });

        windows::Win32::Graphics::Gdi::BitBlt(
            hdc,
            0,
            0,
            w,
            h,
            Some(mem),
            0,
            0,
            windows::Win32::Graphics::Gdi::SRCCOPY,
        )
        .ok();

        SelectObject(mem, oldbmp);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        let _ = DeleteObject(HGDIOBJ(bg.0));
        let _ = DeleteObject(HGDIOBJ(border.0));
        let _ = EndPaint(hwnd, &ps);
    }
}

/// 現在のコンテンツを取得(コピー操作用)
pub fn current_text() -> (String, Option<String>) {
    CONTENT.with(|c| {
        let c = c.borrow();
        (c.source.clone(), c.translation.clone())
    })
}
