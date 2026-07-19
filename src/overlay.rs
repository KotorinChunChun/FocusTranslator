// 結果オーバーレイ (SPEC v0.3 §3)
// - カーソル近傍に原文小・訳文大・エンジン切替チップをコンパクト表示
// - ピン留め時はコピー・閉じるボタンを表示
// - 余白部分は WM_NCHITTEST で HTTRANSPARENT を返し背面へクリック透過
// - レイアウト計算は overlay_layout モジュールに委譲
use crate::capture::Captured;

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
/// 画像編集(SPECv0.4 §1-§4追補): 矩形/投げ輪/選択解除/選択範囲を残す/選択範囲を消す/
/// 元に戻す/編集終了。「選択範囲を残す」「選択範囲を消す」「元に戻す」は編集モードを
/// 終了せず作業中の画像だけを差し替える。OCR/翻訳の再実行は「編集終了」時のみ行う。
pub const CHIP_EDIT_RECT: usize = 112;
pub const CHIP_EDIT_LASSO: usize = 113;
/// 選択解除 (旧「リセット」): 選択中の形状のみを取り消す
pub const CHIP_EDIT_RESET: usize = 114;
/// 選択範囲を残す (旧「適用」「切り抜き」): 選択範囲でクロップし作業中画像を差し替える
pub const CHIP_EDIT_APPLY: usize = 115;
/// 編集終了 (旧「戻る」): 画像編集モード自体を終了し、変更があれば再認識する
pub const CHIP_EDIT_CANCEL: usize = 116;
/// 元に戻す: 直近の「選択範囲を残す/消す」操作を1段階巻き戻す
pub const CHIP_EDIT_UNDO: usize = 117;
/// 選択範囲を消す: 選択範囲の内側を隣接色で塗りつぶす(サイズは変わらない)
pub const CHIP_EDIT_ERASE: usize = 118;
/// インライン編集関連チップ
pub const CHIP_EDIT_SRC: usize = 119;
#[allow(dead_code)]
pub const CHIP_SAVE_SRC: usize = 120;
pub const CHIP_EDIT_TR: usize = 121;
#[allow(dead_code)]
pub const CHIP_SAVE_TR: usize = 122;
pub const CHIP_EDIT_EXP: usize = 123;
#[allow(dead_code)]
pub const CHIP_SAVE_EXP: usize = 124;
pub const CHIP_FORCE_PIN: usize = 125;
/// 選択中の文字列を読み取り結果として(再)採用する (SPECv0.5追補)。
/// 「キャプチャ画像」チップの左に配置。選択が検出できていないときは無効。
pub const CHIP_SELECTED_TEXT: usize = 126;

/// テキストのインライン編集対象ブロック
#[derive(Clone, Copy, PartialEq, Default)]
#[allow(dead_code)]
pub enum EditBlock {
    #[default]
    None,
    Source,
    Translation,
    Explanation,
}

/// 画像編集の選択ツール (SPECv0.4 §3)
#[derive(Clone, Copy, PartialEq)]
pub enum EditTool {
    Rect,
    Lasso,
}

/// 矩形選択のリサイズハンドル (角4 + 辺中央4)
#[derive(Clone, Copy, PartialEq)]
enum Handle {
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

/// レイアウト計算に渡す編集状態の要約 (実データ(画像バイト列・座標列)は overlay.rs 内に留める)
#[derive(Clone, Copy)]
pub struct EditLayoutInfo {
    pub img_w: u32,
    pub img_h: u32,
    pub tool: EditTool,
    pub has_selection: bool,
    /// マウスホイールでのズーム倍率 (1.0 = 収まる最大サイズでの等倍表示)
    pub zoom: f32,
    /// 元に戻せる履歴があるか (EditState.undo から直接算出する)
    pub has_undo: bool,
}

/// 画像編集の実データ(選択中の画像・ドラッグ中の座標)。マウス操作のたびに直接更新する。
/// 「選択範囲を残す/消す」「元に戻す」は編集セッション内で完結し、App/DB/OCRには一切
/// 触れない。「編集終了」時に初めて最終的な画像を chip_handler へ引き渡して確定する。
struct EditState {
    /// 現在の作業中画像 (選択範囲を残す/消す/元に戻すのたびに差し替わる)
    img: Arc<Captured>,
    tool: EditTool,
    dragging: bool,
    /// 矩形選択 (元画像ピクセル座標: x0,y0,x1,y1)
    rect: Option<(i32, i32, i32, i32)>,
    /// 投げ輪の軌跡 (元画像ピクセル座標)
    lasso: Vec<(i32, i32)>,
    /// ドラッグ中にヒットしたリサイズハンドル (None なら新規選択のドラッグ)
    resize_handle: Option<Handle>,
    /// マウスホイールでのズーム倍率
    zoom: f32,
    /// 「選択範囲を残す/消す」適用前の画像履歴 (DBとは別にメモリ上でのみ、最大10件)
    undo: Vec<Arc<Captured>>,
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
    /// LLM翻訳時の詳細(プロファイル名とモデル名)。例: "Gemini gemini-3.5-flash"
    pub tr_engine_detail: Option<String>,
    /// 解説を生成するLLMの表示名 (解説結果ブロックの見出し用。例: "Gemini")
    pub explain_engine: String,
    /// 直近の認識が UIA 経路(OCR不要)で得られたか
    pub via_uia: bool,
    pub ocr_keys: Vec<String>,
    pub ocr_labels: Vec<String>,
    pub ocr_enabled: Vec<bool>,
    pub cur_ocr_chip_key: String,
    pub tr_keys: Vec<String>,
    pub tr_labels: Vec<String>,
    pub tr_enabled: Vec<bool>,
    pub cur_tr_chip_key: String,
    pub explanation: Option<String>,
    pub explaining: bool,
    pub error_only: bool,
    pub app_title: String,
    /// UIAパスの各ノード。クリックでOCRの代わりにそのノードのテキストを原文として採用する
    pub uia_nodes: Vec<crate::uia::UiaPathNode>,
    pub scroll_y: i32,
    /// OCR対象画像を保持しているか (「OCR対象画像」ボタンの表示条件)
    pub has_image: bool,
    /// カーソル位置要素で選択中のテキスト (SPECv0.5追補)。
    /// 「選択中の文字列」チップの活性判定・全文ツールチップに使う。
    pub selected_text: Option<String>,
    /// 時間のかかる処理(再認識・再翻訳・解説取得)の実行中。
    pub busy: bool,
    /// 画像編集モードの要約 (overlay::update 内で EDIT の内容から自動的に設定される。
    /// 呼び出し側 (chip_handler/app_state) が明示的に設定する必要はない)
    pub edit: Option<EditLayoutInfo>,
    /// インライン編集中の対象ブロック (SPECv0.4 オーバーレイインライン編集機能)
    pub editing_block: EditBlock,
}

const TIMER_AUTOHIDE: usize = 7;
const TIMER_ANIMATION: usize = 8;
/// UIAパスチップの全文ツールチップ表示用ワンショットタイマー
const TIMER_TOOLTIP: usize = 9;
/// ホバー開始からツールチップを出すまでの遅延 (ms)
const TOOLTIP_DELAY_MS: u32 = 450;

thread_local! {
    static CONTENT: RefCell<OverlayContent> = RefCell::new(OverlayContent::default());
    static LAYOUT: RefCell<Layout> = const { RefCell::new(Layout { w: 0, h: 0, content_h: 0, items: Vec::new(), panels: Vec::new(), edit_preview: None }) };
    /// マウスカーソルが乗っているチップID (✕ボタンのホバー強調に使用)
    static HOVER_ID: RefCell<Option<usize>> = const { RefCell::new(None) };
    /// 直近のマウス座標 (クライアント座標)。ツールチップの表示位置決定に使う。
    static MOUSE_POS: RefCell<(i32, i32)> = const { RefCell::new((0, 0)) };
    /// タイマー発火待ちのツールチップ対象チップID (ホバーが変わったら破棄する)
    static TOOLTIP_PENDING: RefCell<Option<usize>> = const { RefCell::new(None) };
    /// 表示中のツールチップ (チップID, 表示位置)。None なら非表示。
    static TOOLTIP_SHOWN: RefCell<Option<(usize, i32, i32)>> = const { RefCell::new(None) };
    /// 直近に表示したアンカー。同一アンカーでの再描画では実際のウィンドウ位置を維持する。
    static LAST_ANCHOR: RefCell<Option<(i32, i32)>> = const { RefCell::new(None) };
    /// 画像編集モードの実データ (SPECv0.4 §1-§4)。None のとき編集モード無効。
    static EDIT: RefCell<Option<EditState>> = const { RefCell::new(None) };
}

/// 画像編集モードを開始する
pub fn enter_edit_mode(img: Arc<Captured>) {
    EDIT.with(|e| {
        *e.borrow_mut() = Some(EditState {
            img,
            tool: EditTool::Rect,
            dragging: false,
            rect: None,
            lasso: Vec::new(),
            resize_handle: None,
            zoom: 1.0,
            undo: Vec::new(),
        });
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
            st.resize_handle = None;
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
            st.resize_handle = None;
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

/// 直前の画像をUndo履歴へ積む (最大10件、超過分は最古を破棄)
fn push_undo(st: &mut EditState, img: Arc<Captured>) {
    st.undo.push(img);
    if st.undo.len() > 10 {
        st.undo.remove(0);
    }
}

/// 選択後の状態をクリアする共通処理 (選択形状・ドラッグ状態のリセット)
fn clear_selection_state(st: &mut EditState) {
    st.rect = None;
    st.lasso.clear();
    st.dragging = false;
    st.resize_handle = None;
}

/// 「選択範囲を残す」(旧「切り抜き」): 選択範囲でクロップし作業中画像を差し替える。
/// 編集モードは終了しない。失敗時はユーザー向けメッセージを返す。
pub fn apply_crop_keep_selection() -> Result<(), String> {
    let Some((img, sel)) = take_edit_selection() else {
        return Err("選択範囲がありません".into());
    };
    let Some(cropped) = image_edit::apply(&img, &sel) else {
        return Err("選択範囲が小さすぎます".into());
    };
    EDIT.with(|e| {
        if let Some(st) = e.borrow_mut().as_mut() {
            push_undo(st, img);
            st.img = Arc::new(cropped);
            clear_selection_state(st);
            st.zoom = 1.0; // サイズが変わるため新しい画像に合わせて再フィット
        }
    });
    Ok(())
}

/// 「選択範囲を消す」: 選択範囲の内側を隣接色で塗りつぶす(サイズは変わらない)。
/// 編集モードは終了しない。失敗時はユーザー向けメッセージを返す。
pub fn erase_selection_action() -> Result<(), String> {
    let Some((img, sel)) = take_edit_selection() else {
        return Err("選択範囲がありません".into());
    };
    let Some(erased) = image_edit::erase_selection(&img, &sel) else {
        return Err("選択範囲が小さすぎます".into());
    };
    EDIT.with(|e| {
        if let Some(st) = e.borrow_mut().as_mut() {
            push_undo(st, img);
            st.img = Arc::new(erased);
            clear_selection_state(st);
            // サイズは変わらないためズーム(表示位置)は維持する
        }
    });
    Ok(())
}

/// 直近の「選択範囲を残す/消す」を1段階巻き戻す。履歴が無ければ何もせず false を返す。
pub fn undo_edit() -> bool {
    EDIT.with(|e| {
        let mut g = e.borrow_mut();
        let Some(st) = g.as_mut() else { return false };
        let Some(prev) = st.undo.pop() else { return false };
        st.img = prev;
        clear_selection_state(st);
        true
    })
}

/// 編集セッションを終了する。編集(選択範囲を残す/消す)が1度でも行われていれば
/// 最終的な作業中画像を返す(呼び出し側はこれをApp/DBへ確定し再認識する)。
/// 何も変更されていなければ None を返す(単なるキャンセルとして扱う)。
pub fn finish_edit_session() -> Option<Arc<Captured>> {
    let result = EDIT.with(|e| {
        e.borrow()
            .as_ref()
            .and_then(|st| if st.undo.is_empty() { None } else { Some(st.img.clone()) })
    });
    exit_edit_mode();
    result
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
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | windows::Win32::UI::WindowsAndMessaging::WS_EX_TRANSPARENT,
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
        ).unwrap_or_default();
        hwnd
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
            zoom: st.zoom,
            has_undo: !st.undo.is_empty(),
        })
    });

    let (_cur_block, has_progress, error_only, pinned) = CONTENT.with(|c| {
        let c = c.borrow();
        (c.editing_block, c.explaining || c.busy || (c.has_image && c.app_title.is_empty()), c.error_only, c.pinned)
    });
    
    let anchor = content.anchor;
    CONTENT.with(|c| *c.borrow_mut() = content);
    // UIAパスノードの並びが変わりうるため、古いチップIDに対するツールチップ状態は破棄する
    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_TOOLTIP);
    }
    TOOLTIP_PENDING.with(|t| *t.borrow_mut() = None);
    TOOLTIP_SHOWN.with(|t| *t.borrow_mut() = None);

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
        // プロンプト編集ウィンドウ/テキスト編集ダイアログが開いている間は、
        // オーバーレイの再TOPMOST化でそれらを覆い隠さないよう、その直下に留める。
        let insert_after = if crate::input_dialog::is_open() {
            crate::input_dialog::hwnd()
        } else if crate::prompt_edit::is_open() {
            crate::prompt_edit::hwnd()
        } else {
            HWND_TOPMOST
        };
        let _ = SetWindowPos(hwnd, Some(insert_after), x, y, w, h, SWP_NOACTIVATE);
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
    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_TOOLTIP);
    }
    TOOLTIP_PENDING.with(|t| *t.borrow_mut() = None);
    TOOLTIP_SHOWN.with(|t| *t.borrow_mut() = None);
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
            | CHIP_EDIT_APPLY | CHIP_EDIT_CANCEL | CHIP_EDIT_UNDO | CHIP_EDIT_ERASE
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

/// 矩形選択の8ハンドル(角4+辺中央4)の画面座標一覧を返す
fn handle_points(rect: RECT, scale: f32, sel: (i32, i32, i32, i32)) -> [(Handle, i32, i32); 8] {
    let (x0, y0, x1, y1) = sel;
    let to_screen = |ix: i32, iy: i32| {
        (rect.left + (ix as f32 * scale).round() as i32, rect.top + (iy as f32 * scale).round() as i32)
    };
    let (sx0, sy0) = to_screen(x0, y0);
    let (sx1, sy1) = to_screen(x1, y1);
    let mx = (sx0 + sx1) / 2;
    let my = (sy0 + sy1) / 2;
    [
        (Handle::NW, sx0, sy0),
        (Handle::N, mx, sy0),
        (Handle::NE, sx1, sy0),
        (Handle::W, sx0, my),
        (Handle::E, sx1, my),
        (Handle::SW, sx0, sy1),
        (Handle::S, mx, sy1),
        (Handle::SE, sx1, sy1),
    ]
}

/// 画面座標(x,y)がいずれかのリサイズハンドル上にあるかを判定する(矩形選択が確定している場合のみ)
fn hit_test_handle(x: i32, y: i32) -> Option<Handle> {
    const TOL: i32 = 7;
    EDIT.with(|e| {
        let g = e.borrow();
        let st = g.as_ref()?;
        if st.tool != EditTool::Rect {
            return None;
        }
        let sel = st.rect?;
        let (rect, scale) = LAYOUT.with(|l| l.borrow().edit_preview)?;
        handle_points(rect, scale, sel)
            .into_iter()
            .find(|(_, hx, hy)| (x - hx).abs() <= TOL && (y - hy).abs() <= TOL)
            .map(|(h, _, _)| h)
    })
}

/// クライアント領域のドラッグでウィンドウ全体を移動させる (タイトルバー相当の挙動)。
fn begin_window_drag(hwnd: HWND) {
    unsafe {
        let _ = ReleaseCapture();
        let _ = SendMessageW(
            hwnd,
            WM_NCLBUTTONDOWN,
            Some(WPARAM(HTCAPTION as usize)),
            Some(LPARAM(0)),
        );
    }
}

/// 画像編集プレビュー上のマウス押下でドラッグを開始する。既存の矩形選択のハンドル上なら
/// その辺/角のリサイズ、それ以外のプレビュー内なら新規選択のドラッグを始める。
/// ドラッグを開始したら true (呼び出し側で SetCapture・再描画する)。
fn begin_edit_drag(x: i32, y: i32) -> bool {
    if let Some(handle) = hit_test_handle(x, y) {
        EDIT.with(|e| {
            if let Some(st) = e.borrow_mut().as_mut() {
                st.dragging = true;
                st.resize_handle = Some(handle);
            }
        });
        return true;
    }
    if let Some((ix, iy)) = edit_preview_hit(x, y) {
        EDIT.with(|e| {
            if let Some(st) = e.borrow_mut().as_mut() {
                st.dragging = true;
                st.resize_handle = None;
                match st.tool {
                    EditTool::Rect => st.rect = Some((ix, iy, ix, iy)),
                    EditTool::Lasso => {
                        st.lasso.clear();
                        st.lasso.push((ix, iy));
                    }
                }
            }
        });
        return true;
    }
    false
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as i16;
            // WM_MOUSEWHEEL の座標はスクリーン座標なのでクライアント座標へ変換する
            let sx = (lparam.0 & 0xFFFF) as i16 as i32;
            let sy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut pt = POINT { x: sx, y: sy };
            unsafe {
                let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt);
            }
            let over_preview = is_editing_image()
                && LAYOUT.with(|l| {
                    l.borrow()
                        .edit_preview
                        .map(|(r, _)| pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom)
                        .unwrap_or(false)
                });
            if over_preview {
                // 画像編集プレビュー上: マウスホイールでズーム (SPECv0.4追補)
                EDIT.with(|e| {
                    if let Some(st) = e.borrow_mut().as_mut() {
                        let notches = delta as f32 / 120.0;
                        let factor = 1.15f32.powf(notches);
                        st.zoom = (st.zoom * factor).clamp(0.2, 8.0);
                    }
                });
                refresh(hwnd);
            } else {
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
            }
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            MOUSE_POS.with(|p| *p.borrow_mut() = (x, y));
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
            // ホバー対象が変わったらツールチップの表示/待機タイマーを立て直す。
            // UIAパスチップ(全文が省略表示されうる)のときのみ遅延後に表示する。
            if changed {
                unsafe {
                    let _ = KillTimer(Some(hwnd), TIMER_TOOLTIP);
                }
                let had_shown = TOOLTIP_SHOWN.with(|t| t.borrow_mut().take().is_some());
                match hit {
                    Some(id) if id >= CHIP_UIA_NODE_BASE || id == CHIP_SELECTED_TEXT => {
                        TOOLTIP_PENDING.with(|t| *t.borrow_mut() = Some(id));
                        unsafe {
                            SetTimer(Some(hwnd), TIMER_TOOLTIP, TOOLTIP_DELAY_MS, None);
                        }
                    }
                    _ => {
                        TOOLTIP_PENDING.with(|t| *t.borrow_mut() = None);
                    }
                }
                if had_shown {
                    unsafe {
                        let _ = InvalidateRect(Some(hwnd), None, true);
                    }
                }
            }
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
                let handle = st.resize_handle;
                match st.tool {
                    EditTool::Rect => {
                        if let Some(r) = &mut st.rect {
                            // ハンドルドラッグ中は対応する辺/角のみ更新し、新規ドラッグ中は終点を更新する
                            // (SPECv0.4追補: 画像の四方からのトリミング)
                            match handle {
                                Some(Handle::N) => r.1 = iy,
                                Some(Handle::S) => r.3 = iy,
                                Some(Handle::E) => r.2 = ix,
                                Some(Handle::W) => r.0 = ix,
                                Some(Handle::NE) => { r.1 = iy; r.2 = ix; }
                                Some(Handle::NW) => { r.0 = ix; r.1 = iy; }
                                Some(Handle::SE) => { r.2 = ix; r.3 = iy; }
                                Some(Handle::SW) => { r.0 = ix; r.3 = iy; }
                                None => { r.2 = ix; r.3 = iy; }
                            }
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
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_TOOLTIP);
            }
            TOOLTIP_PENDING.with(|t| *t.borrow_mut() = None);
            let had_tooltip = TOOLTIP_SHOWN.with(|t| t.borrow_mut().take().is_some());
            if changed || had_tooltip {
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
                // 画像編集中: プレビュー/ハンドル上ならドラッグ選択を開始する。
                // それ以外の領域は通常どおりウィンドウ移動に回す。
                if begin_edit_drag(x, y) {
                    unsafe {
                        SetCapture(hwnd);
                        let _ = InvalidateRect(Some(hwnd), None, true);
                    }
                } else {
                    begin_window_drag(hwnd);
                }
            } else {
                // ウィンドウの移動を開始した時点でピン留めする
                let main = CONTENT.with(|c| c.borrow().main_hwnd);
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(main as *mut _)),
                        crate::app_state::WM_APP_CHIP,
                        WPARAM(CHIP_FORCE_PIN),
                        LPARAM(0),
                    );
                }
                begin_window_drag(hwnd);
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
                st.resize_handle = None;
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
            } else if wparam.0 == TIMER_TOOLTIP {
                unsafe {
                    let _ = KillTimer(Some(hwnd), TIMER_TOOLTIP);
                }
                let pending = TOOLTIP_PENDING.with(|t| *t.borrow());
                // 待機開始後もまだ同じチップにホバーしたままなら表示する
                if let Some(id) = pending
                    && HOVER_ID.with(|h| *h.borrow()) == Some(id)
                {
                    let (mx, my) = MOUSE_POS.with(|p| *p.borrow());
                    TOOLTIP_SHOWN.with(|t| *t.borrow_mut() = Some((id, mx, my)));
                    unsafe {
                        let _ = InvalidateRect(Some(hwnd), None, true);
                    }
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

        let th = overlay_layout::theme();
        let bg = CreateSolidBrush(COLORREF(th.bg));
        FillRect(mem, &rect, bg);
        let border = CreateSolidBrush(COLORREF(th.border));
        FrameRect(mem, &rect, border);
        SetBkMode(mem, TRANSPARENT);

        let sy = CONTENT.with(|c| c.borrow().scroll_y);

        // ブロック(カード)の背景を先に描画
        LAYOUT.with(|l| {
            for panel in &l.borrow().panels {
                let mut r = panel.rect;
                r.top -= sy;
                r.bottom -= sy;
                let panel_bg = CreateSolidBrush(COLORREF(th.panel_bg));
                let panel_pen = CreatePen(PS_SOLID, 1, COLORREF(th.panel_border));
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
                        let hovered = *enabled && HOVER_ID.with(|h| *h.borrow() == Some(*id));
                        let outlined = *id == CHIP_IMAGE;
                        // 展開中(画像編集モード中)は文字と背景の色を反転して「開いている」ことを示す
                        let inverted = outlined && *active;
                        let text_col = if !*enabled {
                            th.chip_disabled
                        } else if inverted {
                            th.panel_bg
                        } else if outlined {
                            th.accent_info
                        } else if *active {
                            th.chip_active_text
                        } else {
                            th.chip_text
                        };
                        if outlined {
                            let fill_col = if inverted {
                                th.accent_info
                            } else if hovered {
                                th.chip_hover
                            } else {
                                th.panel_bg
                            };
                            let fill = CreateSolidBrush(COLORREF(fill_col));
                            FillRect(mem, &r, fill);
                            let _ = DeleteObject(HGDIOBJ(fill.0));
                            let border = CreateSolidBrush(COLORREF(th.accent_info));
                            FrameRect(mem, &r, border);
                            let _ = DeleteObject(HGDIOBJ(border.0));
                        } else {
                            // 全チップ共通: ホバー中はボタンの色を変えてフォーカス可視化する
                            let bgc = if *id == CHIP_CLOSE && hovered {
                                th.close_hover
                            } else if *active && hovered {
                                th.chip_active_hover
                            } else if *active {
                                th.chip_active
                            } else if hovered {
                                th.chip_hover
                            } else {
                                th.chip
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

        draw_tooltip(mem, w, h);

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

/// UIAパスチップの全文ツールチップ。ホバー中のチップの省略前テキストを
/// マウス位置の近くにポップアップ表示する (SPECv0.5追補: 長いUIAテキストの視認性向上)。
const TOOLTIP_MAX_W: i32 = 360;
const TOOLTIP_PAD: i32 = 8;

fn draw_tooltip(mem: HDC, win_w: i32, win_h: i32) {
    let shown = TOOLTIP_SHOWN.with(|t| *t.borrow());
    let Some((id, mx, my)) = shown else { return };
    let text = CONTENT.with(|c| {
        let c = c.borrow();
        if id == CHIP_SELECTED_TEXT {
            c.selected_text.clone()
        } else {
            c.uia_nodes.get(id.wrapping_sub(CHIP_UIA_NODE_BASE)).map(|n| n.text.clone())
        }
    });
    let Some(text) = text else { return };
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    unsafe {
        let th = overlay_layout::theme();
        let (tw, th_h) = overlay_layout::measure(mem, text, overlay_layout::FONT_INFO, false, TOOLTIP_MAX_W);
        let box_w = tw + TOOLTIP_PAD * 2;
        let box_h = th_h + TOOLTIP_PAD * 2;

        let mut left = mx + 12;
        let mut top = my + 20;
        if left + box_w > win_w - 4 {
            left = (win_w - 4 - box_w).max(4);
        }
        if top + box_h > win_h - 4 {
            // 下に収まらなければカーソルの上へ出す
            top = (my - 8 - box_h).max(4);
        }
        let r = RECT { left, top, right: left + box_w, bottom: top + box_h };

        let bg = CreateSolidBrush(COLORREF(th.panel_bg));
        FillRect(mem, &r, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));
        let border = CreatePen(PS_SOLID, 1, COLORREF(th.panel_border));
        let old_pen = SelectObject(mem, HGDIOBJ(border.0));
        let old_brush = SelectObject(mem, HGDIOBJ(windows::Win32::Graphics::Gdi::GetStockObject(windows::Win32::Graphics::Gdi::NULL_BRUSH).0));
        let _ = windows::Win32::Graphics::Gdi::Rectangle(mem, r.left, r.top, r.right, r.bottom);
        SelectObject(mem, old_brush);
        SelectObject(mem, old_pen);
        let _ = DeleteObject(HGDIOBJ(border.0));

        let mut text_r = RECT { left: r.left + TOOLTIP_PAD, top: r.top + TOOLTIP_PAD, right: r.right - TOOLTIP_PAD, bottom: r.bottom - TOOLTIP_PAD };
        let font = overlay_layout::make_font(overlay_layout::FONT_INFO, false);
        let old_font = SelectObject(mem, HGDIOBJ(font.0));
        SetTextColor(mem, COLORREF(th.text));
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        DrawTextW(mem, &mut wide, &mut text_r, DT_WORDBREAK | DT_NOPREFIX);
        SelectObject(mem, old_font);
        let _ = DeleteObject(HGDIOBJ(font.0));
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
                    if let Some(sel @ (x0, y0, x1, y1)) = st.rect {
                        let pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]
                            .map(to_screen);
                        let _ = Polyline(mem, &pts);

                        // 四方+四隅のリサイズハンドル (SPECv0.4追補: 画像の四方からのトリミング)
                        const HS: i32 = 4;
                        let handle_brush = CreateSolidBrush(COLORREF(0x0050C8FF));
                        for (_, hx, hy) in handle_points(preview, scale, sel) {
                            let hr = RECT { left: hx - HS, top: hy - HS, right: hx + HS, bottom: hy + HS };
                            FillRect(mem, &hr, handle_brush);
                        }
                        let _ = DeleteObject(HGDIOBJ(handle_brush.0));
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
