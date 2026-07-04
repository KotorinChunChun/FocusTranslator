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
};
use windows::core::w;

// チップID (main へ WM_APP_CHIP で通知)
pub const CHIP_OCR_BASE: usize = 0; // 0..=4
pub const CHIP_TR_BASE: usize = 10; // 10..=13
pub const CHIP_COPY: usize = 100;
pub const CHIP_CLOSE: usize = 101;

pub const OCR_KEYS: [&str; 5] = ["win", "paddle", "yomitoku", "ndl", "gemini"];
pub const OCR_LABELS: [&str; 5] = ["Win", "Paddle", "YomiToku", "NDL", "Gemini"];
pub const TR_KEYS: [&str; 4] = ["local", "deepl", "google", "gemini"];
pub const TR_LABELS: [&str; 4] = ["ローカル", "DeepL", "Google", "Gemini"];

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
    pub ocr_enabled: [bool; 5],
    pub tr_enabled: [bool; 4],
    /// エラーのみの短時間表示(チップなし・自動クローズ)
    pub error_only: bool,
}

enum Item {
    Text { rect: RECT, text: String, size: i32, color: u32, bold: bool },
    Chip { rect: RECT, label: String, id: usize, active: bool, enabled: bool },
}

struct Layout {
    w: i32,
    h: i32,
    items: Vec<Item>,
}

thread_local! {
    static CONTENT: RefCell<OverlayContent> = RefCell::new(OverlayContent::default());
    static LAYOUT: RefCell<Layout> = const { RefCell::new(Layout { w: 0, h: 0, items: Vec::new() }) };
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
                return Layout { w: need_w.min(MAXW + PAD * 2), h: y, items };
            }

            // ヘッダ: 現在エンジン + バッジ
            let mut header = format!(
                "OCR: {} / 翻訳: {}",
                ocr_label(&content.cur_ocr),
                tr_label(&content.cur_tr)
            );
            if let Some(b) = &content.badge {
                header.push_str(&format!("  [{b}]"));
            }
            let (hw, hh) = measure(hdc, &header, 12, false, MAXW);
            items.push(Item::Text {
                rect: RECT { left: PAD, top: y, right: PAD + hw + 4, bottom: y + hh },
                text: header,
                size: 12,
                color: COL_LABEL,
                bold: false,
            });
            y += hh + 6;

            // 原文(小)
            if !content.source.is_empty() {
                let (sw, sh) = measure(hdc, &content.source, 13, false, MAXW);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + sh },
                    text: content.source.clone(),
                    size: 13,
                    color: COL_SRC,
                    bold: false,
                });
                y += sh + 6;
                need_w = need_w.max(sw + PAD * 2 + 4);
            }

            // 訳文(大)または状態表示
            if let Some(t) = &content.translation {
                let (tw, th) = measure(hdc, t, 17, true, MAXW);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + th },
                    text: t.clone(),
                    size: 17,
                    color: COL_TEXT,
                    bold: true,
                });
                y += th + 8;
                need_w = need_w.max(tw + PAD * 2 + 4);
            } else if let Some(s) = &content.status {
                let (tw, th) = measure(hdc, s, 13, false, MAXW);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + th },
                    text: s.clone(),
                    size: 13,
                    color: COL_STATUS,
                    bold: false,
                });
                y += th + 8;
                need_w = need_w.max(tw + PAD * 2 + 4);
            }

            // チップ行 (OCR / 翻訳)
            let chip_h = 24;
            let row = |items: &mut Vec<Item>,
                           y: &mut i32,
                           label: &str,
                           keys: &[&str],
                           labels: &[&str],
                           cur: &str,
                           enabled: &[bool],
                           base: usize,
                           need_w: &mut i32| {
                let (lw, _lh) = measure(hdc, label, 12, false, 200);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: *y + 5, right: PAD + lw + 4, bottom: *y + chip_h },
                    text: label.to_string(),
                    size: 12,
                    color: COL_LABEL,
                    bold: false,
                });
                let mut x = PAD + lw + 10;
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

            row(
                &mut items,
                &mut y,
                "OCR:",
                &OCR_KEYS,
                &OCR_LABELS,
                &content.cur_ocr,
                &content.ocr_enabled,
                CHIP_OCR_BASE,
                &mut need_w,
            );
            row(
                &mut items,
                &mut y,
                "翻訳:",
                &TR_KEYS,
                &TR_LABELS,
                &content.cur_tr,
                &content.tr_enabled,
                CHIP_TR_BASE,
                &mut need_w,
            );

            // 操作行: コピー / 閉じる(ピン留め時)
            y += 2;
            let mut x = PAD;
            let ops: &[(&str, usize)] = if content.pinned {
                &[("コピー", CHIP_COPY), ("閉じる", CHIP_CLOSE)]
            } else {
                &[("コピー", CHIP_COPY)]
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

            let _ = ReleaseDC(Some(hwnd), hdc);
            Layout { w: need_w.min(MAXW + PAD * 2), h: y, items }
        }
    })
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
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
                l.borrow().items.iter().find_map(|it| match it {
                    Item::Chip { rect, id, enabled, .. }
                        if *enabled
                            && x >= rect.left
                            && x < rect.right
                            && y >= rect.top
                            && y < rect.bottom =>
                    {
                        Some(*id)
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
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_AUTOHIDE {
                unsafe {
                    let _ = KillTimer(Some(hwnd), TIMER_AUTOHIDE);
                }
                hide(hwnd);
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

        LAYOUT.with(|l| {
            for item in &l.borrow().items {
                match item {
                    Item::Text { rect, text, size, color, bold } => {
                        let font = make_font(*size, *bold);
                        let old = SelectObject(mem, HGDIOBJ(font.0));
                        SetTextColor(mem, COLORREF(*color));
                        let mut wide: Vec<u16> = text.encode_utf16().collect();
                        if !wide.is_empty() {
                            let mut r = *rect;
                            DrawTextW(mem, &mut wide, &mut r, DT_WORDBREAK | DT_NOPREFIX);
                        }
                        SelectObject(mem, old);
                        let _ = DeleteObject(HGDIOBJ(font.0));
                    }
                    Item::Chip { rect, label, active, enabled, .. } => {
                        let bgc = if *active { COL_CHIP_ACTIVE } else { COL_CHIP };
                        let brush = CreateSolidBrush(COLORREF(bgc));
                        FillRect(mem, rect, brush);
                        let _ = DeleteObject(HGDIOBJ(brush.0));
                        let font = make_font(12, *active);
                        let old = SelectObject(mem, HGDIOBJ(font.0));
                        SetTextColor(
                            mem,
                            COLORREF(if *enabled { COL_CHIP_TEXT } else { COL_CHIP_DISABLED }),
                        );
                        let mut wide: Vec<u16> = label.encode_utf16().collect();
                        let mut r = *rect;
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

