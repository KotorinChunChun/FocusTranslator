// プレビューウィンドウ: ログビューアから画像をクリックしたときに開く原寸表示ウィンドウ。
// クリック&ドラッグでパンニング、マウスホイールで拡大縮小(デフォルト100%)、
// 矢印キーでのスクロールも併用できる。画像範囲外へはパン/スクロールできない。
use std::cell::RefCell;
use windows::Win32::Foundation::{
    COLORREF, HANDLE, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, COLOR_BTNFACE, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateSolidBrush, DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint,
    FillRect, GetMonitorInfoW, HALFTONE, HBRUSH, HDC, HGDIOBJ, InvalidateRect,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, PAINTSTRUCT, ScreenToClient,
    SelectObject, SetStretchBltMode, StretchDIBits,
};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_DIB;
use windows::Win32::UI::Controls::SetScrollInfo;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, VK_C, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_LEFT,
    VK_RIGHT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetScrollInfo, GetWindowRect,
    IDC_ARROW, IsWindow, LoadCursorW, RegisterClassW, SB_HORZ, SB_LINEDOWN, SB_LINEUP,
    SB_PAGEDOWN, SB_PAGEUP, SB_THUMBPOSITION, SB_THUMBTRACK, SB_TOP, SB_VERT, SCROLLINFO,
    SIF_ALL, SIF_TRACKPOS, SW_SHOW, SWP_NOACTIVATE, SetForegroundWindow, SetWindowPos,
    SetWindowTextW, ShowWindow, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_HSCROLL, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_SIZE, WM_VSCROLL,
    WNDCLASSW, WS_EX_TOPMOST, WS_HSCROLL, WS_OVERLAPPEDWINDOW, WS_VSCROLL,
};
use windows::core::w;

/// プレビューウィンドウで開く画像の種別
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum ImgKind {
    /// OCR対象画像 (crop_rect による赤枠は表示しない)
    Ocr,
    /// 対象アプリ全体画像 (crop_rect があれば赤枠を表示する)
    Full,
}

const MIN_ZOOM: f64 = 0.1;
const MAX_ZOOM: f64 = 8.0;

/// クリック&ドラッグ中のパン状態: (開始マウス座標, 開始スクロール量)
type PanState = ((i32, i32), (i32, i32));

thread_local! {
    static IMG: RefCell<Option<(u32, u32, Vec<u8>)>> = const { RefCell::new(None) };
    static SCROLL: RefCell<(i32, i32)> = const { RefCell::new((0, 0)) };
    static ZOOM: RefCell<f64> = const { RefCell::new(1.0) };
    static REGISTERED: RefCell<bool> = const { RefCell::new(false) };
    static PREVIEW_HWND: RefCell<Option<isize>> = const { RefCell::new(None) };
    /// 全体画像表示時、OCR抽出範囲を示す赤枠 (x, y, w, h / 画像内の物理ピクセル座標)。
    /// OCR対象画像の表示時は None (SPECv0.5.2追補)。
    static BOX_RECT: RefCell<Option<(i32, i32, i32, i32)>> = const { RefCell::new(None) };
    static PANNING: RefCell<Option<PanState>> = const { RefCell::new(None) };
}

/// 矩形の縁を width px の赤枠で囲う(4本の細い塗り潰しで描く。OCR抽出範囲の可視化用。
/// SPECv0.5.2追補)。
pub(crate) fn draw_red_box(hdc: HDC, r: RECT, width: i32) {
    unsafe {
        let brush = CreateSolidBrush(COLORREF(0x000000FF));
        let bars = [
            RECT { left: r.left, top: r.top, right: r.right, bottom: r.top + width },
            RECT { left: r.left, top: r.bottom - width, right: r.right, bottom: r.bottom },
            RECT { left: r.left, top: r.top, right: r.left + width, bottom: r.bottom },
            RECT { left: r.right - width, top: r.top, right: r.right, bottom: r.bottom },
        ];
        for bar in bars {
            FillRect(hdc, &bar, brush);
        }
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
}

/// 親ウィンドウの隣(画面に余裕がある方向)に配置する座標を計算する
fn place_beside_parent(parent: HWND, cw: i32, ch: i32) -> (i32, i32) {
    unsafe {
        let mut prect = RECT::default();
        let _ = GetWindowRect(parent, &mut prect);
        let hmon = MonitorFromWindow(parent, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(hmon, &mut mi);
        let work = mi.rcWork;
        let space_right = work.right - prect.right;
        let space_left = prect.left - work.left;
        let y = prect.top.max(work.top).min((work.bottom - ch).max(work.top));
        let x = if space_right >= space_left {
            (prect.right).min(work.right - cw).max(work.left)
        } else {
            (prect.left - cw).max(work.left)
        };
        (x, y)
    }
}

/// 画像をプレビューウィンドウで開く(既存があれば再利用し1つまでに制限)。
/// box_rect を渡すと OCR抽出範囲を赤枠で重ねて表示する (SPECv0.5.2追補)。
pub(crate) fn open_preview(
    parent: HWND,
    which: ImgKind,
    image: Option<(u32, u32, Vec<u8>)>,
    box_rect: Option<(i32, i32, i32, i32)>,
) {
    let Some((iw, ih, _)) = image.as_ref().map(|(a, b, _)| (*a, *b, ())) else { return };
    IMG.with(|c| *c.borrow_mut() = image);
    BOX_RECT.with(|c| *c.borrow_mut() = box_rect);
    SCROLL.with(|c| *c.borrow_mut() = (0, 0));
    ZOOM.with(|c| *c.borrow_mut() = 1.0);
    let title = match which {
        ImgKind::Ocr => w!("OCR対象画像 (クリック&ドラッグ:パン / ホイール:拡大縮小 / Esc:閉じる / Ctrl+C:コピー)"),
        ImgKind::Full => w!("全体画像 (クリック&ドラッグ:パン / ホイール:拡大縮小 / Esc:閉じる / Ctrl+C:コピー)"),
    };

    // 既存のプレビューウィンドウがあれば再利用する(2つ以上開かない)
    let existing = PREVIEW_HWND.with(|c| *c.borrow());
    if let Some(raw) = existing {
        let h = HWND(raw as *mut _);
        if unsafe { IsWindow(Some(h)) }.as_bool() {
            let cw = (iw as i32 + 20).min(1400);
            let ch = (ih as i32 + 40).min(900);
            let (x, y) = place_beside_parent(parent, cw, ch);
            unsafe {
                let _ = SetWindowTextW(h, title);
                let _ = SetWindowPos(h, None, x, y, cw, ch, SWP_NOACTIVATE);
                update_scrollbars(h);
                let _ = InvalidateRect(Some(h), None, false);
                let _ = ShowWindow(h, SW_SHOW);
                let _ = SetForegroundWindow(h);
            }
            return;
        }
    }

    let inst = unsafe {
        HINSTANCE(
            windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                .map(|m| m.0)
                .unwrap_or(std::ptr::null_mut()),
        )
    };
    unsafe {
        let class = w!("FocusTranslatorImagePreview");
        REGISTERED.with(|r| {
            if !*r.borrow() {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(preview_wndproc),
                    hInstance: inst,
                    hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                    hIcon: crate::app_state::app_icon(),
                    hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as usize as *mut _),
                    lpszClassName: class,
                    ..Default::default()
                };
                RegisterClassW(&wc);
                *r.borrow_mut() = true;
            }
        });
        // 画像サイズに合わせる(画面サイズにクランプ、スクロールバー付き)
        let cw = (iw as i32 + 20).min(1400);
        let ch = (ih as i32 + 40).min(900);
        let (x, y) = place_beside_parent(parent, cw, ch);
        if let Ok(pwnd) = CreateWindowExW(
            WS_EX_TOPMOST,
            class,
            title,
            WS_OVERLAPPEDWINDOW | WS_HSCROLL | WS_VSCROLL,
            x,
            y,
            cw,
            ch,
            Some(parent),
            None,
            Some(inst),
            None,
        ) {
            PREVIEW_HWND.with(|c| *c.borrow_mut() = Some(pwnd.0 as isize));
            update_scrollbars(pwnd);
            let _ = ShowWindow(pwnd, SW_SHOW);
        }
    }
}

fn scaled_size() -> Option<(i32, i32)> {
    IMG.with(|c| c.borrow().as_ref().map(|(w, hh, _)| (*w, *hh))).map(|(iw, ih)| {
        let z = ZOOM.with(|z| *z.borrow());
        (
            ((iw as f64 * z).round() as i32).max(1),
            ((ih as f64 * z).round() as i32).max(1),
        )
    })
}

fn client_size(h: HWND) -> (i32, i32) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(h, &mut r);
    }
    (r.right - r.left, r.bottom - r.top)
}

/// スクロール量を画像範囲内にクランプして保存し、変化があれば再描画・スクロールバー更新する。
fn set_scroll_clamped(h: HWND, x: i32, y: i32) {
    let Some((sw, sh)) = scaled_size() else { return };
    let (cw, ch) = client_size(h);
    let max_x = (sw - cw).max(0);
    let max_y = (sh - ch).max(0);
    let nx = x.clamp(0, max_x);
    let ny = y.clamp(0, max_y);
    let changed = SCROLL.with(|c| {
        let mut sc = c.borrow_mut();
        let changed = *sc != (nx, ny);
        *sc = (nx, ny);
        changed
    });
    if changed {
        update_scrollbars(h);
        unsafe {
            let _ = InvalidateRect(Some(h), None, false);
        }
    }
}

fn step_scroll(h: HWND, dx: i32, dy: i32) {
    let (sx, sy) = SCROLL.with(|c| *c.borrow());
    set_scroll_clamped(h, sx + dx, sy + dy);
}

/// リサイズ後などにスクロール量が範囲外になっていないか確認する。
fn clamp_scroll_to_bounds(h: HWND) {
    let (sx, sy) = SCROLL.with(|c| *c.borrow());
    set_scroll_clamped(h, sx, sy);
}

/// カーソル位置(クライアント座標)を中心に拡大縮小する。デフォルトは100%表示。
fn zoom_at(h: HWND, cx: i32, cy: i32, wheel_delta: i32) {
    if IMG.with(|c| c.borrow().is_none()) {
        return;
    }
    let old_zoom = ZOOM.with(|z| *z.borrow());
    let notches = wheel_delta as f64 / 120.0;
    let factor = 1.25f64.powf(notches);
    let new_zoom = (old_zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
    if (new_zoom - old_zoom).abs() < f64::EPSILON {
        return;
    }
    let (sx, sy) = SCROLL.with(|c| *c.borrow());
    // カーソル直下の画像上の点がズーム後も同じ画面位置に留まるよう補正する
    let img_x = (sx as f64 + cx as f64) / old_zoom;
    let img_y = (sy as f64 + cy as f64) / old_zoom;
    ZOOM.with(|z| *z.borrow_mut() = new_zoom);
    let nsx = (img_x * new_zoom - cx as f64).round() as i32;
    let nsy = (img_y * new_zoom - cy as f64).round() as i32;
    set_scroll_clamped(h, nsx, nsy);
}

fn update_scrollbars(h: HWND) {
    let Some((sw, sh)) = scaled_size() else { return };
    let (cw, ch) = client_size(h);
    let (sx, sy) = SCROLL.with(|c| *c.borrow());
    unsafe {
        let mut si = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_ALL,
            nMin: 0,
            nMax: (sw - 1).max(0),
            nPage: cw.max(1) as u32,
            nPos: sx,
            ..Default::default()
        };
        SetScrollInfo(h, SB_HORZ, &si, true);
        si.nMax = (sh - 1).max(0);
        si.nPage = ch.max(1) as u32;
        si.nPos = sy;
        SetScrollInfo(h, SB_VERT, &si, true);
    }
}

/// WM_HSCROLL / WM_VSCROLL (スクロールバーのクリック・ドラッグ操作) を処理する。
fn handle_scroll_msg(h: HWND, wparam: WPARAM, horizontal: bool) {
    let code = (wparam.0 & 0xFFFF) as i32;
    let Some((sw, sh)) = scaled_size() else { return };
    let (cw, ch) = client_size(h);
    let (max, page, cur) = if horizontal {
        (sw, cw, SCROLL.with(|c| c.borrow().0))
    } else {
        (sh, ch, SCROLL.with(|c| c.borrow().1))
    };
    let max_pos = (max - page).max(0);
    let small = 40;
    let large = page.max(1);
    let bar = if horizontal { SB_HORZ } else { SB_VERT };
    let new_pos = if code == SB_LINEUP.0 {
        cur - small
    } else if code == SB_LINEDOWN.0 {
        cur + small
    } else if code == SB_PAGEUP.0 {
        cur - large
    } else if code == SB_PAGEDOWN.0 {
        cur + large
    } else if code == SB_THUMBTRACK.0 || code == SB_THUMBPOSITION.0 {
        let mut si = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_TRACKPOS,
            ..Default::default()
        };
        unsafe {
            let _ = GetScrollInfo(h, bar, &mut si);
        }
        si.nTrackPos
    } else if code == SB_TOP.0 {
        0
    } else {
        cur
    };
    let new_pos = new_pos.clamp(0, max_pos);
    if horizontal {
        set_scroll_clamped(h, new_pos, SCROLL.with(|c| c.borrow().1));
    } else {
        set_scroll_clamped(h, SCROLL.with(|c| c.borrow().0), new_pos);
    }
}

/// 現在の画像を CF_DIB (24bit ボトムアップDIB) としてクリップボードへコピーする。
fn copy_to_clipboard(h: HWND) {
    let img = IMG.with(|c| c.borrow().clone());
    let Some((iw, ih, rgba)) = img else { return };
    let iw = iw as i32;
    let ih = ih as i32;
    let row_stride = ((iw * 3 + 3) / 4) * 4;
    let mut pixels = vec![0u8; (row_stride * ih) as usize];
    for y in 0..ih {
        let src_row = &rgba[(y as usize * iw as usize * 4)..][..iw as usize * 4];
        // ボトムアップDIBのため上下反転して書き込む
        let dst_y = (ih - 1 - y) as usize;
        let dst_row = &mut pixels[(dst_y * row_stride as usize)..][..iw as usize * 3];
        for (dst, src) in dst_row.chunks_mut(3).zip(src_row.chunks(4)) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
        }
    }
    let header = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: iw,
        biHeight: ih,
        biPlanes: 1,
        biBitCount: 24,
        biCompression: BI_RGB.0,
        biSizeImage: pixels.len() as u32,
        ..Default::default()
    };
    unsafe {
        let header_bytes = std::slice::from_raw_parts(
            (&header as *const BITMAPINFOHEADER) as *const u8,
            std::mem::size_of::<BITMAPINFOHEADER>(),
        );
        let total = header_bytes.len() + pixels.len();
        if OpenClipboard(Some(h)).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, total) {
            let ptr = GlobalLock(hmem);
            let mut ok = false;
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(header_bytes.as_ptr(), ptr as *mut u8, header_bytes.len());
                std::ptr::copy_nonoverlapping(
                    pixels.as_ptr(),
                    (ptr as *mut u8).add(header_bytes.len()),
                    pixels.len(),
                );
                let _ = GlobalUnlock(hmem);
                ok = SetClipboardData(CF_DIB.0 as u32, Some(HANDLE(hmem.0))).is_ok();
            }
            if !ok {
                let _ = windows::Win32::Foundation::GlobalFree(Some(HGLOBAL(hmem.0)));
            }
        }
        let _ = CloseClipboard();
    }
}

/// ダブルバッファで描画してチラつきを防ぐ(WM_ERASEBKGNDは無効化しているため、
/// この関数が領域全体を毎回塗りつぶし切る)。
fn paint(h: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(h, &mut ps);
        let mut rect = RECT::default();
        let _ = GetClientRect(h, &mut rect);
        let w = (rect.right - rect.left).max(1);
        let ht = (rect.bottom - rect.top).max(1);

        let mem = CreateCompatibleDC(Some(hdc));
        let bmp = CreateCompatibleBitmap(hdc, w, ht);
        let oldbmp = SelectObject(mem, HGDIOBJ(bmp.0));

        let bg = CreateSolidBrush(COLORREF(0x00202020));
        FillRect(mem, &rect, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));

        IMG.with(|c| {
            if let Some((iw, ih, rgba)) = c.borrow().as_ref() {
                let (sx, sy) = SCROLL.with(|s| *s.borrow());
                let zoom = ZOOM.with(|z| *z.borrow());
                let mut bgra = vec![0u8; rgba.len()];
                for (o, px) in bgra.chunks_mut(4).zip(rgba.chunks(4)) {
                    o[0] = px[2];
                    o[1] = px[1];
                    o[2] = px[0];
                    o[3] = px[3];
                }
                let bmi = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: *iw as i32,
                        biHeight: -(*ih as i32),
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let dw = ((*iw as f64 * zoom).round() as i32).max(1);
                let dh = ((*ih as f64 * zoom).round() as i32).max(1);
                SetStretchBltMode(mem, HALFTONE);
                StretchDIBits(
                    mem, -sx, -sy, dw, dh,
                    0, 0, *iw as i32, *ih as i32,
                    Some(bgra.as_ptr() as *const _), &bmi, DIB_RGB_COLORS,
                    windows::Win32::Graphics::Gdi::SRCCOPY,
                );
                // 全体画像表示時はOCR抽出範囲を赤枠で示す (SPECv0.5.2追補)
                if let Some((bx, by, bw, bh)) = BOX_RECT.with(|b| *b.borrow()) {
                    let r = RECT {
                        left: (bx as f64 * zoom).round() as i32 - sx,
                        top: (by as f64 * zoom).round() as i32 - sy,
                        right: ((bx + bw) as f64 * zoom).round() as i32 - sx,
                        bottom: ((by + bh) as f64 * zoom).round() as i32 - sy,
                    };
                    draw_red_box(mem, r, 3);
                }
            }
        });

        let _ = windows::Win32::Graphics::Gdi::BitBlt(
            hdc, 0, 0, w, ht, Some(mem), 0, 0, windows::Win32::Graphics::Gdi::SRCCOPY,
        );

        SelectObject(mem, oldbmp);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);

        let _ = EndPaint(h, &ps);
    }
}

unsafe extern "system" fn preview_wndproc(h: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // ダブルバッファで領域全体を毎回塗りつぶし切るため、既定の背景消去は無効化する
        // (スクロール・ズーム時の点滅を防ぐ)。
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut pt = POINT {
                x: (lparam.0 & 0xFFFF) as i16 as i32,
                y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
            };
            unsafe {
                let _ = ScreenToClient(h, &mut pt);
            }
            zoom_at(h, pt.x, pt.y, delta);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let sc = SCROLL.with(|c| *c.borrow());
            PANNING.with(|c| *c.borrow_mut() = Some(((x, y), sc)));
            unsafe {
                SetCapture(h);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let dragging = PANNING.with(|c| *c.borrow());
            if let Some(((sx0, sy0), (ox, oy))) = dragging {
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                set_scroll_clamped(h, ox - (x - sx0), oy - (y - sy0));
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if PANNING.with(|c| c.borrow_mut().take()).is_some() {
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
            LRESULT(0)
        }
        WM_HSCROLL => {
            handle_scroll_msg(h, wparam, true);
            LRESULT(0)
        }
        WM_VSCROLL => {
            handle_scroll_msg(h, wparam, false);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = wparam.0 as u16;
            if vk == VK_ESCAPE.0 {
                unsafe {
                    let _ = DestroyWindow(h);
                }
            } else if vk == VK_C.0 && unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0 {
                copy_to_clipboard(h);
            } else if vk == VK_LEFT.0 {
                step_scroll(h, -40, 0);
            } else if vk == VK_RIGHT.0 {
                step_scroll(h, 40, 0);
            } else if vk == VK_UP.0 {
                step_scroll(h, 0, -40);
            } else if vk == VK_DOWN.0 {
                step_scroll(h, 0, 40);
            }
            LRESULT(0)
        }
        WM_SIZE => {
            clamp_scroll_to_bounds(h);
            update_scrollbars(h);
            unsafe {
                let _ = InvalidateRect(Some(h), None, false);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            paint(h);
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(h);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PREVIEW_HWND.with(|c| *c.borrow_mut() = None);
            IMG.with(|c| *c.borrow_mut() = None);
            PANNING.with(|c| *c.borrow_mut() = None);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(h, msg, wparam, lparam) },
    }
}
