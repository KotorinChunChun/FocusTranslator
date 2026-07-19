// キャプチャ領域計画 (SPEC v0.3)
// UIA検出結果からOCRキャプチャ矩形を決定するロジック。
// 認識ワーカー (worker.rs) と領域検出モード (detect.rs) の両方で共用する。
use crate::capture::{self, Captured};
use crate::ocr;
use crate::uia;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow};

/// ホールド認識で切り出すカーソル周辺帯のサイズ (px)。領域検出モードの枠表示と共用。
pub const BAND_W: i32 = 1200;
pub const BAND_H: i32 = 160;
/// UIA矩形とカーソルY座標の許容距離 (px)。これ以上離れた矩形は採用しない。
const NEAR_Y_PX: i32 = 10;
/// 直下要素(TextPatternなし)をそのままキャプチャする高さの上限 (px)。
/// これより高い要素は段落帯 (要素幅 × カーソル中心 PARA_BAND_H) に切り替える。
const HOVER_MAX_H: i32 = 320;
/// 段落帯の高さ (px)。段落全体を拾えるよう既定帯より上下に広くとる。
const PARA_BAND_H: i32 = 320;
/// 採用した矩形に付ける余白 (px)。文字の欠けを防ぐ。
const CAP_PAD: i32 = 6;
/// OCRへ渡す画像の最小高さ (px)。UIA直下要素の矩形 (例: 1行のEDIT) はこれより
/// 低いことがあり、OneOCRは極端に低い画像で実行自体に失敗することがあるため、
/// カーソル位置を中心に上下へ広げてこの高さを確保する。
const MIN_OCR_H: i32 = 48;

/// キャプチャ矩形の由来 (plan_capture_rect の決定結果)
#[derive(Clone, Copy, PartialEq)]
pub enum CapKind {
    /// UIA行矩形(黄)の統合
    Line,
    /// TextPattern要素(緑)
    Element,
    /// 直下要素(紫)
    Hover,
    /// 背の高い直下要素内の段落帯 (OCRは段落モード)
    HoverBand,
    /// 既定のカーソル中心帯(橙)
    Band,
}

impl CapKind {
    /// 領域検出モードのラベル表示用
    pub fn label(&self) -> &'static str {
        match self {
            CapKind::Line => "UIA行",
            CapKind::Element => "UIA要素",
            CapKind::Hover => "直下要素",
            CapKind::HoverBand => "段落帯",
            CapKind::Band => "既定帯",
        }
    }
}

/// 矩形とY座標の垂直距離 (内側なら0)
fn v_dist(r: &RECT, y: i32) -> i32 {
    (r.top - y).max(y - r.bottom).max(0)
}

/// 幅・高さが最低限あるか
fn rect_valid(r: &RECT) -> bool {
    r.right - r.left >= 8 && r.bottom - r.top >= 4
}

/// 複数矩形を1つの外接矩形へ統合
fn merge_rects(rects: &[RECT]) -> RECT {
    let mut m = rects[0];
    for r in &rects[1..] {
        m.left = m.left.min(r.left);
        m.top = m.top.min(r.top);
        m.right = m.right.max(r.right);
        m.bottom = m.bottom.max(r.bottom);
    }
    m
}

/// 余白を付けてウィンドウ矩形へクランプし、極端に低い矩形はOCR失敗を避けるため
/// 最小高さまで(カーソルYを中心に)上下へ広げる。
fn pad_clamp(r: &RECT, win: &RECT) -> RECT {
    let padded = RECT {
        left: (r.left - CAP_PAD).max(win.left),
        top: (r.top - CAP_PAD).max(win.top),
        right: (r.right + CAP_PAD).min(win.right),
        bottom: (r.bottom + CAP_PAD).min(win.bottom),
    };
    let h = padded.bottom - padded.top;
    if h >= MIN_OCR_H || h <= 0 {
        return padded;
    }
    let cy = (padded.top + padded.bottom) / 2;
    let half = MIN_OCR_H / 2;
    let mut top = cy - half;
    let mut bottom = cy + half;
    if top < win.top {
        bottom += win.top - top;
        top = win.top;
    }
    if bottom > win.bottom {
        top -= bottom - win.bottom;
        bottom = win.bottom;
    }
    RECT { left: padded.left, top: top.max(win.top), right: padded.right, bottom: bottom.min(win.bottom) }
}

/// UIA検出結果からキャプチャすべきスクリーン矩形を決める。
/// 優先: UIA行矩形(統合) → TextPattern要素 → 直下要素(高すぎる場合は段落帯) → 既定帯。
/// カーソルYから NEAR_Y_PX 以上離れた矩形は誤検出とみなして採用しない。
/// 領域検出モード(detect)の枠表示と実際のキャプチャで共用する。
pub fn plan_capture_rect(p: &uia::UiaProbe, win: &RECT, x: i32, y: i32) -> (RECT, CapKind) {
    // 黄: 行矩形群を1つの長方形に統合
    if !p.line_rects.is_empty() {
        let merged = merge_rects(&p.line_rects);
        if rect_valid(&merged) && v_dist(&merged, y) < NEAR_Y_PX {
            return (pad_clamp(&merged, win), CapKind::Line);
        }
    }
    // 緑: TextPattern が見つかった要素
    if let Some(r) = &p.element_rect
        && rect_valid(r) && v_dist(r, y) < NEAR_Y_PX {
            return (pad_clamp(r, win), CapKind::Element);
        }
    // 紫: 直下要素。高すぎる場合はカーソル位置の段落を狙った帯へ切り替える
    if let Some(r) = &p.hover_rect
        && rect_valid(r) && v_dist(r, y) < NEAR_Y_PX {
            if r.bottom - r.top > HOVER_MAX_H {
                let band = RECT {
                    left: r.left,
                    top: (y - PARA_BAND_H / 2).max(r.top),
                    right: r.right,
                    bottom: (y + PARA_BAND_H / 2).min(r.bottom),
                };
                return (pad_clamp(&band, win), CapKind::HoverBand);
            }
            return (pad_clamp(r, win), CapKind::Hover);
        }
    // 橙: 既定のカーソル中心帯
    (capture::band_screen_rect(win, x, y, BAND_W, BAND_H), CapKind::Band)
}

/// キャプチャ由来に応じたOCRの行選択モード
pub fn focus_for(kind: CapKind, fy: f32) -> ocr::Focus {
    match kind {
        CapKind::HoverBand => ocr::Focus::Paragraph(fy),
        _ => ocr::Focus::Line(fy),
    }
}

/// キャプチャ結果の中間データ (画像 + フォーカスY座標)
pub struct Band {
    pub img: Captured,
    pub focus_y: f32,
}

/// 対象ウィンドウをキャプチャし、スクリーン座標の矩形 sr を切り出す。
/// focus_y は切り出し後画像内でのカーソルYを返す。
pub fn capture_screen_rect(target: isize, sr: &RECT, y: i32) -> Result<Band, String> {
    let hwnd = HWND(target as *mut _);
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() || IsIconic(hwnd).as_bool() {
            return Err("このウィンドウは取得できません".into());
        }
    }
    let full = capture::capture_window(hwnd)?;
    let r = capture::window_frame_rect(hwnd);
    let rw = (r.right - r.left).max(1);
    let rh = (r.bottom - r.top).max(1);
    let scale_x = full.width as f32 / rw as f32;
    let scale_y = full.height as f32 / rh as f32;
    let left = ((sr.left - r.left) as f32 * scale_x) as i32;
    let top = ((sr.top - r.top) as f32 * scale_y) as i32;
    let w = ((sr.right - sr.left) as f32 * scale_x) as i32;
    let h = ((sr.bottom - sr.top) as f32 * scale_y) as i32;
    let band = capture::crop(&full, left, top, w, h).ok_or("このウィンドウは取得できません")?;
    let focus_y = ((y - sr.top.max(r.top)) as f32 * scale_y).max(0.0);
    Ok(Band { img: band, focus_y })
}

/// ポインタ直下ウィンドウをキャプチャし、カーソル周辺の帯を切り出す (SPEC §6.3)
pub fn capture_band(x: i32, y: i32, target: isize, bw: i32, bh: i32) -> Result<Band, String> {
    let win = capture::window_frame_rect(HWND(target as *mut _));
    let sr = capture::band_screen_rect(&win, x, y, bw, bh);
    capture_screen_rect(target, &sr, y)
}

/// UIA検出結果に基づいてキャプチャ領域を決めて切り出す (黄/緑/紫 → 既定帯の順)
pub fn capture_probe(x: i32, y: i32, target: isize, probe: &uia::UiaProbe) -> Result<(Band, CapKind), String> {
    let win = capture::window_frame_rect(HWND(target as *mut _));
    let (rect, kind) = plan_capture_rect(probe, &win, x, y);
    capture_screen_rect(target, &rect, y).map(|b| (b, kind))
}
