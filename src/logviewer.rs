// ログビューア (FocusTranslator_LOG_SPECv0.1.md §4)
// 3段ドリルダウン: 認識ログ一覧 → 翻訳候補一覧 → 詳細(訳文/生JSON展開 + 画像小表示)。
// 全削除・最新に更新・外部画像ビューア起動。
use crate::logdb::{self, RecogRow, TransRow};
use crate::util::to_wide;
use crate::ui_helpers::*;
use std::cell::RefCell;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, COLOR_BTNFACE, CreateFontW, DEFAULT_CHARSET,
    DEFAULT_PITCH, DIB_RGB_COLORS, FF_DONTCARE, FW_NORMAL, GetMonitorInfoW, HALFTONE, HBRUSH,
    InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, SetStretchBltMode,
    StretchDIBits,
};
use windows::Win32::UI::Controls::{
    INITCOMMONCONTROLSEX, InitCommonControlsEx, LVCF_SUBITEM, LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW,
    LVIF_STATE, LVIF_TEXT, LVITEMW, LIST_VIEW_ITEM_STATE_FLAGS, LVM_DELETEALLITEMS, LVM_GETNEXTITEM,
    LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMTEXTW,
    LVM_SETITEMW, LVM_ENSUREVISIBLE, LVN_ITEMCHANGED, LVN_KEYDOWN, NMLVKEYDOWN, LVS_EX_FULLROWSELECT, LVS_REPORT,
    LVS_SHOWSELALWAYS, LVS_SINGLESEL, NMHDR,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_DELETE,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBS_DROPDOWNLIST, CW_USEDEFAULT, CallWindowProcW,
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_WNDPROC, GetClientRect, GetDlgItem,
    GetWindowRect, HMENU, IDC_ARROW, IDC_SIZENS, IsWindow, LoadCursorW, MB_ICONQUESTION, MB_OK,
    MB_YESNO, MessageBoxW, SWP_NOACTIVATE, SetCursor, SW_SHOW, SW_SHOWNORMAL, SendMessageW,
    SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow,
    WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_NOTIFY, WM_SETCURSOR, WM_SIZE, WNDCLASSW, WS_BORDER, WS_CHILD,
    WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
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
const IDC_OCR_COMBO: i32 = 220;
const IDC_TR_COMBO: i32 = 221;
const IDC_BTN_REOCR: i32 = 222;
const IDC_BTN_RETRANS: i32 = 223;
const IDC_BTN_DEL_RECOG: i32 = 224;
const IDC_BTN_DEL_TRANS: i32 = 225;

const IDC_SEARCH_EDIT: i32 = 230;
const IDC_EXE_COMBO: i32 = 231;
const IDC_BTN_EXPORT: i32 = 232;
const IDC_BTN_EXP: i32 = 233;
const IDC_TAG_EDIT: i32 = 234;
const IDC_BTN_SAVE_TAG: i32 = 235;

/// 再OCR/再翻訳のワーカースレッド完了通知(ビューア限定メッセージ)
const WM_APP_RELOAD: u32 = WM_APP + 30;

/// 再OCRエンジン(内部キー / 表示名)
const OCR_ENGINES: [(&str, &str); 5] = [
    ("win", "Windows OCR"),
    ("paddle", "PaddleOCR"),
    ("yomitoku", "YomiToku"),
    ("ndl", "NDL-OCR"),
    ("llm", "LLM(統合)"),
];
/// 再翻訳エンジン(内部キー / 表示名)
const TR_ENGINES: [(&str, &str); 4] = [
    ("local", "ローカルONNX"),
    ("deepl", "DeepL"),
    ("google", "Google"),
    ("llm", "LLM"),
];

#[derive(Clone, Copy, PartialEq)]
enum DetailView {
    Text,
    Request,
    Response,
    Explanation,
}

struct State {
    recogs: Vec<RecogRow>,
    trans: Vec<TransRow>,
    sel_recog: Option<usize>,
    sel_trans: Option<usize>,
    detail_view: DetailView,
    /// 現在表示中画像のデコード済みRGBA (幅, 高さ, ピクセル)
    image: Option<(u32, u32, Vec<u8>)>,
    /// 認識/翻訳境界の上部エリアに対する累積比率
    split_a: f32,
    split_b: f32,
    /// スプリッタードラッグ中(1=認識/翻訳境界, 2=翻訳/詳細境界)
    dragging: u8,
}

thread_local! {
    static WND: RefCell<isize> = const { RefCell::new(0) };
    static REGISTERED: RefCell<bool> = const { RefCell::new(false) };
    static STATE: RefCell<State> = const { RefCell::new(State {
        recogs: Vec::new(), trans: Vec::new(), sel_recog: None, sel_trans: None,
        detail_view: DetailView::Text, image: None,
        split_a: 0.38, split_b: 0.66, dragging: 0,
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
                    hIcon: crate::app_state::app_icon(),
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
            WS_CHILD
                | WS_VISIBLE
                | WS_BORDER
                | WINDOW_STYLE(LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS),
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
    add_col(recog, 0, "日時", 150);
    add_col(recog, 1, "アプリ", 100);
    add_col(recog, 2, "モード", 50);
    add_col(recog, 3, "エンジン", 70);
    add_col(recog, 4, "ms", 40);
    add_col(recog, 5, "画像", 40);
    add_col(recog, 6, "認識テキスト", 380);

    let trans = lv(h, inst, IDC_TRANS_LV);
    add_col(trans, 0, "日時", 150);
    add_col(trans, 1, "エンジン", 70);
    add_col(trans, 2, "方向", 70);
    add_col(trans, 3, "ms", 50);
    add_col(trans, 4, "tok入/出", 70);
    add_col(trans, 5, "訳文", 480);

    // 詳細エディット (複数行・読み取り専用・右端で折り返し)
    // WS_HSCROLL/ES_AUTOHSCROLLを付けないことで、右端での自動改行(ワードラップ)が有効になる。
    unsafe {
        const ES_MULTILINE: u32 = 0x0004;
        const ES_READONLY: u32 = 0x0800;
        const ES_AUTOVSCROLL: u32 = 0x0040;
        if let Ok(detail) = CreateWindowExW(
            Default::default(),
            w!("EDIT"),
            w!(""),
            WS_CHILD
                | WS_VISIBLE
                | WS_BORDER
                | WS_VSCROLL
                | WINDOW_STYLE(ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL),
            0,
            0,
            0,
            0,
            Some(h),
            Some(HMENU(IDC_DETAIL as usize as *mut _)),
            Some(inst),
            None,
        ) {
            subclass_detail(detail);
        }
    }

    btn(h, inst, "原文/訳文", IDC_BTN_SRC);
    btn(h, inst, "送信JSON", IDC_BTN_REQ);
    btn(h, inst, "受信JSON", IDC_BTN_RES);
    btn(h, inst, "解説(Exp)", IDC_BTN_EXP);
    btn(h, inst, "画像を開く", IDC_BTN_IMG);
    btn(h, inst, "最新に更新", IDC_BTN_REFRESH);
    btn(h, inst, "ログを全削除", IDC_BTN_CLEAR);

    ctl(h, inst, w!("EDIT"), "", WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP, 0, 0, 0, 0, IDC_SEARCH_EDIT);
    let exe_combo = combo(h, inst, IDC_EXE_COMBO);
    combo_add(exe_combo, "全アプリ");
    for exe in logdb::get_unique_app_exes() {
        combo_add(exe_combo, &exe);
    }
    combo_set(exe_combo, 0);
    btn(h, inst, "CSV出力", IDC_BTN_EXPORT);

    ctl(h, inst, w!("EDIT"), "", WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP, 0, 0, 0, 0, IDC_TAG_EDIT);
    btn(h, inst, "タグ保存", IDC_BTN_SAVE_TAG);

    // 下段: 再OCR/再翻訳エンジンのコンボと実行ボタン、選択削除
    let ocr_combo = combo(h, inst, IDC_OCR_COMBO);
    for (_, disp) in OCR_ENGINES {
        combo_add(ocr_combo, disp);
    }
    combo_set(ocr_combo, 0);
    btn(h, inst, "再OCR", IDC_BTN_REOCR);

    let tr_combo = combo(h, inst, IDC_TR_COMBO);
    for (_, disp) in TR_ENGINES {
        combo_add(tr_combo, disp);
    }
    combo_set(tr_combo, 0);
    btn(h, inst, "再翻訳", IDC_BTN_RETRANS);

    btn(h, inst, "選択した認識を削除", IDC_BTN_DEL_RECOG);
    btn(h, inst, "選択した翻訳を削除", IDC_BTN_DEL_TRANS);

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

thread_local! {
    /// 詳細エディットのサブクラス化前の元WNDPROC(Ctrl+Aの全選択対応のため)
    static DETAIL_OLDPROC: RefCell<isize> = const { RefCell::new(0) };
}

/// 詳細エディットをサブクラス化し、Ctrl+Aで全選択できるようにする
/// (標準EDITコントロールはCtrl+Cのコピーは既定で動作するがCtrl+Aは未対応のため)
fn subclass_detail(edit: HWND) {
    unsafe {
        let old = SetWindowLongPtrW(edit, GWLP_WNDPROC, detail_wndproc as *const () as isize);
        DETAIL_OLDPROC.with(|c| *c.borrow_mut() = old);
    }
}

unsafe extern "system" fn detail_wndproc(h: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    const EM_SETSEL: u32 = 0x00B1;
    if msg == WM_KEYDOWN && wparam.0 == 'A' as usize {
        let ctrl_down = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        if ctrl_down {
            unsafe {
                SendMessageW(h, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1isize)));
            }
            return LRESULT(0);
        }
    }
    let old = DETAIL_OLDPROC.with(|c| *c.borrow());
    unsafe {
        CallWindowProcW(
            std::mem::transmute::<isize, windows::Win32::UI::WindowsAndMessaging::WNDPROC>(old),
            h,
            msg,
            wparam,
            lparam,
        )
    }
}

fn btn(parent: HWND, inst: HINSTANCE, text: &str, id: i32) -> HWND {
    ctl(parent, inst, w!("BUTTON"), text, WS_TABSTOP, 0, 0, 0, 0, id)
}

fn dlg_item(h: HWND, id: i32) -> HWND {
    unsafe { GetDlgItem(Some(h), id).unwrap_or_default() }
}

fn combo(parent: HWND, inst: HINSTANCE, id: i32) -> HWND {
    ctl(parent, inst, w!("COMBOBOX"), "", WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32), 0, 0, 0, 0, id)
}

fn combo_add(cb: HWND, text: &str) {
    unsafe {
        let wide = to_wide(text);
        SendMessageW(cb, CB_ADDSTRING, Some(WPARAM(0)), Some(LPARAM(wide.as_ptr() as isize)));
    }
}

fn combo_set(cb: HWND, idx: usize) {
    unsafe {
        SendMessageW(cb, CB_SETCURSEL, Some(WPARAM(idx)), Some(LPARAM(0)));
    }
}

fn combo_sel(h: HWND, id: i32) -> usize {
    unsafe {
        let r = SendMessageW(dlg_item(h, id), CB_GETCURSEL, Some(WPARAM(0)), Some(LPARAM(0)));
        if r.0 < 0 { 0 } else { r.0 as usize }
    }
}

/// コンボの指定indexの項目文字列を取得
fn combo_item_text(h: HWND, id: i32, idx: usize) -> String {
    use windows::Win32::UI::WindowsAndMessaging::{CB_GETLBTEXT, CB_GETLBTEXTLEN};
    unsafe {
        let cb = dlg_item(h, id);
        let len = SendMessageW(cb, CB_GETLBTEXTLEN, Some(WPARAM(idx)), None).0;
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        SendMessageW(cb, CB_GETLBTEXT, Some(WPARAM(idx)), Some(LPARAM(buf.as_mut_ptr() as isize)));
        String::from_utf16_lossy(&buf[..len as usize])
    }
}

/// EDITコントロールの内容を取得 (最大1023文字)
fn edit_text(h: HWND, id: i32) -> String {
    unsafe {
        let mut buf = [0u16; 1024];
        let len = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(dlg_item(h, id), &mut buf);
        String::from_utf16_lossy(&buf[..len as usize])
    }
}

/// ウィンドウサイズに合わせて子コントロールを配置
fn layout(h: HWND) {
    unsafe {
        let mut rc = RECT::default();
        let _ = GetClientRect(h, &mut rc);
        let w = rc.right;
        let g = geometry(h);

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
        let gap = 6;

        // 検索行
        mv(IDC_SEARCH_EDIT, PAD, g.search.top, 200, BTN_H);
        mv(IDC_EXE_COMBO, PAD + 200 + gap, g.search.top, 150, 200);
        mv(IDC_BTN_EXPORT, PAD + 200 + gap + 150 + gap, g.search.top, 100, BTN_H);

        let rh = |r: &RECT| r.bottom - r.top;
        mv(IDC_RECOG_LV, g.recog.left, g.recog.top, w - PAD * 2, rh(&g.recog));
        mv(IDC_TRANS_LV, g.trans.left, g.trans.top, w - PAD * 2, rh(&g.trans));
        mv(IDC_DETAIL, g.detail_text.left, g.detail_text.top, g.detail_text.right - g.detail_text.left, rh(&g.detail_text).max(20));

        // タグ行
        mv(IDC_TAG_EDIT, PAD, g.tag_y, g.detail_text.right - PAD - gap - 80, BTN_H);
        mv(IDC_BTN_SAVE_TAG, g.detail_text.right - 80, g.tag_y, 80, BTN_H);

        // 上段ボタン行(表示切替・画像)
        let bw = 80;
        let y1 = g.row1_y;
        mv(IDC_BTN_SRC, PAD, y1, bw, BTN_H);
        mv(IDC_BTN_REQ, PAD + (bw + gap), y1, bw, BTN_H);
        mv(IDC_BTN_RES, PAD + (bw + gap) * 2, y1, bw, BTN_H);
        mv(IDC_BTN_EXP, PAD + (bw + gap) * 3, y1, bw, BTN_H);
        mv(IDC_BTN_IMG, PAD + (bw + gap) * 4, y1, bw, BTN_H);
        mv(IDC_BTN_REFRESH, w - (bw + gap) * 2 - PAD, y1, bw, BTN_H);
        mv(IDC_BTN_CLEAR, w - (bw + gap) - PAD, y1, bw + 8, BTN_H);

        // OCR結果リスト直下: 再OCR・削除ボタン行
        let y_ocr_btn = g.ocr_btn_y;
        let cbw = 110;
        let mut x = PAD;
        mv(IDC_OCR_COMBO, x, y_ocr_btn, cbw, 200);
        x += cbw + 4;
        mv(IDC_BTN_REOCR, x, y_ocr_btn, 80, BTN_H);
        x += 80 + gap;
        mv(IDC_BTN_DEL_RECOG, x, y_ocr_btn, 110, BTN_H);

        // 翻訳結果リスト直下: 再翻訳・削除ボタン行
        let y_trans_btn = g.trans_btn_y;
        let mut x = PAD;
        mv(IDC_TR_COMBO, x, y_trans_btn, cbw, 200);
        x += cbw + 4;
        mv(IDC_BTN_RETRANS, x, y_trans_btn, 80, BTN_H);
        x += 80 + gap;
        mv(IDC_BTN_DEL_TRANS, x, y_trans_btn, 110, BTN_H);
    }
}

const PAD: i32 = 8;
const BTN_H: i32 = 28;
const SPLITTER: i32 = 6;

/// 各領域の矩形(レイアウト・描画・ヒットテストで共有)
struct Geo {
    search: RECT,
    recog: RECT,
    sp1: RECT,
    trans: RECT,
    sp2: RECT,
    detail_text: RECT,
    img: RECT,
    tag_y: i32,
    row1_y: i32,
    ocr_btn_y: i32,
    trans_btn_y: i32,
}

fn geometry(h: HWND) -> Geo {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(h, &mut rc);
    }
    let w = rc.right;
    let ht = rc.bottom;
    let (split_a, split_b) = STATE.with(|s| {
        let st = s.borrow();
        (st.split_a, st.split_b)
    });
    
    let row2_y = ht - BTN_H - PAD;
    let row1_y = row2_y - BTN_H - 4;
    let tag_y = row1_y - BTN_H - 4;
    
    let area_top = PAD + BTN_H + PAD;
    let area_bottom = tag_y - PAD;
    let area_h = (area_bottom - area_top).max(60);

    let search = RECT { left: PAD, top: PAD, right: w - PAD, bottom: PAD + BTN_H };

    // OCR結果リストの底 + ボタンエリア
    let recog_bottom = area_top + (area_h as f32 * split_a) as i32;
    let ocr_btn_y = recog_bottom + PAD;
    let sp1_top = ocr_btn_y + BTN_H + PAD;

    // 翻訳結果リストの計算(split_bはarea_topからの相対位置) + 翻訳操作ボタン行
    let trans_bottom = area_top + (area_h as f32 * split_b) as i32;
    let trans_btn_y = trans_bottom + PAD;
    let sp2_top = trans_btn_y + BTN_H + PAD;
    let detail_top = sp2_top + SPLITTER;

    let lw = w - PAD * 2;

    let recog = RECT { left: PAD, top: area_top, right: PAD + lw, bottom: recog_bottom };
    let sp1 = RECT { left: PAD, top: sp1_top, right: PAD + lw, bottom: sp1_top + SPLITTER };
    let trans = RECT { left: PAD, top: sp1_top + SPLITTER, right: PAD + lw, bottom: trans_bottom };
    let sp2 = RECT { left: PAD, top: sp2_top, right: PAD + lw, bottom: sp2_top + SPLITTER };
    let text_w = (w as f32 * 0.60) as i32 - PAD;
    let detail_text = RECT { left: PAD, top: detail_top, right: PAD + text_w, bottom: area_bottom };
    let img_left = PAD + text_w + PAD;
    let img = RECT { left: img_left, top: detail_top, right: w - PAD, bottom: area_bottom };
    Geo { search, recog, sp1, trans, sp2, detail_text, img, tag_y, row1_y, ocr_btn_y, trans_btn_y }
}

fn in_rect(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn fmt_ts(ts_ms: i64) -> String {
    // 日本時間 (JST = UTC + 9時間) に補正
    let jst_ms = ts_ms.max(0) + 9 * 3600 * 1000;
    let secs = jst_ms / 1000;
    
    // エポック (1970-01-01) からの経過日数と、その日の時分秒
    let mut days = secs / 86400;
    let tod = secs % 86400;
    let (h, m, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);

    // 1970年からの年月日計算 (うるう年を考慮)
    let mut year = 1970;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
        let year_days = if leap { 366 } else { 365 };
        if days >= year_days {
            days -= year_days;
            year += 1;
        } else {
            break;
        }
    }

    let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let month_days = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    for &md in &month_days {
        if days >= md {
            days -= md;
            month += 1;
        } else {
            break;
        }
    }
    let day = days + 1;

    format!("{year:04}/{month:02}/{day:02} {h:02}:{m:02}:{s:02}")
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

fn lv_select(lvh: HWND, idx: i32) {
    unsafe {
        let mut item = LVITEMW {
            mask: LVIF_STATE,
            iItem: idx,
            state: LIST_VIEW_ITEM_STATE_FLAGS(0x0003),  // LVIS_SELECTED | LVIS_FOCUSED
            stateMask: LIST_VIEW_ITEM_STATE_FLAGS(0x0003),
            ..Default::default()
        };
        SendMessageW(lvh, LVM_SETITEMW, Some(WPARAM(0)), Some(LPARAM(&mut item as *mut _ as isize)));
        SendMessageW(lvh, LVM_ENSUREVISIBLE, Some(WPARAM(idx as usize)), Some(LPARAM(0)));
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

/// DBから再読込して認識一覧を更新 (検索欄・exeフィルタを適用; SPEC v0.3 §2.3.2)
fn reload() {
    let h = hwnd();
    let query = edit_text(h, IDC_SEARCH_EDIT);
    let exe_idx = combo_sel(h, IDC_EXE_COMBO);
    // index 0 は「全アプリ」
    let app_exe = if exe_idx == 0 { String::new() } else { combo_item_text(h, IDC_EXE_COMBO, exe_idx) };

    let recogs = logdb::search_recognitions(&query, &app_exe, 1000);
    let recog_lv = dlg_item(h, IDC_RECOG_LV);
    lv_clear(recog_lv);
    for (i, r) in recogs.iter().enumerate() {
        let img = if r.image_path.is_some() { "✓" } else { "" };
        let text = if r.success { r.source_text.clone() } else { format!("[エラー] {}", r.error) };
        lv_add_row(recog_lv, i as i32, &[
            fmt_ts(r.ts_ms),
            truncate(r.app_title.as_deref().unwrap_or(""), 20),
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
            t.translated_text.clone()
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

    // タグセット
    let tags = logdb::get_explanation_and_tags(recog_id).map(|(_, t)| t).unwrap_or_default();
    unsafe {
        let wide = to_wide(&tags);
        let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
            dlg_item(h, IDC_TAG_EDIT),
            PCWSTR(wide.as_ptr()),
        );
    }

    let has_trans = !trans.is_empty();
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.trans = trans;
        st.sel_recog = Some(idx);
        st.sel_trans = if has_trans { Some(0) } else { None };
        st.image = image;
    });
    if has_trans {
        lv_select(trans_lv, 0);
        refresh_detail();
    } else {
        set_detail("");
    }
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
                let app = st.sel_recog.and_then(|ri| st.recogs.get(ri)).and_then(|r| r.app_title.clone()).unwrap_or_default();
                let uia = st.sel_recog.and_then(|ri| st.recogs.get(ri)).and_then(|r| r.uia_path.clone()).unwrap_or_default();
                let body = if t.success { t.translated_text.clone() } else { format!("[エラー] {}", t.error) };
                
                let mut info = String::new();
                if !app.is_empty() {
                    info.push_str(&format!("【対象アプリ】\r\n{}\r\n", app));
                }
                if !uia.is_empty() {
                    info.push_str(&format!("【UIAパス】\r\n{}\r\n", uia));
                }
                if !info.is_empty() {
                    info.push_str("\r\n");
                }
                
                format!("{}【原文】\r\n{}\r\n\r\n【訳文】\r\n{}", info, src, body)
            }
            DetailView::Request => {
                if t.request_json.is_empty() { "(送信JSONなし: ローカル翻訳またはキャッシュ)".into() }
                else { pretty_json(&t.request_json) }
            }
            DetailView::Response => {
                if t.response_json.is_empty() { "(受信JSONなし)".into() }
                else { pretty_json(&t.response_json) }
            }
            DetailView::Explanation => {
                if let Some(ri) = st.sel_recog {
                    if let Some(r) = st.recogs.get(ri) {
                        let (text, _tags) = logdb::get_explanation_and_tags(r.id).unwrap_or_default();
                        if text.is_empty() { "(解説なし)".into() } else { text }
                    } else { String::new() }
                } else { String::new() }
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
    // geometry() は STATE を借用するので、image の借用より先に取得する
    let g = geometry(h);
    STATE.with(|s| {
        let st = s.borrow();
        let Some((iw, ih, rgba)) = st.image.as_ref() else { return };
        unsafe {
            let img_left = g.img.left;
            let detail_top = g.img.top;
            let img_w = (g.img.right - g.img.left).max(1);
            let img_h = (g.img.bottom - g.img.top).max(20);

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

/// スプリッター2本を薄い罫線で描く(ドラッグ位置の視認用)
fn paint_splitters(h: HWND) {
    let g = geometry(h);
    unsafe {
        let hdc = windows::Win32::Graphics::Gdi::GetDC(Some(h));
        let brush = windows::Win32::Graphics::Gdi::CreateSolidBrush(COLORREF(0x00909090));
        windows::Win32::Graphics::Gdi::FillRect(hdc, &g.sp1, brush);
        windows::Win32::Graphics::Gdi::FillRect(hdc, &g.sp2, brush);
        let _ = windows::Win32::Graphics::Gdi::DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(brush.0));
        let _ = windows::Win32::Graphics::Gdi::ReleaseDC(Some(h), hdc);
    }
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

/// 保存PNG(RGBA)を capture::Captured(BGRA)へ変換
fn rgba_to_captured(iw: u32, ih: u32, rgba: &[u8]) -> crate::capture::Captured {
    let mut bgra = vec![0u8; rgba.len()];
    for (o, px) in bgra.chunks_mut(4).zip(rgba.chunks(4)) {
        o[0] = px[2];
        o[1] = px[1];
        o[2] = px[0];
        o[3] = px[3];
    }
    crate::capture::Captured { width: iw, height: ih, bgra }
}

/// 選択した認識ログの画像を、指定エンジンで再OCRして新規ログに追記する(ワーカースレッド)。
fn start_reocr(h: HWND) {
    let sel = STATE.with(|s| {
        let st = s.borrow();
        st.sel_recog.and_then(|i| st.recogs.get(i)).map(|r| (r.id, r.image_path.clone()))
    });
    let Some((recog_id, image_path)) = sel else {
        unsafe { MessageBoxW(Some(h), w!("認識ログを選択してください。"), w!("再OCR"), MB_OK); }
        return;
    };
    let Some(rel) = image_path else {
        unsafe { MessageBoxW(Some(h), w!("この認識ログには画像がありません(デバッグモードで記録した画像のみ再OCRできます)。"), w!("再OCR"), MB_OK); }
        return;
    };
    let engine = OCR_ENGINES[combo_sel(h, IDC_OCR_COMBO).min(OCR_ENGINES.len() - 1)].0.to_string();
    let hwnd_isize = h.0 as isize;
    let _ = recog_id;
    std::thread::spawn(move || {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }
        let cfg = crate::config::Config::load();
        let path = logdb::logs_dir().join(&rel);
        if let Some((iw, ih, rgba)) = decode_png(&path) {
            let cap = rgba_to_captured(iw, ih, &rgba);
            let t0 = std::time::Instant::now();
            let (text, err): (Option<String>, Option<String>) =
                match crate::ocr::run(&engine, &cfg, &cap, crate::ocr::Focus::All) {
                    Ok(o) => (Some(o.text), None),
                    Err(e) => (None, Some(e)),
                };
            let ms = t0.elapsed().as_millis();
            // 再OCR結果を新規認識ログとして追記(デバッグ時は画像も再保存)
            logdb::log_recognition(
                "review", "ocr", &engine, ms, text.as_deref(), err.as_deref(),
                Some(&cap), cfg.debug_mode, None, None, None
            );
        }
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                Some(HWND(hwnd_isize as *mut _)),
                WM_APP_RELOAD,
                WPARAM(0),
                LPARAM(0),
            );
        }
    });
}

/// 選択した翻訳ログ(の属する認識の原文)を、指定エンジンで再翻訳して追記する(ワーカースレッド)。
fn start_retranslate(h: HWND) {
    let sel = STATE.with(|s| {
        let st = s.borrow();
        st.sel_recog.and_then(|i| st.recogs.get(i)).map(|r| (r.id, r.source_text.clone()))
    });
    let Some((recog_id, source)) = sel else {
        unsafe { MessageBoxW(Some(h), w!("認識ログを選択してください。"), w!("再翻訳"), MB_OK); }
        return;
    };
    if source.trim().is_empty() {
        unsafe { MessageBoxW(Some(h), w!("原文が空のため再翻訳できません。"), w!("再翻訳"), MB_OK); }
        return;
    }
    let engine = TR_ENGINES[combo_sel(h, IDC_TR_COMBO).min(TR_ENGINES.len() - 1)].0.to_string();
    let hwnd_isize = h.0 as isize;
    std::thread::spawn(move || {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }
        let cfg = crate::config::Config::load();
        let t0 = std::time::Instant::now();
        match crate::translate::translate(&engine, &cfg, &source) {
            Ok(t) => {
                let ms = t0.elapsed().as_millis();
                logdb::log_translation(
                    Some(recog_id), &t.engine, &t.source_lang, &t.target_lang, ms, t.cache_hit,
                    Some(&t.text), None, t.detail.request_json.as_deref(),
                    t.detail.response_json.as_deref(), t.detail.tokens_in, t.detail.tokens_out,
                );
            }
            Err(e) => {
                let ms = t0.elapsed().as_millis();
                logdb::log_translation(
                    Some(recog_id), &engine, &cfg.source_lang, &cfg.target_lang, ms, false,
                    None, Some(&e), None, None, None, None,
                );
            }
        }
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                Some(HWND(hwnd_isize as *mut _)),
                WM_APP_RELOAD,
                WPARAM(0),
                LPARAM(0),
            );
        }
    });
}

// ---- 画像1:1表示ウィンドウ ----

thread_local! {
    static IMG1: RefCell<Option<(u32, u32, Vec<u8>)>> = const { RefCell::new(None) };
    static IMG1_SCROLL: RefCell<(i32, i32)> = const { RefCell::new((0, 0)) };
    static IMG1_REGISTERED: RefCell<bool> = const { RefCell::new(false) };
    static IMG1_HWND: RefCell<Option<isize>> = const { RefCell::new(None) };
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

/// 現在の画像を原寸(1:1)表示する別ウィンドウを開く(既存があれば再利用し1つまでに制限)
fn open_image_1to1(parent: HWND) {
    let img = STATE.with(|s| s.borrow().image.clone());
    let Some((iw, ih, _)) = img.as_ref().map(|(a, b, _)| (*a, *b, ())) else { return };
    IMG1.with(|c| *c.borrow_mut() = img);
    IMG1_SCROLL.with(|c| *c.borrow_mut() = (0, 0));

    // 既存の1:1表示ウィンドウがあれば再利用する(2つ以上開かない)
    let existing = IMG1_HWND.with(|c| *c.borrow());
    if let Some(raw) = existing {
        let h = HWND(raw as *mut _);
        if unsafe { IsWindow(Some(h)) }.as_bool() {
            let cw = (iw as i32 + 20).min(1400);
            let ch = (ih as i32 + 40).min(900);
            let (x, y) = place_beside_parent(parent, cw, ch);
            unsafe {
                let _ = SetWindowPos(h, None, x, y, cw, ch, SWP_NOACTIVATE);
                let _ = InvalidateRect(Some(h), None, true);
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
        let class = w!("FocusTranslatorImageView");
        IMG1_REGISTERED.with(|r| {
            if !*r.borrow() {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(img_wndproc),
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
        if let Ok(iwnd) = CreateWindowExW(
            WS_EX_TOPMOST,
            class,
            w!("画像 (原寸 1:1 / ホイール・矢印でスクロール)"),
            WS_OVERLAPPEDWINDOW,
            x,
            y,
            cw,
            ch,
            Some(parent),
            None,
            Some(inst),
            None,
        ) {
            IMG1_HWND.with(|c| *c.borrow_mut() = Some(iwnd.0 as isize));
            let _ = ShowWindow(iwnd, SW_SHOW);
        }
    }
}

unsafe extern "system" fn img_wndproc(h: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{WM_KEYDOWN, WM_MOUSEWHEEL};
    // スクロールは SCROLLINFO を使わず、ホイール/矢印キーでオフセットを動かす簡易方式
    match msg {
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let shift = (wparam.0 & 0x0004) != 0; // MK_SHIFT で横スクロール
            IMG1_SCROLL.with(|c| {
                let mut sc = c.borrow_mut();
                let step = delta / 120 * 48;
                if shift { sc.0 = (sc.0 - step).max(0); } else { sc.1 = (sc.1 - step).max(0); }
            });
            unsafe { let _ = InvalidateRect(Some(h), None, true); }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = wparam.0 as i32;
            IMG1_SCROLL.with(|c| {
                let mut sc = c.borrow_mut();
                match vk {
                    0x25 => sc.0 = (sc.0 - 40).max(0), // ←
                    0x27 => sc.0 += 40,                // →
                    0x26 => sc.1 = (sc.1 - 40).max(0), // ↑
                    0x28 => sc.1 += 40,                // ↓
                    _ => {}
                }
            });
            unsafe { let _ = InvalidateRect(Some(h), None, true); }
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_PAINT => {
            unsafe {
                let mut ps = windows::Win32::Graphics::Gdi::PAINTSTRUCT::default();
                let hdc = windows::Win32::Graphics::Gdi::BeginPaint(h, &mut ps);
                IMG1.with(|c| {
                    if let Some((iw, ih, rgba)) = c.borrow().as_ref() {
                        let (sx, sy) = IMG1_SCROLL.with(|s| *s.borrow());
                        let mut bgra = vec![0u8; rgba.len()];
                        for (o, px) in bgra.chunks_mut(4).zip(rgba.chunks(4)) {
                            o[0] = px[2]; o[1] = px[1]; o[2] = px[0]; o[3] = px[3];
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
                        // 原寸(1:1)でスクロールオフセット分ずらして描画
                        StretchDIBits(
                            hdc, -sx, -sy, *iw as i32, *ih as i32,
                            0, 0, *iw as i32, *ih as i32,
                            Some(bgra.as_ptr() as *const _), &bmi, DIB_RGB_COLORS,
                            windows::Win32::Graphics::Gdi::SRCCOPY,
                        );
                    }
                });
                let _ = windows::Win32::Graphics::Gdi::EndPaint(h, &ps);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe { let _ = DestroyWindow(h); }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(h, msg, wparam, lparam) },
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
                paint_splitters(h);
                let _ = windows::Win32::Graphics::Gdi::EndPaint(h, &ps);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let g = geometry(h);
            if in_rect(&g.sp1, x, y) {
                STATE.with(|s| s.borrow_mut().dragging = 1);
                unsafe { SetCapture(h); }
            } else if in_rect(&g.sp2, x, y) {
                STATE.with(|s| s.borrow_mut().dragging = 2);
                unsafe { SetCapture(h); }
            } else if in_rect(&g.img, x, y) {
                // 画像領域クリック → 原寸(1:1)表示ウィンドウ
                open_image_1to1(h);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let dragging = STATE.with(|s| s.borrow().dragging);
            if dragging != 0 {
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                let mut rc = RECT::default();
                unsafe { let _ = GetClientRect(h, &mut rc); }
                let ht = rc.bottom;
                let area_top = PAD;
                let area_bottom = (ht - BTN_H - PAD) - (BTN_H + 4) - PAD;
                let area_h = (area_bottom - area_top).max(60) as f32;
                let frac = ((y - area_top) as f32 / area_h).clamp(0.1, 0.9);
                STATE.with(|s| {
                    let mut st = s.borrow_mut();
                    if dragging == 1 {
                        st.split_a = frac.min(st.split_b - 0.05);
                    } else {
                        st.split_b = frac.max(st.split_a + 0.05);
                    }
                });
                layout(h);
                unsafe { let _ = InvalidateRect(Some(h), None, true); }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let was = STATE.with(|s| {
                let mut st = s.borrow_mut();
                let d = st.dragging;
                st.dragging = 0;
                d
            });
            if was != 0 {
                unsafe { let _ = ReleaseCapture(); }
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            // スプリッター上では上下リサイズカーソル
            let mut pt = windows::Win32::Foundation::POINT::default();
            unsafe { let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt); }
            let mut cpt = pt;
            unsafe { let _ = windows::Win32::Graphics::Gdi::ScreenToClient(h, &mut cpt); }
            let g = geometry(h);
            if in_rect(&g.sp1, cpt.x, cpt.y) || in_rect(&g.sp2, cpt.x, cpt.y) {
                unsafe {
                    if let Ok(c) = LoadCursorW(None, IDC_SIZENS) {
                        SetCursor(Some(c));
                    }
                }
                return LRESULT(1);
            }
            unsafe { DefWindowProcW(h, msg, wparam, lparam) }
        }
        WM_APP_RELOAD => {
            // 再OCR/再翻訳後のリロード: 前の選択アイテムを復元する
            let sel_recog_before = STATE.with(|s| s.borrow().sel_recog);
            reload();
            // 前の選択インデックスを復元(アイテムが削除されている可能性も考慮)
            if let Some(old_idx) = sel_recog_before {
                let recog_lv = dlg_item(h, IDC_RECOG_LV);
                let count = unsafe {
                    SendMessageW(recog_lv, windows::Win32::UI::Controls::LVM_GETITEMCOUNT, Some(WPARAM(0)), Some(LPARAM(0))).0 as i32
                };
                if count > 0 {
                    let new_idx = if (old_idx as i32) < count { old_idx } else { 0 };
                    lv_select(recog_lv, new_idx as i32);
                    on_recog_selected(new_idx);
                }
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
            } else if nmhdr.code == LVN_KEYDOWN {
                let key_nm = unsafe { &*(lparam.0 as *const NMLVKEYDOWN) };
                if key_nm.wVKey == VK_DELETE.0 {
                    let id = nmhdr.idFrom as i32;
                    if id == IDC_RECOG_LV {
                        unsafe { let _ = SendMessageW(h, WM_COMMAND, Some(WPARAM(IDC_BTN_DEL_RECOG as usize)), None); }
                    } else if id == IDC_TRANS_LV {
                        unsafe { let _ = SendMessageW(h, WM_COMMAND, Some(WPARAM(IDC_BTN_DEL_TRANS as usize)), None); }
                    }
                }
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = (wparam.0 >> 16) & 0xFFFF;
            if id == IDC_SEARCH_EDIT && code == 0x0300 /* EN_CHANGE */ {
                reload();
                return LRESULT(0);
            }
            if id == IDC_EXE_COMBO && code == 1 /* CBN_SELCHANGE */ {
                reload();
                return LRESULT(0);
            }
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
                IDC_BTN_REOCR => start_reocr(h),
                IDC_BTN_RETRANS => start_retranslate(h),
                IDC_BTN_DEL_RECOG => {
                    let sel_idx = STATE.with(|s| s.borrow().sel_recog);
                    let id = STATE.with(|s| {
                        let st = s.borrow();
                        st.sel_recog.and_then(|i| st.recogs.get(i)).map(|r| r.id)
                    });
                    if let Some(id) = id {
                        logdb::delete_recognition(id);
                        reload();
                        // 削除後、次のアイテムをフォーカス
                        if let Some(old_idx) = sel_idx {
                            let recog_lv = dlg_item(h, IDC_RECOG_LV);
                            let count = unsafe {
                                SendMessageW(recog_lv, windows::Win32::UI::Controls::LVM_GETITEMCOUNT, Some(WPARAM(0)), Some(LPARAM(0))).0 as i32
                            };
                            let new_idx = if (old_idx as i32) < count { old_idx } else if count > 0 { (count - 1) as usize } else { 0 };
                            if count > 0 && (new_idx as i32) < count {
                                lv_select(recog_lv, new_idx as i32);
                                on_recog_selected(new_idx);
                            }
                        }
                    }
                }
                IDC_BTN_DEL_TRANS => {
                    let (tid, recog_idx, sel_trans_idx) = STATE.with(|s| {
                        let st = s.borrow();
                        let tid = st.sel_trans.and_then(|i| st.trans.get(i)).map(|t| t.id);
                        (tid, st.sel_recog, st.sel_trans)
                    });
                    if let Some(tid) = tid {
                        logdb::delete_translation(tid);
                        // 翻訳候補一覧だけ更新
                        if let Some(idx) = recog_idx {
                            on_recog_selected(idx);
                            // 削除後、次のアイテムをフォーカス
                            if let Some(old_idx) = sel_trans_idx {
                                let trans_lv = dlg_item(h, IDC_TRANS_LV);
                                let count = unsafe {
                                    SendMessageW(trans_lv, windows::Win32::UI::Controls::LVM_GETITEMCOUNT, Some(WPARAM(0)), Some(LPARAM(0))).0 as i32
                                };
                                let new_idx = if (old_idx as i32) < count { old_idx } else if count > 0 { (count - 1) as usize } else { 0 };
                                if count > 0 && (new_idx as i32) < count {
                                    lv_select(trans_lv, new_idx as i32);
                                    on_trans_selected(new_idx);
                                }
                            }
                        }
                    }
                }
                IDC_BTN_EXP => {
                    STATE.with(|s| s.borrow_mut().detail_view = DetailView::Explanation);
                    refresh_detail();
                }
                IDC_BTN_SAVE_TAG => {
                    let recog_id = STATE.with(|s| {
                        let st = s.borrow();
                        st.sel_recog.and_then(|idx| st.recogs.get(idx).map(|r| r.id))
                    });
                    if let Some(id) = recog_id {
                        let tags = edit_text(h, IDC_TAG_EDIT);
                        let (exp, _) = logdb::get_explanation_and_tags(id).unwrap_or_default();
                        logdb::save_explanation_and_tags(id, &exp, &tags);
                        unsafe { MessageBoxW(Some(h), w!("タグを保存しました。"), w!("タグ保存"), MB_OK); }
                    }
                }
                IDC_BTN_EXPORT => {
                    let path = logdb::logs_dir().join("export.csv");
                    if let Ok(mut f) = std::fs::File::create(&path) {
                        use std::io::Write;
                        let _ = writeln!(f, "\u{FEFF}ID,日時,エンジン,原文,訳文,タグ,解説");
                        let recogs = STATE.with(|s| s.borrow().recogs.clone());
                        for r in recogs {
                            let trans = logdb::translations_for(r.id);
                            let (exp, tags) = logdb::get_explanation_and_tags(r.id).unwrap_or_default();
                            let tr_text = trans.first().map(|t| t.translated_text.clone()).unwrap_or_default();
                            let tr_eng = trans.first().map(|t| t.engine.clone()).unwrap_or_default();
                            let escape = |s: &str| format!("\"{}\"", s.replace("\"", "\"\""));
                            let _ = writeln!(f, "{},{},{},{},{},{},{}", r.id, fmt_ts(r.ts_ms), tr_eng, escape(&r.source_text), escape(&tr_text), escape(&tags), escape(&exp));
                        }
                        unsafe {
                            let wide = to_wide(&path.to_string_lossy());
                            let _ = ShellExecuteW(None, w!("open"), PCWSTR(wide.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
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
