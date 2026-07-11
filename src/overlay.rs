// 結果オーバーレイ (SPEC v0.3 §3)
// - カーソル近傍に原文小・訳文大・エンジン切替チップをコンパクト表示
// - ピン留め時はコピー・閉じるボタンを表示
// - 余白部分は WM_NCHITTEST で HTTRANSPARENT を返し背面へクリック透過
// - レイアウト計算は overlay_layout モジュールに委譲
use crate::capture::Captured;
use crate::engine;
use crate::image_edit;
use crate::overlay_layout::{self, Item, Layout};
use std::cell::RefCell;
use std::sync::Arc;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, CreateCompatibleBitmap, CreatePen,
    CreateCompatibleDC, CreateSolidBrush, DIB_RGB_COLORS, DT_NOPREFIX,
    DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, DeleteDC, DeleteObject, DrawTextW,
    EndPaint, FillRect, FrameRect, HALFTONE, HDC,
    GetMonitorInfoW, HGDIOBJ, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, PAINTSTRUCT, PS_SOLID, Polyline, RoundRect, SelectObject, SetBkMode,
    SetStretchBltMode, SetTextColor, StretchDIBits, TRANSPARENT,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, GetClientRect, GetWindowRect,
    IsWindowVisible, HTCLIENT,
    HTTRANSPARENT, HWND_TOPMOST, IDC_ARROW, KillTimer, LoadCursorW, MA_NOACTIVATE, PostMessageW,
    RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SetTimer, SetWindowPos,
    ShowWindow, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_NCHITTEST,
    WM_PAINT, WM_TIMER,
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
/// 画像編集(SPECv0.4 §1-§4): 矩形/投げ輪/リセット/適用/戻る
pub const CHIP_EDIT_RECT: usize = 112;
pub const CHIP_EDIT_LASSO: usize = 113;
pub const CHIP_EDIT_RESET: usize = 114;
pub const CHIP_EDIT_APPLY: usize = 115;
pub const CHIP_EDIT_CANCEL: usize = 116;

/// 画像編集の選択ツール (SPECv0.4 §3)
#[derive(Clone, Copy, PartialEq)]
pub enum EditTool {
    Rect,
    Lasso,
}

/// レイアウト計算に渡す編集状態の要約 (実データ(画像バイト列・座標列)は overlay.rs 内に留める)
#[derive(Clone, Copy)]
pub struct EditLayoutInfo {
    pub img_w: u32,
    pub img_h: u32,
    pub tool: EditTool,
    pub has_selection: bool,
}

/// 画像編集の実データ(選択中の画像・ドラッグ中の座標)。マウス操作のたびに直接更新する。
struct EditState {
    img: Arc<Captured>,
    tool: EditTool,
    dragging: bool,
    /// 矩形選択 (元画像ピクセル座標: x0,y0,x1,y1)
    rect: Option<(i32, i32, i32, i32)>,
    /// 投げ輪の軌跡 (元画像ピクセル座標)
    lasso: Vec<(i32, i32)>,
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
    /// 画像編集モードの要約 (overlay::update 内で EDIT の内容から自動的に設定される。
    /// 呼び出し側 (chip_handler/app_state) が明示的に設定する必要はない)
    pub edit: Option<EditLayoutInfo>,
}

const TIMER_AUTOHIDE: usize = 7;
const TIMER_ANIMATION: usize = 8;

thread_local! {
    static CONTENT: RefCell<OverlayContent> = RefCell::new(OverlayContent::default());
    static LAYOUT: RefCell<Layout> = const { RefCell::new(Layout { w: 0, h: 0, content_h: 0, items: Vec::new(), panels: Vec::new(), edit_preview: None }) };
    /// マウスカーソルが乗っているチップID (✕ボタンのホバー強調に使用)
    static HOVER_ID: RefCell<Option<usize>> = const { RefCell::new(None) };
    /// 直近に表示したアンカー。同一アンカーでの再描画では実際のウィンドウ位置を維持する。
    static LAST_ANCHOR: RefCell<Option<(i32, i32)>> = const { RefCell::new(None) };
    /// 画像編集モードの実データ (SPECv0.4 §1-§4)。None のとき編集モード無効。
    static EDIT: RefCell<Option<EditState>> = const { RefCell::new(None) };
}

/// 画像編集モードを開始する
pub fn enter_edit_mode(img: Arc<Captured>) {
    EDIT.with(|e| {
        *e.borrow_mut() = Some(EditState { img, tool: EditTool::Rect, dragging: false, rect: None, lasso: Vec::new() });
    });
}

/// 画像編集モードを終了する
pub fn exit_edit_mode() {
    EDIT.with(|e| *e.borrow_mut() = None);
}

pub fn is_editing_image() -> bool {
    EDIT.with(|e| e.borrow().is_some())
}

/// 選択ツールを切り替える。切り替え時は選択中の形状を破棄する。
pub fn set_edit_tool(tool: EditTool) {
    EDIT.with(|e| {
        if let Some(st) = e.borrow_mut().as_mut() {
            st.tool = tool;
            st.rect = None;
            st.lasso.clear();
            st.dragging = false;
        }
    });
}

/// 選択中の形状をリセットする(ツールは維持)
pub fn reset_edit_selection() {
    EDIT.with(|e| {
        if let Some(st) = e.borrow_mut().as_mut() {
            st.rect = None;
            st.lasso.clear();
            st.dragging = false;
        }
    });
}

/// 確定した選択を取り出す(適用時に使用)。選択が無効なら None。
pub fn take_edit_selection() -> Option<(Arc<Captured>, image_edit::Selection)> {
    EDIT.with(|e| {
        let g = e.borrow();
        let st = g.as_ref()?;
        match st.tool {
            EditTool::Rect => {
                let (x0, y0, x1, y1) = st.rect?;
                Some((st.img.clone(), image_edit::Selection::Rect { x0, y0, x1, y1 }))
            }
            EditTool::Lasso => {
                if st.lasso.len() < 3 {
                    None
                } else {
                    Some((st.img.clone(), image_edit::Selection::Lasso(st.lasso.clone())))
                }
            }
        }
    })
}

/// 直近の CONTENT で再度 update() を呼び、レイアウトと表示を更新する
/// (chip_handler を介さずに overlay.rs 内のマウス操作から直接呼べる軽量な再描画)
pub fn refresh(hwnd: HWND) {
    let content = CONTENT.with(|c| c.borrow().clone());
    update(hwnd, content);
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
pub fn update(hwnd: HWND, mut content: OverlayContent) {
    // 画像編集モードの実データは EDIT (overlay.rs内) が唯一の情報源。
    // 呼び出し側が古いスナップショットで content.edit を上書きしないよう、ここで都度差し替える。
    content.edit = EDIT.with(|e| {
        e.borrow().as_ref().map(|st| EditLayoutInfo {
            img_w: st.img.width,
            img_h: st.img.height,
            tool: st.tool,
            has_selection: match st.tool {
                EditTool::Rect => st.rect.is_some(),
                EditTool::Lasso => st.lasso.len() >= 3,
            },
        })
    });
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

/// スクロールオフセットの影響を受けない固定チップ (右上の✕/📌、および編集パネルのチップ)
fn is_fixed_chip(id: usize) -> bool {
    matches!(
        id,
        CHIP_CLOSE | CHIP_PIN | CHIP_EDIT_RECT | CHIP_EDIT_LASSO | CHIP_EDIT_RESET
            | CHIP_EDIT_APPLY | CHIP_EDIT_CANCEL
    )
}

/// チップのヒットテスト (WM_MOUSEMOVE / WM_LBUTTONDOWN で共用)
fn hit_test_chip(x: i32, y: i32) -> Option<usize> {
    LAYOUT.with(|l| {
        let sy = CONTENT.with(|c| c.borrow().scroll_y);
        l.borrow().items.iter().find_map(|it| match it {
            Item::Chip { rect, id, enabled, .. } => {
                let mut r = *rect;
                let off = if is_fixed_chip(*id) { 0 } else { sy };
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

/// クリック開始位置が編集プレビュー矩形内かどうかを判定し、元画像ピクセル座標を返す
fn edit_preview_hit(x: i32, y: i32) -> Option<(i32, i32)> {
    let (rect, scale) = LAYOUT.with(|l| l.borrow().edit_preview)?;
    if x < rect.left || x >= rect.right || y < rect.top || y >= rect.bottom {
        return None;
    }
    edit_preview_map_clamped(x, y, rect, scale)
}

/// 画面座標を元画像ピクセル座標へ逆変換する(ドラッグ継続中はプレビュー外に出ても画像端にクランプする)
fn edit_preview_map_clamped(x: i32, y: i32, rect: RECT, scale: f32) -> Option<(i32, i32)> {
    let ix = ((x - rect.left) as f32 / scale).round() as i32;
    let iy = ((y - rect.top) as f32 / scale).round() as i32;
    EDIT.with(|e| {
        let g = e.borrow();
        let st = g.as_ref()?;
        Some((ix.clamp(0, st.img.width as i32), iy.clamp(0, st.img.height as i32)))
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
            // 画像編集: ドラッグ中は矩形の終点更新/投げ輪の軌跡追加のみ行う(レイアウト再計算はしない)
            let dragged = EDIT.with(|e| {
                let mut g = e.borrow_mut();
                let Some(st) = g.as_mut() else { return false };
                if !st.dragging {
                    return false;
                }
                let Some((rect, scale)) = LAYOUT.with(|l| l.borrow().edit_preview) else { return false };
                let ix = ((x - rect.left) as f32 / scale).round() as i32;
                let iy = ((y - rect.top) as f32 / scale).round() as i32;
                let ix = ix.clamp(0, st.img.width as i32);
                let iy = iy.clamp(0, st.img.height as i32);
                match st.tool {
                    EditTool::Rect => {
                        if let Some(r) = &mut st.rect {
                            r.2 = ix;
                            r.3 = iy;
                        }
                    }
                    EditTool::Lasso => st.lasso.push((ix, iy)),
                }
                true
            });
            if changed || dragged {
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
            } else if is_editing_image() {
                // 画像編集中: プレビュー内クリックでドラッグ選択を開始する(ウィンドウ移動は行わない)
                if let Some((ix, iy)) = edit_preview_hit(x, y) {
                    EDIT.with(|e| {
                        if let Some(st) = e.borrow_mut().as_mut() {
                            st.dragging = true;
                            match st.tool {
                                EditTool::Rect => st.rect = Some((ix, iy, ix, iy)),
                                EditTool::Lasso => {
                                    st.lasso.clear();
                                    st.lasso.push((ix, iy));
                                }
                            }
                        }
                    });
                    unsafe {
                        SetCapture(hwnd);
                        let _ = InvalidateRect(Some(hwnd), None, true);
                    }
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
        WM_LBUTTONUP => {
            let ended = EDIT.with(|e| {
                let mut g = e.borrow_mut();
                let Some(st) = g.as_mut() else { return false };
                if !st.dragging {
                    return false;
                }
                st.dragging = false;
                // 誤操作防止: 極小の矩形/点未満の投げ輪は選択なし扱いにする
                if let EditTool::Rect = st.tool
                    && let Some((x0, y0, x1, y1)) = st.rect
                    && (x1 - x0).abs() < 4 && (y1 - y0).abs() < 4
                {
                    st.rect = None;
                }
                if let EditTool::Lasso = st.tool
                    && st.lasso.len() < 3
                {
                    st.lasso.clear();
                }
                true
            });
            if ended {
                unsafe {
                    let _ = ReleaseCapture();
                }
                refresh(hwnd);
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
                        let off = if is_fixed_chip(*id) { 0 } else { sy };
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

        LAYOUT.with(|l| {
            if let Some((rect, scale)) = l.borrow().edit_preview {
                draw_edit_preview(mem, rect, scale);
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

/// 編集中の画像プレビューと選択中の矩形/投げ輪の枠線・軌跡を描画する (SPECv0.4 §2.2, §3.3)
fn draw_edit_preview(mem: HDC, preview: RECT, scale: f32) {
    EDIT.with(|e| {
        let g = e.borrow();
        let Some(st) = g.as_ref() else { return };
        unsafe {
            let iw = st.img.width;
            let ih = st.img.height;
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: iw as i32,
                    biHeight: -(ih as i32), // トップダウン
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            SetStretchBltMode(mem, HALFTONE);
            StretchDIBits(
                mem,
                preview.left,
                preview.top,
                preview.right - preview.left,
                preview.bottom - preview.top,
                0,
                0,
                iw as i32,
                ih as i32,
                Some(st.img.bgra.as_ptr() as *const _),
                &bmi,
                DIB_RGB_COLORS,
                windows::Win32::Graphics::Gdi::SRCCOPY,
            );

            let to_screen = |p: (i32, i32)| POINT {
                x: preview.left + (p.0 as f32 * scale).round() as i32,
                y: preview.top + (p.1 as f32 * scale).round() as i32,
            };

            let pen = CreatePen(PS_SOLID, 2, COLORREF(0x0050C8FF));
            let old_pen = SelectObject(mem, HGDIOBJ(pen.0));
            match st.tool {
                EditTool::Rect => {
                    if let Some((x0, y0, x1, y1)) = st.rect {
                        let pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]
                            .map(to_screen);
                        let _ = Polyline(mem, &pts);
                    }
                }
                EditTool::Lasso => {
                    if st.lasso.len() >= 2 {
                        let mut pts: Vec<POINT> = st.lasso.iter().map(|&p| to_screen(p)).collect();
                        if !st.dragging && pts.len() >= 3 {
                            pts.push(pts[0]); // 確定後は閉じた多角形として描画
                        }
                        let _ = Polyline(mem, &pts);
                    }
                }
            }
            SelectObject(mem, old_pen);
            let _ = DeleteObject(HGDIOBJ(pen.0));
        }
    });
}

/// 現在のコンテンツを取得(コピー操作用)
pub fn current_text() -> (String, Option<String>) {
    CONTENT.with(|c| {
        let c = c.borrow();
        (c.source.clone(), c.translation.clone())
    })
}
