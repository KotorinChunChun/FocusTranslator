// ログビューア (FocusTranslator_LOG_SPECv0.1.md §4)
// 3段ドリルダウン: 認識ログ一覧 → 翻訳候補一覧 → 詳細(訳文/生JSON展開 + 画像小表示)。
// 全削除・最新に更新・外部画像ビューア起動。
use crate::logdb::{self, RecogRow, TransRow};
use crate::util::to_wide;
use std::cell::RefCell;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, COLOR_BTNFACE, CreateFontW, DEFAULT_CHARSET,
    DEFAULT_PITCH, DIB_RGB_COLORS, FF_DONTCARE, FW_NORMAL, HALFTONE, HBRUSH, InvalidateRect,
    SetStretchBltMode, StretchDIBits,
};
use windows::Win32::UI::Controls::{
    INITCOMMONCONTROLSEX, InitCommonControlsEx, LVCF_SUBITEM, LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW,
    LVIF_TEXT, LVITEMW, LVM_DELETEALLITEMS, LVM_GETNEXTITEM, LVM_INSERTCOLUMNW, LVM_INSERTITEMW,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMTEXTW, LVN_ITEMCHANGED, LVS_EX_FULLROWSELECT,
    LVS_REPORT, LVS_SINGLESEL, NMHDR,
};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect,
    GetDlgItem, HMENU, IDC_ARROW, IsWindow, LoadCursorW, MB_ICONQUESTION, MB_OK, MB_YESNO,
    MessageBoxW, SW_SHOW, SW_SHOWNORMAL, SendMessageW, SetForegroundWindow, SetWindowTextW,
    ShowWindow, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_NOTIFY, WM_SIZE, WNDCLASSW,
    WS_BORDER, WS_CHILD, WS_EX_TOPMOST, WS_HSCROLL, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
    WS_VSCROLL,
};
use windows::core::{PCWSTR, w};

const IDC_RECOG_LV: i32 = 201;
const IDC_TRANS_LV: i32 = 202;
const IDC_DETAIL: i32 = 203;
const IDC_BTN_SRC: i32 = 210;
const IDC_BTN_REQ: i32 = 211;
const IDC_BTN_RES: i32 = 212;
const IDC_BTN_IMG: i32 = 213;
const IDC_BTN_REFRESH: i32 = 214;
const IDC_BTN_CLEAR: i32 = 215;

#[derive(Clone, Copy, PartialEq)]
enum DetailView {
    Text,
    Request,
    Response,
}

struct State {
    recogs: Vec<RecogRow>,
    trans: Vec<TransRow>,
    sel_recog: Option<usize>,
    sel_trans: Option<usize>,
    detail_view: DetailView,
    /// 現在表示中画像のデコード済みRGBA (幅, 高さ, ピクセル)
    image: Option<(u32, u32, Vec<u8>)>,
}

thread_local! {
    static WND: RefCell<isize> = const { RefCell::new(0) };
    static REGISTERED: RefCell<bool> = const { RefCell::new(false) };
    static STATE: RefCell<State> = const { RefCell::new(State {
        recogs: Vec::new(), trans: Vec::new(), sel_recog: None, sel_trans: None,
        detail_view: DetailView::Text, image: None,
    }) };
}

pub fn hwnd() -> HWND {
    HWND(WND.with(|w| *w.borrow()) as *mut _)
}

pub fn is_open() -> bool {
    let h = hwnd();
    !h.is_invalid() && unsafe { IsWindow(Some(h)).as_bool() }
}

pub fn open(instance: HINSTANCE) {
    if is_open() {
        unsafe {
            let _ = SetForegroundWindow(hwnd());
        }
        return;
    }
    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: windows::Win32::UI::Controls::ICC_LISTVIEW_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);

        let class = w!("FocusTranslatorLogViewer");
        REGISTERED.with(|r| {
            if !*r.borrow() {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(wndproc),
                    hInstance: instance,
                    hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                    hIcon: crate::app_icon(),
                    hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as usize as *mut _),
                    lpszClassName: class,
                    ..Default::default()
                };
                RegisterClassW(&wc);
                *r.borrow_mut() = true;
            }
        });
        if let Ok(h) = CreateWindowExW(
            WS_EX_TOPMOST,
            class,
            w!("Focus Translator ログビューア"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            920,
            720,
            None,
            None,
            Some(instance),
            None,
        ) {
            WND.with(|w| *w.borrow_mut() = h.0 as isize);
            build(h, instance);
            reload();
            let _ = ShowWindow(h, SW_SHOW);
            let _ = SetForegroundWindow(h);
        }
    }
}

use windows::Win32::UI::WindowsAndMessaging::RegisterClassW;

fn lv(parent: HWND, inst: HINSTANCE, id: i32) -> HWND {
    unsafe {
        let h = CreateWindowExW(
            Default::default(),
            w!("SysListView32"),
            w!(""),
            WS_CHILD | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(LVS_REPORT | LVS_SINGLESEL),
            0,
            0,
            0,
            0,
            Some(parent),
            Some(HMENU(id as usize as *mut _)),
            Some(inst),
            None,
        )
        .unwrap_or_default();
        SendMessageW(
            h,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            Some(WPARAM(0)),
            Some(LPARAM(LVS_EX_FULLROWSELECT as isize)),
        );
        h
    }
}

fn add_col(lvh: HWND, idx: i32, text: &str, width: i32) {
    unsafe {
        let wide = to_wide(text);
        let mut col = LVCOLUMNW {
            mask: LVCF_TEXT | LVCF_WIDTH | LVCF_SUBITEM,
            cx: width,
            pszText: windows::core::PWSTR(wide.as_ptr() as *mut _),
            iSubItem: idx,
            ..Default::default()
        };
        SendMessageW(
            lvh,
            LVM_INSERTCOLUMNW,
            Some(WPARAM(idx as usize)),
            Some(LPARAM(&mut col as *mut _ as isize)),
        );
    }
}

fn build(h: HWND, inst: HINSTANCE) {
    let recog = lv(h, inst, IDC_RECOG_LV);
    add_col(recog, 0, "時刻", 140);
    add_col(recog, 1, "モード", 60);
    add_col(recog, 2, "エンジン", 80);
    add_col(recog, 3, "ms", 50);
    add_col(recog, 4, "画像", 40);
    add_col(recog, 5, "認識テキスト", 460);

    let trans = lv(h, inst, IDC_TRANS_LV);
    add_col(trans, 0, "時刻", 140);
    add_col(trans, 1, "エンジン", 70);
    add_col(trans, 2, "方向", 70);
    add_col(trans, 3, "ms", 50);
    add_col(trans, 4, "tok入/出", 70);
    add_col(trans, 5, "訳文", 480);

    // 詳細エディット (複数行・読み取り専用)
    unsafe {
        const ES_MULTILINE: u32 = 0x0004;
        const ES_READONLY: u32 = 0x0800;
        const ES_AUTOVSCROLL: u32 = 0x0040;
        let _ = CreateWindowExW(
            Default::default(),
            w!("EDIT"),
            w!(""),
            WS_CHILD
                | WS_VISIBLE
                | WS_BORDER
                | WS_VSCROLL
                | WS_HSCROLL
                | WINDOW_STYLE(ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL),
            0,
            0,
            0,
            0,
            Some(h),
            Some(HMENU(IDC_DETAIL as usize as *mut _)),
            Some(inst),
            None,
        );
    }

    btn(h, inst, "原文/訳文", IDC_BTN_SRC);
    btn(h, inst, "送信JSON", IDC_BTN_REQ);
    btn(h, inst, "受信JSON", IDC_BTN_RES);
    btn(h, inst, "画像を開く", IDC_BTN_IMG);
    btn(h, inst, "最新に更新", IDC_BTN_REFRESH);
    btn(h, inst, "ログを全削除", IDC_BTN_CLEAR);

    // フォント適用
    unsafe {
        let font = CreateFontW(
            -13, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, DEFAULT_CHARSET, Default::default(),
            Default::default(), Default::default(),
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32, w!("Yu Gothic UI"),
        );
        let _ = windows::Win32::UI::WindowsAndMessaging::EnumChildWindows(
            Some(h),
            Some(set_font_proc),
            LPARAM(font.0 as isize),
        );
    }
    layout(h);
}

unsafe extern "system" fn set_font_proc(child: HWND, lparam: LPARAM) -> windows::core::BOOL {
    unsafe {
        SendMessageW(
            child,
            windows::Win32::UI::WindowsAndMessaging::WM_SETFONT,
            Some(WPARAM(lparam.0 as usize)),
            Some(LPARAM(1)),
        );
    }
    true.into()
}

fn btn(parent: HWND, inst: HINSTANCE, text: &str, id: i32) -> HWND {
    unsafe {
        let wide = to_wide(text);
        CreateWindowExW(
            Default::default(),
            w!("BUTTON"),
            PCWSTR(wide.as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP,
            0,
            0,
            0,
            0,
            Some(parent),
            Some(HMENU(id as usize as *mut _)),
            Some(inst),
            None,
        )
        .unwrap_or_default()
    }
}

fn dlg_item(h: HWND, id: i32) -> HWND {
    unsafe { GetDlgItem(Some(h), id).unwrap_or_default() }
}

/// ウィンドウサイズに合わせて子コントロールを配置
fn layout(h: HWND) {
    unsafe {
        let mut rc = RECT::default();
        let _ = GetClientRect(h, &mut rc);
        let w = rc.right;
        let ht = rc.bottom;
        let pad = 8;
        let btn_h = 28;
        let btn_row_y = ht - btn_h - pad;
        // 3段: 認識(上) 30%、翻訳(中) 25%、詳細(下) 残り
        let recog_h = (ht as f32 * 0.30) as i32;
        let trans_h = (ht as f32 * 0.25) as i32;
        let detail_top = pad + recog_h + pad + trans_h + pad;
        let detail_h = (btn_row_y - pad) - detail_top;
        // 詳細は左テキスト60% / 右画像40%
        let detail_text_w = (w as f32 * 0.60) as i32 - pad;

        let mv = |id: i32, x: i32, y: i32, cw: i32, ch: i32| {
            let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                dlg_item(h, id),
                None,
                x,
                y,
                cw,
                ch,
                windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER,
            );
        };
        mv(IDC_RECOG_LV, pad, pad, w - pad * 2, recog_h);
        mv(IDC_TRANS_LV, pad, pad + recog_h + pad, w - pad * 2, trans_h);
        mv(IDC_DETAIL, pad, detail_top, detail_text_w, detail_h.max(20));
        // 画像領域は WM_PAINT で描くのでコントロールは無し(右側の座標は paint で参照)

        let by = btn_row_y;
        let bw = 96;
        let gap = 6;
        mv(IDC_BTN_SRC, pad, by, bw, btn_h);
        mv(IDC_BTN_REQ, pad + (bw + gap), by, bw, btn_h);
        mv(IDC_BTN_RES, pad + (bw + gap) * 2, by, bw, btn_h);
        mv(IDC_BTN_IMG, pad + (bw + gap) * 3, by, bw, btn_h);
        mv(IDC_BTN_REFRESH, w - (bw + gap) * 2 - pad, by, bw, btn_h);
        mv(IDC_BTN_CLEAR, w - (bw + gap) - pad, by, bw + 8, btn_h);
    }
}

fn fmt_ts(ts_ms: i64) -> String {
    // 簡易ローカル時刻(UTCms → HH:MM:SS 表示のみ、日付は MM/DD)
    let secs = ts_ms / 1000;
    let days = secs / 86400;
    let tod = secs % 86400;
    let (h, m, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    // 1970-01-01 からの日数 → 月日は概算せず、経過日ベースは分かりづらいので時刻主体
    let _ = days;
    format!("{h:02}:{m:02}:{s:02}")
}

fn lv_clear(lvh: HWND) {
    unsafe {
        SendMessageW(lvh, LVM_DELETEALLITEMS, Some(WPARAM(0)), Some(LPARAM(0)));
    }
}

fn lv_add_row(lvh: HWND, row: i32, cols: &[String]) {
    unsafe {
        let first = to_wide(&cols[0]);
        let mut item = LVITEMW {
            mask: LVIF_TEXT,
            iItem: row,
            iSubItem: 0,
            pszText: windows::core::PWSTR(first.as_ptr() as *mut _),
            ..Default::default()
        };
        SendMessageW(lvh, LVM_INSERTITEMW, Some(WPARAM(0)), Some(LPARAM(&mut item as *mut _ as isize)));
        for (i, c) in cols.iter().enumerate().skip(1) {
            let wide = to_wide(c);
            let mut sub = LVITEMW {
                mask: LVIF_TEXT,
                iItem: row,
                iSubItem: i as i32,
                pszText: windows::core::PWSTR(wide.as_ptr() as *mut _),
                ..Default::default()
            };
            SendMessageW(
                lvh,
                LVM_SETITEMTEXTW,
                Some(WPARAM(row as usize)),
                Some(LPARAM(&mut sub as *mut _ as isize)),
            );
        }
    }
}

fn lv_selected(lvh: HWND) -> Option<usize> {
    unsafe {
        const LVNI_SELECTED: usize = 0x0002;
        let r = SendMessageW(
            lvh,
            LVM_GETNEXTITEM,
            Some(WPARAM(usize::MAX)),
            Some(LPARAM(LVNI_SELECTED as isize)),
        );
        if r.0 < 0 { None } else { Some(r.0 as usize) }
    }
}

fn truncate(s: &str, n: usize) -> String {
    let one_line: String = s.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
    if one_line.chars().count() > n {
        one_line.chars().take(n).collect::<String>() + "…"
    } else {
        one_line
    }
}

/// DBから再読込して認識一覧を更新
fn reload() {
    let recogs = logdb::recent_recognitions(1000);
    let h = hwnd();
    let recog_lv = dlg_item(h, IDC_RECOG_LV);
    lv_clear(recog_lv);
    for (i, r) in recogs.iter().enumerate() {
        let img = if r.image_path.is_some() { "✓" } else { "" };
        let text = if r.success { r.source_text.clone() } else { format!("[エラー] {}", r.error) };
        lv_add_row(recog_lv, i as i32, &[
            fmt_ts(r.ts_ms),
            r.mode.clone(),
            r.engine.clone(),
            r.duration_ms.to_string(),
            img.to_string(),
            truncate(&text, 80),
        ]);
    }
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.recogs = recogs;
        st.trans.clear();
        st.sel_recog = None;
        st.sel_trans = None;
        st.image = None;
    });
    lv_clear(dlg_item(h, IDC_TRANS_LV));
    set_detail("");
    update_image_button();
}

/// 認識選択時: 翻訳候補一覧を更新
fn on_recog_selected(idx: usize) {
    let recog_id = STATE.with(|s| s.borrow().recogs.get(idx).map(|r| r.id));
    let Some(recog_id) = recog_id else { return };
    let trans = logdb::translations_for(recog_id);
    let h = hwnd();
    let trans_lv = dlg_item(h, IDC_TRANS_LV);
    lv_clear(trans_lv);
    for (i, t) in trans.iter().enumerate() {
        let dir = format!("{}→{}", t.source_lang, t.target_lang);
        let tok = match (t.tokens_in, t.tokens_out) {
            (Some(a), Some(b)) => format!("{a}/{b}"),
            _ => String::new(),
        };
        let text = if t.success {
            let cache = if t.cache_hit { "[cache] " } else { "" };
            format!("{cache}{}", t.translated_text)
        } else {
            format!("[エラー] {}", t.error)
        };
        lv_add_row(trans_lv, i as i32, &[
            fmt_ts(t.ts_ms),
            t.engine.clone(),
            dir,
            t.duration_ms.to_string(),
            tok,
            truncate(&text, 80),
        ]);
    }
    // 画像デコード
    let image = STATE.with(|s| {
        s.borrow().recogs.get(idx).and_then(|r| r.image_path.clone())
    }).and_then(|rel| decode_png(&logdb::logs_dir().join(rel)));

    STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.trans = trans;
        st.sel_recog = Some(idx);
        st.sel_trans = None;
        st.image = image;
    });
    set_detail("");
    update_image_button();
    unsafe {
        let _ = InvalidateRect(Some(hwnd()), None, true);
    }
}

/// 翻訳選択時: 詳細を表示
fn on_trans_selected(idx: usize) {
    STATE.with(|s| s.borrow_mut().sel_trans = Some(idx));
    refresh_detail();
}

fn refresh_detail() {
    let text = STATE.with(|s| {
        let st = s.borrow();
        let Some(ti) = st.sel_trans else { return String::new() };
        let Some(t) = st.trans.get(ti) else { return String::new() };
        match st.detail_view {
            DetailView::Text => {
                let src = st.sel_recog.and_then(|ri| st.recogs.get(ri)).map(|r| r.source_text.clone()).unwrap_or_default();
                let body = if t.success { t.translated_text.clone() } else { format!("[エラー] {}", t.error) };
                format!("【原文】\r\n{src}\r\n\r\n【訳文】\r\n{body}")
            }
            DetailView::Request => {
                if t.request_json.is_empty() { "(送信JSONなし: ローカル翻訳またはキャッシュ)".into() }
                else { pretty_json(&t.request_json) }
            }
            DetailView::Response => {
                if t.response_json.is_empty() { "(受信JSONなし)".into() }
                else { pretty_json(&t.response_json) }
            }
        }
    });
    set_detail(&text);
}

fn pretty_json(s: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(s) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| s.to_string()),
        Err(_) => s.to_string(),
    }
}

fn set_detail(text: &str) {
    unsafe {
        // EDIT は \n だけだと改行されないため \r\n に正規化
        let normalized = text.replace("\r\n", "\n").replace('\n', "\r\n");
        let wide = to_wide(&normalized);
        let _ = SetWindowTextW(dlg_item(hwnd(), IDC_DETAIL), PCWSTR(wide.as_ptr()));
    }
}

fn update_image_button() {
    let has = STATE.with(|s| {
        let st = s.borrow();
        st.sel_recog
            .and_then(|ri| st.recogs.get(ri))
            .map(|r| r.image_path.is_some())
            .unwrap_or(false)
    });
    unsafe {
        let _ = EnableWindow(dlg_item(hwnd(), IDC_BTN_IMG), has);
    }
}

/// PNGファイルを RGBA へデコード
fn decode_png(path: &std::path::Path) -> Option<(u32, u32, Vec<u8>)> {
    let file = std::fs::File::open(path).ok()?;
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    // RGBA8 前提(capture::to_png が RGBA を書くため)。他は簡易対応。
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for px in buf[..info.buffer_size()].chunks(3) {
                out.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            out
        }
        _ => return None,
    };
    Some((w, h, rgba))
}

/// 詳細下段の右側に画像を縮小描画する
fn paint_image(h: HWND) {
    STATE.with(|s| {
        let st = s.borrow();
        let Some((iw, ih, rgba)) = st.image.as_ref() else { return };
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(h, &mut rc);
            let w = rc.right;
            let ht = rc.bottom;
            let pad = 8;
            let btn_h = 28;
            let btn_row_y = ht - btn_h - pad;
            let recog_h = (ht as f32 * 0.30) as i32;
            let trans_h = (ht as f32 * 0.25) as i32;
            let detail_top = pad + recog_h + pad + trans_h + pad;
            let detail_h = (btn_row_y - pad) - detail_top;
            let img_left = (w as f32 * 0.60) as i32 + pad;
            let img_w = w - img_left - pad;
            let img_h = detail_h.max(20);

            // アスペクト比維持で img_w×img_h に収める
            let scale = (img_w as f32 / *iw as f32).min(img_h as f32 / *ih as f32).min(1.0);
            let dw = (*iw as f32 * scale) as i32;
            let dh = (*ih as f32 * scale) as i32;

            let hdc = windows::Win32::Graphics::Gdi::GetDC(Some(h));
            // 背景を塗る
            let bg = windows::Win32::Graphics::Gdi::CreateSolidBrush(COLORREF(0x00202020));
            let area = RECT { left: img_left, top: detail_top, right: img_left + img_w, bottom: detail_top + img_h };
            windows::Win32::Graphics::Gdi::FillRect(hdc, &area, bg);
            let _ = windows::Win32::Graphics::Gdi::DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(bg.0));

            // RGBA → BGRA トップダウンDIB
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
                    biHeight: -(*ih as i32), // トップダウン
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            SetStretchBltMode(hdc, HALFTONE);
            StretchDIBits(
                hdc,
                img_left,
                detail_top,
                dw,
                dh,
                0,
                0,
                *iw as i32,
                *ih as i32,
                Some(bgra.as_ptr() as *const _),
                &bmi,
                DIB_RGB_COLORS,
                windows::Win32::Graphics::Gdi::SRCCOPY,
            );
            let _ = windows::Win32::Graphics::Gdi::ReleaseDC(Some(h), hdc);
        }
    });
}

fn open_current_image() {
    let path = STATE.with(|s| {
        let st = s.borrow();
        st.sel_recog
            .and_then(|ri| st.recogs.get(ri))
            .and_then(|r| r.image_path.clone())
    });
    if let Some(rel) = path {
        let full = logdb::logs_dir().join(rel);
        unsafe {
            let wide = to_wide(&full.to_string_lossy());
            let _ = ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(wide.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
        }
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            layout(h);
            unsafe {
                let _ = InvalidateRect(Some(h), None, true);
            }
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_PAINT => {
            unsafe {
                let mut ps = windows::Win32::Graphics::Gdi::PAINTSTRUCT::default();
                let _ = windows::Win32::Graphics::Gdi::BeginPaint(h, &mut ps);
                paint_image(h);
                let _ = windows::Win32::Graphics::Gdi::EndPaint(h, &ps);
            }
            LRESULT(0)
        }
        WM_NOTIFY => {
            let nmhdr = unsafe { &*(lparam.0 as *const NMHDR) };
            if nmhdr.code == LVN_ITEMCHANGED {
                let id = nmhdr.idFrom as i32;
                if id == IDC_RECOG_LV {
                    if let Some(sel) = lv_selected(dlg_item(h, IDC_RECOG_LV)) {
                        on_recog_selected(sel);
                    }
                } else if id == IDC_TRANS_LV
                    && let Some(sel) = lv_selected(dlg_item(h, IDC_TRANS_LV)) {
                        on_trans_selected(sel);
                    }
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            match id {
                IDC_BTN_SRC => {
                    STATE.with(|s| s.borrow_mut().detail_view = DetailView::Text);
                    refresh_detail();
                }
                IDC_BTN_REQ => {
                    STATE.with(|s| s.borrow_mut().detail_view = DetailView::Request);
                    refresh_detail();
                }
                IDC_BTN_RES => {
                    STATE.with(|s| s.borrow_mut().detail_view = DetailView::Response);
                    refresh_detail();
                }
                IDC_BTN_IMG => open_current_image(),
                IDC_BTN_REFRESH => reload(),
                IDC_BTN_CLEAR => {
                    let r = unsafe {
                        MessageBoxW(
                            Some(h),
                            w!("すべてのログと画像を削除します。よろしいですか?"),
                            w!("ログを全削除"),
                            MB_YESNO | MB_ICONQUESTION,
                        )
                    };
                    if r == windows::Win32::UI::WindowsAndMessaging::IDYES {
                        logdb::clear_all();
                        reload();
                        unsafe {
                            MessageBoxW(Some(h), w!("ログを削除しました。"), w!("Focus Translator"), MB_OK);
                        }
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(h);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            WND.with(|w| *w.borrow_mut() = 0);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(h, msg, wparam, lparam) },
    }
}
