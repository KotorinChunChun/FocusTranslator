// 結果オーバーレイ (SPEC §8, §10)
// - カーソル近傍に原文小・訳文大・エンジン切替チップをコンパクト表示
// - ピン留め時はコピー・閉じるボタンを表示
// - 余白部分は WM_NCHITTEST で HTTRANSPARENT を返し背面へクリック透過
use std::cell::RefCell;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CALCRECT,
    DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, DeleteDC, DeleteObject, DrawTextW,
    EndPaint, FONT_OUTPUT_PRECISION, FW_BOLD, FW_NORMAL, FillRect, FrameRect, GetDC,
    GetMonitorInfoW, HDC, HFONT, HGDIOBJ, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, PAINTSTRUCT, ReleaseDC, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, GetClientRect, HTCLIENT,
    HTTRANSPARENT, HWND_TOPMOST, IDC_ARROW, KillTimer, LoadCursorW, MA_NOACTIVATE, PostMessageW,
    RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SetTimer, SetWindowPos,
    ShowWindow, WM_LBUTTONDOWN, WM_MOUSEACTIVATE, WM_NCHITTEST, WM_PAINT, WM_TIMER, WNDCLASSW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
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

pub const OCR_KEYS: [&str; 5] = ["win", "paddle", "yomitoku", "ndl", "llm"];
pub const OCR_LABELS: [&str; 5] = ["Win", "Paddle", "YomiToku", "NDL", "LLM(統合)"];
pub const TR_KEYS: [&str; 4] = ["local", "deepl", "google", "llm"];
pub const TR_LABELS: [&str; 4] = ["ローカル", "DeepL", "Google", "LLM"];

pub fn ocr_label(key: &str) -> &'static str {
    OCR_KEYS.iter().position(|k| *k == key).map(|i| OCR_LABELS[i]).unwrap_or("Win")
}
pub fn tr_label(key: &str) -> &'static str {
    TR_KEYS.iter().position(|k| *k == key).map(|i| TR_LABELS[i]).unwrap_or("ローカル")
}

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
    pub ocr_enabled: [bool; OCR_KEYS.len()],
    pub tr_enabled: [bool; TR_KEYS.len()],
    pub explanation: Option<String>,
    pub explaining: bool,
    pub error_only: bool,
    pub app_title: String,
    pub uia_path: String,
    pub scroll_y: i32,
}

enum Item {
    Text { rect: RECT, text: String, size: i32, color: u32, bold: bool },
    Chip { rect: RECT, label: String, id: usize, active: bool, enabled: bool },
}

struct Layout {
    w: i32,
    h: i32,
    content_h: i32,
    items: Vec<Item>,
}

thread_local! {
    static CONTENT: RefCell<OverlayContent> = RefCell::new(OverlayContent::default());
    static LAYOUT: RefCell<Layout> = const { RefCell::new(Layout { w: 0, h: 0, content_h: 0, items: Vec::new() }) };
}

// 配色 (COLORREF は 0x00BBGGRR)
const COL_BG: u32 = 0x00221E1C;
const COL_BORDER: u32 = 0x00524A46;
const COL_TEXT: u32 = 0x00F0EEEC;
const COL_SRC: u32 = 0x00B4AFAA;
const COL_STATUS: u32 = 0x0050C8FF;
const COL_CHIP: u32 = 0x003F3833;
const COL_CHIP_ACTIVE: u32 = 0x00D28C3C;
const COL_CHIP_TEXT: u32 = 0x00E8E4E0;
const COL_CHIP_DISABLED: u32 = 0x00787068;
const COL_LABEL: u32 = 0x00908A84;

const PAD: i32 = 12;
const MAXW: i32 = 620;
const TIMER_AUTOHIDE: usize = 7;
const TIMER_ANIMATION: usize = 8;

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
    let has_status = content.status.is_some();
    CONTENT.with(|c| *c.borrow_mut() = content);
    let layout = compute_layout(hwnd);
    let (w, h) = (layout.w, layout.h);
    LAYOUT.with(|l| *l.borrow_mut() = layout);

    // 画面下端では上側に表示 (SPEC §10)
    let (x, y) = place(anchor, w, h);
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = InvalidateRect(Some(hwnd), None, true);
        if error_only && !pinned {
            SetTimer(Some(hwnd), TIMER_AUTOHIDE, 1800, None);
        } else {
            let _ = KillTimer(Some(hwnd), TIMER_AUTOHIDE);
        }
        if has_status && !error_only {
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
}

fn place(anchor: (i32, i32), w: i32, h: i32) -> (i32, i32) {
    unsafe {
        let pt = POINT { x: anchor.0, y: anchor.1 };
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(hmon, &mut mi);
        let wa = mi.rcWork;
        let mut x = anchor.0 - 16;
        let mut y = anchor.1 + 28;
        if y + h > wa.bottom {
            y = anchor.1 - h - 28;
        }
        if y < wa.top {
            y = wa.top;
        }
        if x + w > wa.right {
            x = wa.right - w;
        }
        if x < wa.left {
            x = wa.left;
        }
        (x, y)
    }
}

fn make_font(size: i32, bold: bool) -> HFONT {
    unsafe {
        CreateFontW(
            -size,
            0,
            0,
            0,
            if bold { FW_BOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
            0,
            0,
            0,
            DEFAULT_CHARSET,
            FONT_OUTPUT_PRECISION(0),
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0.into(),
            w!("Yu Gothic UI"),
        )
    }
}

fn measure(hdc: HDC, text: &str, size: i32, bold: bool, maxw: i32) -> (i32, i32) {
    unsafe {
        let font = make_font(size, bold);
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        if wide.is_empty() {
            wide.push(' ' as u16);
        }
        let mut r = RECT { left: 0, top: 0, right: maxw, bottom: 0 };
        DrawTextW(hdc, &mut wide, &mut r, DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX);
        SelectObject(hdc, old);
        let _ = DeleteObject(HGDIOBJ(font.0));
        (r.right - r.left, r.bottom - r.top)
    }
}

fn compute_layout(hwnd: HWND) -> Layout {
    CONTENT.with(|c| {
        let content = c.borrow();
        unsafe {
            let hdc = GetDC(Some(hwnd));
            let mut items: Vec<Item> = Vec::new();
            let mut y = PAD;
            let mut need_w = 240i32;

            if content.error_only {
                let msg = content.status.clone().unwrap_or_default();
                let (tw, th) = measure(hdc, &msg, 14, false, MAXW);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + tw + 4, bottom: y + th },
                    text: msg,
                    size: 14,
                    color: COL_STATUS,
                    bold: false,
                });
                y += th + PAD;
                need_w = need_w.max(tw + PAD * 2 + 4);
                let _ = ReleaseDC(Some(hwnd), hdc);
                return Layout { w: need_w.min(MAXW + PAD * 2), h: y, content_h: y, items };
            }

            // 対象アプリ情報
            if !content.app_title.is_empty() {
                let mut info = format!("対象: {}", content.app_title);
                if !content.uia_path.is_empty() {
                    info.push_str("\r\nパス: ");
                    info.push_str(&content.uia_path);
                }
                if let Some(b) = &content.badge {
                    info.push_str(&format!("\r\n[{b}]"));
                }
                let (tw, th) = measure(hdc, &info, 11, false, MAXW);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + th },
                    text: info,
                    size: 11,
                    color: COL_LABEL,
                    bold: false,
                });
                y += th + 6;
                need_w = need_w.max(tw + PAD * 2 + 4);
            }

            let chip_h = 24;
            let row = |items: &mut Vec<Item>,
                           y: &mut i32,
                           keys: &[&str],
                           labels: &[&str],
                           cur: &str,
                           enabled: &[bool],
                           base: usize,
                           need_w: &mut i32| {
                let mut x = PAD;
                for (i, lab) in labels.iter().enumerate() {
                    let (cw, _) = measure(hdc, lab, 12, false, 200);
                    let w = cw + 18;
                    items.push(Item::Chip {
                        rect: RECT { left: x, top: *y, right: x + w, bottom: *y + chip_h },
                        label: lab.to_string(),
                        id: base + i,
                        active: keys[i] == cur,
                        enabled: enabled[i],
                    });
                    x += w + 6;
                }
                *need_w = (*need_w).max(x + PAD - 6);
                *y += chip_h + 6;
            };

            let copy_w = 28;

            // OCR結果ブロック
            if !content.source.is_empty() {
                let heading = format!("【OCR結果 ({})】", ocr_label(&content.cur_ocr));
                let (hw, hh) = measure(hdc, &heading, 13, false, MAXW);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + hw + 4, bottom: y + hh },
                    text: heading,
                    size: 13,
                    color: COL_LABEL,
                    bold: false,
                });
                y += hh + 4;

                let (sw, sh) = measure(hdc, &content.source, 17, false, MAXW - copy_w - 6);
                let text_h = sh.max(24);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + MAXW - copy_w - 6, bottom: y + text_h },
                    text: content.source.clone(),
                    size: 17,
                    color: COL_SRC,
                    bold: false,
                });
                items.push(Item::Chip {
                    rect: RECT { left: PAD + MAXW - copy_w, top: y, right: PAD + MAXW, bottom: y + 24 },
                    label: "📋".to_string(),
                    id: CHIP_COPY_SRC,
                    active: false,
                    enabled: true,
                });
                y += text_h + 6;
                need_w = need_w.max(sw + copy_w + 6 + PAD * 2 + 4);

                row(
                    &mut items,
                    &mut y,
                    &OCR_KEYS,
                    &OCR_LABELS,
                    &content.cur_ocr,
                    &content.ocr_enabled,
                    CHIP_OCR_BASE,
                    &mut need_w,
                );
            }

            // 翻訳結果またはステータスブロック
            if let Some(t) = &content.translation {
                let heading = format!("【翻訳結果 ({})】", tr_label(&content.cur_tr));
                let (hw, hh) = measure(hdc, &heading, 13, false, MAXW);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + hw + 4, bottom: y + hh },
                    text: heading,
                    size: 13,
                    color: COL_LABEL,
                    bold: false,
                });
                y += hh + 4;

                let (tw, th) = measure(hdc, t, 17, true, MAXW - copy_w - 6);
                let text_h = th.max(24);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + MAXW - copy_w - 6, bottom: y + text_h },
                    text: t.clone(),
                    size: 17,
                    color: COL_TEXT,
                    bold: true,
                });
                items.push(Item::Chip {
                    rect: RECT { left: PAD + MAXW - copy_w, top: y, right: PAD + MAXW, bottom: y + 24 },
                    label: "📋".to_string(),
                    id: CHIP_COPY_TR,
                    active: false,
                    enabled: true,
                });
                y += text_h + 8;
                need_w = need_w.max(tw + copy_w + 6 + PAD * 2 + 4);

                row(
                    &mut items,
                    &mut y,
                    &TR_KEYS,
                    &TR_LABELS,
                    &content.cur_tr,
                    &content.tr_enabled,
                    CHIP_TR_BASE,
                    &mut need_w,
                );
            } else if let Some(s) = &content.status {
                let mut disp = s.clone();
                if !content.error_only {
                    let millis = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis();
                    let count = (millis / 300) % 4;
                    disp = disp.replace("…", "");
                    disp.push_str(&".".repeat(count as usize));
                }
                let (tw, th) = measure(hdc, &disp, 13, false, MAXW);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + th },
                    text: disp,
                    size: 13,
                    color: COL_STATUS,
                    bold: false,
                });
                y += th + 8;
                need_w = need_w.max(tw + PAD * 2 + 4);
            }

            // 操作行: ピン留め / 画像 / コピー / 解説 / 設定 / 閉じる
            y += 2;
            let mut x = PAD;
            let ops: &[(&str, usize)] = if content.pinned {
                &[("ピン解除", CHIP_PIN), ("画像", CHIP_IMAGE), ("コピー", CHIP_COPY), ("解説", CHIP_EXPLAIN), ("設定", CHIP_SETTINGS), ("閉じる", CHIP_CLOSE)]
            } else {
                &[("ピン留め", CHIP_PIN), ("画像", CHIP_IMAGE), ("コピー", CHIP_COPY), ("解説", CHIP_EXPLAIN), ("設定", CHIP_SETTINGS)]
            };
            for (lab, id) in ops {
                let (cw, _) = measure(hdc, lab, 12, false, 200);
                let w = cw + 20;
                items.push(Item::Chip {
                    rect: RECT { left: x, top: y, right: x + w, bottom: y + chip_h },
                    label: lab.to_string(),
                    id: *id,
                    active: false,
                    enabled: true,
                });
                x += w + 6;
            }
            need_w = need_w.max(x + PAD - 6);
            y += chip_h + PAD;

            // 解説領域
            if content.explaining {
                let (tw, th) = measure(hdc, "解説を取得中...", 13, false, MAXW);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + th },
                    text: "解説を取得中...".to_string(),
                    size: 13,
                    color: COL_STATUS,
                    bold: false,
                });
                y += th + 8;
                need_w = need_w.max(tw + PAD * 2 + 4);
            } else if let Some(expl) = &content.explanation {
                y += 4;
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + 2 },
                    text: "---".to_string(),
                    size: 10,
                    color: COL_BORDER,
                    bold: false,
                });
                y += 8;

                let (tw, th) = measure(hdc, expl, 13, false, MAXW);
                let text_h = th.max(20);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + text_h },
                    text: expl.clone(),
                    size: 13,
                    color: COL_TEXT,
                    bold: false,
                });
                y += text_h + 8;
                need_w = need_w.max(tw + PAD * 2 + 4);
            }

            let _ = ReleaseDC(Some(hwnd), hdc);
            let display_h = y.min(800); // 画面に収まるように最大高さを制限
            Layout { w: need_w.min(MAXW + PAD * 2), h: display_h, content_h: y, items }
        }
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
        WM_NCHITTEST => {
            // 外周4pxは背面へクリック透過 (SPEC §10 部分ヒットテスト)
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
            let hit = LAYOUT.with(|l| {
                let sy = CONTENT.with(|c| c.borrow().scroll_y);
                l.borrow().items.iter().find_map(|it| match it {
                    Item::Chip { rect, id, enabled, .. } => {
                        let mut r = *rect;
                        r.top -= sy;
                        r.bottom -= sy;
                        if *enabled && x >= r.left && x < r.right && y >= r.top && y < r.bottom {
                            Some(*id)
                        } else {
                            None
                        }
                    }
                    _ => None,
                })
            });
            if let Some(id) = hit {
                let main = CONTENT.with(|c| c.borrow().main_hwnd);
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(main as *mut _)),
                        crate::WM_APP_CHIP,
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

        let bg = CreateSolidBrush(COLORREF(COL_BG));
        FillRect(mem, &rect, bg);
        let border = CreateSolidBrush(COLORREF(COL_BORDER));
        FrameRect(mem, &rect, border);
        SetBkMode(mem, TRANSPARENT);

        let sy = CONTENT.with(|c| c.borrow().scroll_y);
        LAYOUT.with(|l| {
            for item in &l.borrow().items {
                match item {
                    Item::Text { rect, text, size, color, bold } => {
                        let mut r = *rect;
                        r.top -= sy;
                        r.bottom -= sy;
                        let font = make_font(*size, *bold);
                        let old = SelectObject(mem, HGDIOBJ(font.0));
                        SetTextColor(mem, COLORREF(*color));
                        let mut wide: Vec<u16> = text.encode_utf16().collect();
                        if !wide.is_empty() {
                            DrawTextW(mem, &mut wide, &mut r, DT_WORDBREAK | DT_NOPREFIX);
                        }
                        SelectObject(mem, old);
                        let _ = DeleteObject(HGDIOBJ(font.0));
                    }
                    Item::Chip { rect, label, active, enabled, .. } => {
                        let mut r = *rect;
                        r.top -= sy;
                        r.bottom -= sy;
                        let bgc = if *active { COL_CHIP_ACTIVE } else { COL_CHIP };
                        let brush = CreateSolidBrush(COLORREF(bgc));
                        FillRect(mem, &r, brush);
                        let _ = DeleteObject(HGDIOBJ(brush.0));
                        let font = make_font(12, *active);
                        let old = SelectObject(mem, HGDIOBJ(font.0));
                        SetTextColor(
                            mem,
                            COLORREF(if *enabled { COL_CHIP_TEXT } else { COL_CHIP_DISABLED }),
                        );
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

