// 結果オーバーレイ (SPEC §8, §10)
// - カーソル近傍に原文小・訳文大・エンジン切替チップをコンパクト表示
// - ピン留め時はコピー・閉じるボタンを表示
// - 余白部分は WM_NCHITTEST で HTTRANSPARENT を返し背面へクリック透過
use std::cell::RefCell;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleBitmap, CreatePen,
    CreateCompatibleDC, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CALCRECT,
    DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, DeleteDC, DeleteObject, DrawTextW,
    EndPaint, FONT_OUTPUT_PRECISION, FW_BOLD, FW_NORMAL, FillRect, FrameRect, GetDC,
    GetMonitorInfoW, HDC, HFONT, HGDIOBJ, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, PAINTSTRUCT, PS_SOLID, ReleaseDC, RoundRect, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, GetClientRect, HTCLIENT,
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
/// UIAパスノードのボタンID基点(祖先ノード最大5 + 子孫連結ノード1の範囲を確保)
pub const CHIP_UIA_NODE_BASE: usize = 200;

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
    /// 直近の認識が UIA 経路(OCR不要)で得られたか
    pub via_uia: bool,
    pub ocr_enabled: [bool; OCR_KEYS.len()],
    pub tr_enabled: [bool; TR_KEYS.len()],
    pub explanation: Option<String>,
    pub explaining: bool,
    pub error_only: bool,
    pub app_title: String,
    /// UIAパスの各ノード。クリックでOCRの代わりにそのノードのテキストを原文として採用する
    /// (末尾は末端要素の子孫テキストを連結した合成ノードの場合がある)。
    pub uia_nodes: Vec<crate::uia::UiaPathNode>,
    pub scroll_y: i32,
    /// OCR対象画像を保持しているか (「OCR対象画像」ボタンの表示条件)
    pub has_image: bool,
    /// 時間のかかる処理(再認識・再翻訳・解説取得)の実行中。
    /// true の間は閉じる以外の全チップを無効化してウィンドウ全体をロックする。
    pub busy: bool,
}

enum Item {
    Text { rect: RECT, text: String, size: i32, color: u32, bold: bool },
    Chip { rect: RECT, label: String, id: usize, active: bool, enabled: bool },
}

/// ブロック(カード)の背景。見出し・本文より下のレイヤーに描画される。
struct Panel {
    rect: RECT,
    accent: u32,
}

struct Layout {
    w: i32,
    h: i32,
    content_h: i32,
    items: Vec<Item>,
    panels: Vec<Panel>,
}

thread_local! {
    static CONTENT: RefCell<OverlayContent> = RefCell::new(OverlayContent::default());
    static LAYOUT: RefCell<Layout> = const { RefCell::new(Layout { w: 0, h: 0, content_h: 0, items: Vec::new(), panels: Vec::new() }) };
    /// マウスカーソルが乗っているチップID (✕ボタンのホバー強調に使用)
    static HOVER_ID: RefCell<Option<usize>> = const { RefCell::new(None) };
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
// ブロック(カード)の背景・枠・左アクセントバー。本体背景よりわずかに明るくして境界を分かりやすくする。
const COL_PANEL_BG: u32 = 0x002B2723;
const COL_PANEL_BORDER: u32 = 0x00423C37;
const COL_ACCENT_INFO: u32 = 0x00908A84;
const COL_ACCENT_OCR: u32 = 0x00A08F6E;
const COL_ACCENT_TR: u32 = 0x00D28C3C;
const COL_ACCENT_EXPLAIN: u32 = 0x0050C8FF;
/// UIA経路で取得した(OCR不要な)結果であることを示す見出しの色
const COL_UIA_BADGE: u32 = 0x0080D0A0;
/// 閉じる(✕)ボタンにマウスが乗っているときの背景色
const COL_CLOSE_HOVER: u32 = 0x003C3CD6;

const PAD: i32 = 12;
/// ブロック(カード)左右の余白。テキストは PAD、カード枠は少し外側に広げる。
const PANEL_MARGIN: i32 = 6;
/// カードの角丸半径
const PANEL_RADIUS: i32 = 8;
/// カード左端のアクセントバーの太さ
const ACCENT_W: i32 = 4;
const MAXW: i32 = 620;
const TIMER_AUTOHIDE: usize = 7;
const TIMER_ANIMATION: usize = 8;

// フォントサイズ (統一スケール)
const FONT_INFO: i32 = 11; // 対象アプリ情報などの補助テキスト
const FONT_CHIP: i32 = 12; // チップ(ボタン)
const FONT_HEADING: i32 = 13; // 【…】見出し・ステータス
const FONT_BODY: i32 = 17; // 原文・訳文・解説の本文

const CHIP_H: i32 = 24;
/// 右上の閉じるボタンの一辺
const CLOSE_SIZE: i32 = 20;

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
            let mut panel_spans: Vec<(i32, i32, u32)> = Vec::new();
            let mut y = PAD;
            let mut need_w = 240i32;

            if content.error_only {
                let msg = content.status.clone().unwrap_or_default();
                let (tw, th) = measure(hdc, &msg, FONT_HEADING, false, MAXW);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + tw + 4, bottom: y + th },
                    text: msg,
                    size: FONT_HEADING,
                    color: COL_STATUS,
                    bold: false,
                });
                y += th + PAD;
                need_w = need_w.max(tw + PAD * 2 + 4);
                let _ = ReleaseDC(Some(hwnd), hdc);
                return Layout { w: need_w.min(MAXW + PAD * 2), h: y, content_h: y, items, panels: Vec::new() };
            }

            // 見出し行を配置する。chips_left=true のときはチップを見出し文字の手前(左端)に、
            // false のときは従来どおり見出し文字の右側に並べる。戻り値は行の高さ。
            let heading_row = |items: &mut Vec<Item>,
                               y: i32,
                               text: &str,
                               color: u32,
                               chips: &[(&str, usize, bool)],
                               chips_left: bool,
                               need_w: &mut i32|
             -> i32 {
                if chips_left && !chips.is_empty() {
                    let mut x = PAD;
                    for (lab, id, enabled) in chips {
                        let (cw, _) = measure(hdc, lab, FONT_CHIP, false, 200);
                        let w = cw + 16;
                        items.push(Item::Chip {
                            rect: RECT { left: x, top: y - 1, right: x + w, bottom: y - 1 + CLOSE_SIZE },
                            label: lab.to_string(),
                            id: *id,
                            active: false,
                            enabled: *enabled,
                        });
                        x += w + 8;
                    }
                    let (hw, hh) = measure(hdc, text, FONT_HEADING, false, MAXW);
                    items.push(Item::Text {
                        rect: RECT { left: x, top: y, right: x + hw + 4, bottom: y + hh },
                        text: text.to_string(),
                        size: FONT_HEADING,
                        color,
                        bold: false,
                    });
                    *need_w = (*need_w).max(x + hw + PAD);
                    hh.max(CLOSE_SIZE)
                } else {
                    let (hw, hh) = measure(hdc, text, FONT_HEADING, false, MAXW);
                    items.push(Item::Text {
                        rect: RECT { left: PAD, top: y, right: PAD + hw + 4, bottom: y + hh },
                        text: text.to_string(),
                        size: FONT_HEADING,
                        color,
                        bold: false,
                    });
                    let mut x = PAD + hw + 10;
                    let row_h = hh.max(CLOSE_SIZE);
                    for (lab, id, enabled) in chips {
                        let (cw, _) = measure(hdc, lab, FONT_CHIP, false, 200);
                        let w = cw + 16;
                        items.push(Item::Chip {
                            rect: RECT { left: x, top: y - 1, right: x + w, bottom: y - 1 + CLOSE_SIZE },
                            label: lab.to_string(),
                            id: *id,
                            active: false,
                            enabled: *enabled,
                        });
                        x += w + 6;
                    }
                    *need_w = (*need_w).max(x + PAD - 6);
                    row_h
                }
            };

            let chip_row = |items: &mut Vec<Item>,
                            y: &mut i32,
                            keys: &[&str],
                            labels: &[&str],
                            cur: &str,
                            enabled: &[bool],
                            base: usize,
                            need_w: &mut i32| {
                let mut x = PAD;
                for (i, lab) in labels.iter().enumerate() {
                    let (cw, _) = measure(hdc, lab, FONT_CHIP, false, 200);
                    let w = cw + 18;
                    items.push(Item::Chip {
                        rect: RECT { left: x, top: *y, right: x + w, bottom: *y + CHIP_H },
                        label: lab.to_string(),
                        id: base + i,
                        active: keys[i] == cur,
                        enabled: enabled[i],
                    });
                    x += w + 6;
                }
                *need_w = (*need_w).max(x + PAD - 6);
                *y += CHIP_H + 6;
            };

            // 【入力内容】: 対象アプリ情報 + UIAパスノードボタン + OCR対象画像ボタン。コピーは見出しラベルの左端。
            if !content.app_title.is_empty() || content.has_image || !content.uia_nodes.is_empty() {
                let block_start = y;
                let hh = heading_row(
                    &mut items,
                    y,
                    "【入力内容】",
                    COL_LABEL,
                    &[("📋", CHIP_COPY_INFO, true)],
                    true,
                    &mut need_w,
                );
                y += hh + 4;

                if !content.app_title.is_empty() {
                    let info = format!("対象: {}", content.app_title);
                    // 右上のピン/閉じるボタンと重ならないよう幅を控える
                    let info_w = MAXW - (CLOSE_SIZE * 2 + 14);
                    let (tw, th) = measure(hdc, &info, FONT_INFO, false, info_w);
                    items.push(Item::Text {
                        rect: RECT { left: PAD, top: y, right: PAD + info_w, bottom: y + th },
                        text: info,
                        size: FONT_INFO,
                        color: COL_LABEL,
                        bold: false,
                    });
                    y += th + 4;
                    need_w = need_w.max(tw + PAD * 2 + 4);
                }

                // UIAパスの各ノードをボタン化: クリックでOCRの代わりにそのテキストを原文採用する。
                // キャプションは抽出テキストの先頭10文字程度(無ければノード識別ラベル)、
                // 現在の原文と一致するノードは選択中として強調表示する。
                if !content.uia_nodes.is_empty() {
                    let mut x = PAD;
                    for (i, node) in content.uia_nodes.iter().enumerate() {
                        if i > 0 {
                            // ノード間の区切り「＞」(パス階層であることを示す)
                            let sep = "＞";
                            let (sw, sh) = measure(hdc, sep, FONT_CHIP, false, 40);
                            let sy = y + (CHIP_H - sh) / 2;
                            items.push(Item::Text {
                                rect: RECT { left: x, top: sy, right: x + sw, bottom: sy + sh },
                                text: sep.to_string(),
                                size: FONT_CHIP,
                                color: COL_LABEL,
                                bold: false,
                            });
                            x += sw + 6;
                        }
                        let node_text = node.text.trim();
                        let has_text = !node_text.is_empty();
                        let lab = if has_text {
                            crate::util::truncate_chars(node_text, 10)
                        } else {
                            node.label.clone()
                        };
                        // maxwは単一行の実幅がそのまま返るよう十分大きく取る(ボタン内は
                        // DT_SINGLELINEで描画するため、word-wrap前提の幅で測ると文字が
                        // 欠けてしまう)。ラベル自体は10文字+省略記号までに絞っているので
                        // 幅が際限なく伸びることはない。
                        let (cw, _) = measure(hdc, &lab, FONT_CHIP, false, 600);
                        let w = cw + 18;
                        items.push(Item::Chip {
                            rect: RECT { left: x, top: y, right: x + w, bottom: y + CHIP_H },
                            label: lab,
                            id: CHIP_UIA_NODE_BASE + i,
                            active: has_text && node_text == content.source.trim(),
                            enabled: has_text,
                        });
                        x += w + 6;
                    }
                    need_w = need_w.max(x + PAD - 6);
                    y += CHIP_H + 6;
                }

                if content.has_image {
                    let lab = "OCR対象画像";
                    let (cw, _) = measure(hdc, lab, FONT_CHIP, false, 200);
                    items.push(Item::Chip {
                        rect: RECT { left: PAD, top: y, right: PAD + cw + 20, bottom: y + CHIP_H },
                        label: lab.to_string(),
                        id: CHIP_IMAGE,
                        active: false,
                        enabled: true,
                    });
                    y += CHIP_H + 4;
                }
                y += 4;
                panel_spans.push((block_start, y, COL_ACCENT_INFO));
                y += 6;
            }

            // 【OCR結果】(UIA経路の場合はOCRを行わないため専用の見出しにする): コピーは左端
            if !content.source.is_empty() {
                let block_start = y;
                let heading = if content.via_uia {
                    "【画面読み取り結果 (UIA取得)】".to_string()
                } else {
                    format!("【OCR結果 ({})】", ocr_label(&content.cur_ocr))
                };
                let heading_color = if content.via_uia { COL_UIA_BADGE } else { COL_LABEL };
                let hh = heading_row(
                    &mut items,
                    y,
                    &heading,
                    heading_color,
                    &[("📋", CHIP_COPY_SRC, true)],
                    true,
                    &mut need_w,
                );
                y += hh + 4;

                let (sw, sh) = measure(hdc, &content.source, FONT_BODY, false, MAXW);
                let text_h = sh.max(24);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + text_h },
                    text: content.source.clone(),
                    size: FONT_BODY,
                    color: COL_SRC,
                    bold: false,
                });
                y += text_h + 6;
                need_w = need_w.max(sw + PAD * 2 + 4);

                // UIA経路はOCRを行っていないため、どのOCRエンジンもアクティブ表示にしない
                let ocr_cur: &str = if content.via_uia { "" } else { content.cur_ocr.as_str() };
                chip_row(
                    &mut items,
                    &mut y,
                    &OCR_KEYS,
                    &OCR_LABELS,
                    ocr_cur,
                    &content.ocr_enabled,
                    CHIP_OCR_BASE,
                    &mut need_w,
                );
                let accent = if content.via_uia { COL_UIA_BADGE } else { COL_ACCENT_OCR };
                panel_spans.push((block_start, y, accent));
                y += 6;
            }

            // 【翻訳結果】またはステータス: コピーは左端
            if let Some(t) = &content.translation {
                let block_start = y;
                let heading = format!("【翻訳結果 ({})】", tr_label(&content.cur_tr));
                let hh = heading_row(
                    &mut items,
                    y,
                    &heading,
                    COL_LABEL,
                    &[("📋", CHIP_COPY_TR, true)],
                    true,
                    &mut need_w,
                );
                y += hh + 4;

                let (tw, th) = measure(hdc, t, FONT_BODY, true, MAXW);
                let text_h = th.max(24);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + text_h },
                    text: t.clone(),
                    size: FONT_BODY,
                    color: COL_TEXT,
                    bold: true,
                });
                y += text_h + 8;
                need_w = need_w.max(tw + PAD * 2 + 4);

                chip_row(
                    &mut items,
                    &mut y,
                    &TR_KEYS,
                    &TR_LABELS,
                    &content.cur_tr,
                    &content.tr_enabled,
                    CHIP_TR_BASE,
                    &mut need_w,
                );
                panel_spans.push((block_start, y, COL_ACCENT_TR));
                y += 6;
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
                let (tw, th) = measure(hdc, &disp, FONT_HEADING, false, MAXW);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + th },
                    text: disp,
                    size: FONT_HEADING,
                    color: COL_STATUS,
                    bold: false,
                });
                y += th + 8;
                need_w = need_w.max(tw + PAD * 2 + 4);
            }

            // バッジ (「コピーしました」等の一時通知)
            if let Some(b) = &content.badge {
                let text = format!("[{b}]");
                let (tw, th) = measure(hdc, &text, FONT_INFO, false, MAXW);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + th },
                    text,
                    size: FONT_INFO,
                    color: COL_STATUS,
                    bold: false,
                });
                y += th + 4;
                need_w = need_w.max(tw + PAD * 2 + 4);
            }

            // 操作行: 解説 / 設定 (ピン留めは右上角、閉じるは右上角、画像は入力内容、コピーは各見出し左)
            y += 2;
            let mut x = PAD;
            let ops: &[(&str, usize)] = &[("解説", CHIP_EXPLAIN), ("設定", CHIP_SETTINGS)];
            for (lab, id) in ops {
                let (cw, _) = measure(hdc, lab, FONT_CHIP, false, 200);
                let w = cw + 20;
                items.push(Item::Chip {
                    rect: RECT { left: x, top: y, right: x + w, bottom: y + CHIP_H },
                    label: lab.to_string(),
                    id: *id,
                    active: false,
                    enabled: true,
                });
                x += w + 6;
            }
            need_w = need_w.max(x + PAD - 6);
            y += CHIP_H + PAD;

            // 【解説】領域
            if content.explaining {
                let (tw, th) = measure(hdc, "解説を取得中...", FONT_HEADING, false, MAXW);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + th },
                    text: "解説を取得中...".to_string(),
                    size: FONT_HEADING,
                    color: COL_STATUS,
                    bold: false,
                });
                y += th + 8;
                need_w = need_w.max(tw + PAD * 2 + 4);
            } else if let Some(expl) = &content.explanation {
                let block_start = y;
                let hh = heading_row(
                    &mut items,
                    y,
                    "【解説】",
                    COL_LABEL,
                    &[("解説コピー", CHIP_COPY, true)],
                    false,
                    &mut need_w,
                );
                y += hh + 4;

                let (tw, th) = measure(hdc, expl, FONT_BODY, false, MAXW);
                let text_h = th.max(20);
                items.push(Item::Text {
                    rect: RECT { left: PAD, top: y, right: PAD + MAXW, bottom: y + text_h },
                    text: expl.clone(),
                    size: FONT_BODY,
                    color: COL_TEXT,
                    bold: false,
                });
                y += text_h + 8;
                need_w = need_w.max(tw + PAD * 2 + 4);
                panel_spans.push((block_start, y, COL_ACCENT_EXPLAIN));
            }

            let _ = ReleaseDC(Some(hwnd), hdc);
            let w = need_w.min(MAXW + PAD * 2);

            // 処理中は閉じる以外の全チップを無効化 (ウィンドウ全体のロック)
            if content.busy {
                for item in &mut items {
                    if let Item::Chip { id, enabled, .. } = item
                        && *id != CHIP_CLOSE
                    {
                        *enabled = false;
                    }
                }
            }

            // 右上角: ピン留めボタン(📌、常時表示・トグル状態を背景色で表示)。
            // ピン留め時のみ、その右隣に閉じる(✕)ボタンを追加する。いずれもスクロールに追従せず固定。
            let close_right = w - 6;
            let close_left = close_right - CLOSE_SIZE;
            let pin_right = if content.pinned { close_left - 4 } else { close_right };
            let pin_left = pin_right - CLOSE_SIZE;
            items.push(Item::Chip {
                rect: RECT { left: pin_left, top: 6, right: pin_right, bottom: 6 + CLOSE_SIZE },
                label: "📌".to_string(),
                id: CHIP_PIN,
                active: content.pinned,
                enabled: !content.busy,
            });
            if content.pinned {
                items.push(Item::Chip {
                    rect: RECT { left: close_left, top: 6, right: close_right, bottom: 6 + CLOSE_SIZE },
                    label: "✕".to_string(),
                    id: CHIP_CLOSE,
                    active: false,
                    enabled: true,
                });
            }

            // ブロック(カード)の背景。見出しの少し上から本文の少し下までを1枚のカードにする。
            let panels: Vec<Panel> = panel_spans
                .iter()
                .map(|(top, bottom, accent)| Panel {
                    rect: RECT { left: PANEL_MARGIN, top: top - 6, right: w - PANEL_MARGIN, bottom: bottom - 2 },
                    accent: *accent,
                })
                .collect();

            let display_h = y.min(800); // 画面に収まるように最大高さを制限
            Layout { w, h: display_h, content_h: y, items, panels }
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
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let hit = LAYOUT.with(|l| {
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
            });
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
            // ウィンドウ外に出たら WM_MOUSELEAVE を受け取れるよう登録する (✕ホバー解除用)
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
                        // 右上のピン留め・閉じるボタンはスクロールに追従しない
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

        // ブロック(カード)の背景を先に描画し、境界を見出し・本文より分かりやすくする。
        LAYOUT.with(|l| {
            for panel in &l.borrow().panels {
                let mut r = panel.rect;
                r.top -= sy;
                r.bottom -= sy;
                let panel_bg = CreateSolidBrush(COLORREF(COL_PANEL_BG));
                let panel_pen = CreatePen(PS_SOLID, 1, COLORREF(COL_PANEL_BORDER));
                let old_brush = SelectObject(mem, HGDIOBJ(panel_bg.0));
                let old_pen = SelectObject(mem, HGDIOBJ(panel_pen.0));
                let _ = RoundRect(mem, r.left, r.top, r.right, r.bottom, PANEL_RADIUS, PANEL_RADIUS);
                SelectObject(mem, old_brush);
                SelectObject(mem, old_pen);
                let _ = DeleteObject(HGDIOBJ(panel_bg.0));
                let _ = DeleteObject(HGDIOBJ(panel_pen.0));

                // 左端のアクセントバーでブロックの種類を色分けする
                let accent_rect = RECT {
                    left: r.left + 2,
                    top: r.top + 5,
                    right: r.left + 2 + ACCENT_W,
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
                    Item::Chip { rect, label, active, enabled, id } => {
                        let mut r = *rect;
                        // 右上のピン留め・閉じるボタンはスクロールに追従しない
                        let off = if *id == CHIP_CLOSE || *id == CHIP_PIN { 0 } else { sy };
                        r.top -= off;
                        r.bottom -= off;
                        let hovered = HOVER_ID.with(|h| *h.borrow() == Some(*id));
                        let bgc = if *id == CHIP_CLOSE && hovered {
                            COL_CLOSE_HOVER
                        } else if *active {
                            COL_CHIP_ACTIVE
                        } else {
                            COL_CHIP
                        };
                        let brush = CreateSolidBrush(COLORREF(bgc));
                        FillRect(mem, &r, brush);
                        let _ = DeleteObject(HGDIOBJ(brush.0));
                        let font = make_font(FONT_CHIP, *active);
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

