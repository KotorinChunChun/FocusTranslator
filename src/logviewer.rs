// ログビューア (SPECv0.4 §9: 4工程ツリー構造のブロック表示)
// 上段3ブロック: 【入力内容】captures →【読み取り結果】recognitions →【翻訳結果】translations
// 下段1ブロック: 【解説結果】explanations
// 各ブロックは「左: リストビュー / 右: 詳細テキスト(入力ブロックは画像も)」の構成。
// 検索行(部分一致・exeフィルタ・全削除等)は上部にウィンドウ全幅で配置する。
use crate::logdb::{self, CaptureRow, ExplainRow, RecogRow, TransRow};
use crate::ui_helpers::*;
use crate::util::to_wide;
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
    LVIF_STATE, LVIF_TEXT, LVITEMW, LIST_VIEW_ITEM_STATE_FLAGS, LVM_DELETEALLITEMS,
    LVM_GETITEMCOUNT, LVM_GETNEXTITEM, LVM_INSERTCOLUMNW, LVM_INSERTITEMW,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMTEXTW, LVM_SETITEMW, LVM_ENSUREVISIBLE,
    LVN_ITEMCHANGED, LVN_KEYDOWN, NMLVKEYDOWN, LVS_EX_FULLROWSELECT, LVS_REPORT,
    LVS_SHOWSELALWAYS, LVS_SINGLESEL, NMHDR,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_DELETE};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBS_DROPDOWNLIST, CW_USEDEFAULT, CallWindowProcW,
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_WNDPROC, GetClientRect, GetDlgItem,
    GetWindowRect, HMENU, IDC_ARROW, IsWindow, LoadCursorW, MB_ICONQUESTION, MB_OK, MB_YESNO,
    MessageBoxW, SWP_NOACTIVATE, SW_SHOW, SW_SHOWNORMAL, SendMessageW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, WINDOW_STYLE, WM_APP, WM_CLOSE,
    WM_COMMAND, WM_DESTROY, WM_KEYDOWN, WM_LBUTTONDOWN, WM_NOTIFY, WM_SIZE, WNDCLASSW, WS_BORDER,
    WS_CHILD, WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, w};

// リストビュー / 詳細テキスト (工程ブロックごと)
const IDC_CAP_LV: i32 = 201;
const IDC_RECOG_LV: i32 = 202;
const IDC_TRANS_LV: i32 = 203;
const IDC_EXP_LV: i32 = 204;
const IDC_CAP_DETAIL: i32 = 205;
const IDC_RECOG_DETAIL: i32 = 206;
const IDC_TRANS_DETAIL: i32 = 207;
const IDC_EXP_DETAIL: i32 = 208;
// ブロック見出しラベル
const IDC_LBL_CAP: i32 = 240;
const IDC_LBL_RECOG: i32 = 241;
const IDC_LBL_TRANS: i32 = 242;
const IDC_LBL_EXP: i32 = 243;
// 検索行
const IDC_SEARCH_EDIT: i32 = 230;
const IDC_EXE_COMBO: i32 = 231;
const IDC_BTN_EXPORT: i32 = 232;
const IDC_BTN_REFRESH: i32 = 214;
const IDC_BTN_CLEAR: i32 = 215;
// ブロック下部の操作ボタン
const IDC_BTN_DEL_CAP: i32 = 224;
const IDC_OCR_COMBO: i32 = 220;
const IDC_BTN_REOCR: i32 = 222;
const IDC_BTN_DEL_RECOG: i32 = 225;
const IDC_TAG_EDIT: i32 = 234;
const IDC_BTN_SAVE_TAG: i32 = 235;
const IDC_TR_COMBO: i32 = 221;
const IDC_BTN_RETRANS: i32 = 223;
const IDC_BTN_DEL_TRANS: i32 = 226;

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

struct State {
    caps: Vec<CaptureRow>,
    recogs: Vec<RecogRow>,
    trans: Vec<TransRow>,
    exps: Vec<ExplainRow>,
    sel_cap: Option<usize>,
    sel_recog: Option<usize>,
    sel_trans: Option<usize>,
    sel_exp: Option<usize>,
    /// 現在表示中画像のデコード済みRGBA (幅, 高さ, ピクセル)
    image: Option<(u32, u32, Vec<u8>)>,
}

thread_local! {
    static WND: RefCell<isize> = const { RefCell::new(0) };
    static REGISTERED: RefCell<bool> = const { RefCell::new(false) };
    static STATE: RefCell<State> = const { RefCell::new(State {
        caps: Vec::new(), recogs: Vec::new(), trans: Vec::new(), exps: Vec::new(),
        sel_cap: None, sel_recog: None, sel_trans: None, sel_exp: None,
        image: None,
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
            1520,
            860,
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

/// 複数行・読み取り専用・折返しの詳細エディットを作る
fn detail_edit(parent: HWND, inst: HINSTANCE, id: i32) {
    unsafe {
        const ES_MULTILINE: u32 = 0x0004;
        const ES_READONLY: u32 = 0x0800;
        const ES_AUTOVSCROLL: u32 = 0x0040;
        if let Ok(e) = CreateWindowExW(
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
            Some(parent),
            Some(HMENU(id as usize as *mut _)),
            Some(inst),
            None,
        ) {
            subclass_detail(e);
        }
    }
}

fn label(parent: HWND, inst: HINSTANCE, text: &str, id: i32) {
    ctl(parent, inst, w!("STATIC"), text, Default::default(), 0, 0, 0, 0, id);
}

fn build(h: HWND, inst: HINSTANCE) {
    // 検索行
    ctl(h, inst, w!("EDIT"), "", WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP, 0, 0, 0, 0, IDC_SEARCH_EDIT);
    let exe_combo = combo(h, inst, IDC_EXE_COMBO);
    combo_add(exe_combo, "全アプリ");
    for exe in logdb::get_unique_app_exes() {
        combo_add(exe_combo, &exe);
    }
    combo_set(exe_combo, 0);
    btn(h, inst, "CSV出力", IDC_BTN_EXPORT);
    btn(h, inst, "最新に更新", IDC_BTN_REFRESH);
    btn(h, inst, "ログを全削除", IDC_BTN_CLEAR);

    // 【入力内容】ブロック
    label(h, inst, "【入力内容】", IDC_LBL_CAP);
    let cap = lv(h, inst, IDC_CAP_LV);
    add_col(cap, 0, "日時", 120);
    add_col(cap, 1, "モード", 50);
    add_col(cap, 2, "アプリ", 90);
    add_col(cap, 3, "画像", 40);
    detail_edit(h, inst, IDC_CAP_DETAIL);
    btn(h, inst, "選択削除", IDC_BTN_DEL_CAP);

    // 【読み取り結果】ブロック
    label(h, inst, "【読み取り結果】", IDC_LBL_RECOG);
    let recog = lv(h, inst, IDC_RECOG_LV);
    add_col(recog, 0, "日時", 120);
    add_col(recog, 1, "エンジン", 60);
    add_col(recog, 2, "ms", 45);
    add_col(recog, 3, "認識テキスト", 200);
    detail_edit(h, inst, IDC_RECOG_DETAIL);
    let ocr_combo = combo(h, inst, IDC_OCR_COMBO);
    for (_, disp) in OCR_ENGINES {
        combo_add(ocr_combo, disp);
    }
    combo_set(ocr_combo, 0);
    btn(h, inst, "再OCR", IDC_BTN_REOCR);
    btn(h, inst, "削除", IDC_BTN_DEL_RECOG);
    ctl(h, inst, w!("EDIT"), "", WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP, 0, 0, 0, 0, IDC_TAG_EDIT);
    btn(h, inst, "タグ保存", IDC_BTN_SAVE_TAG);

    // 【翻訳結果】ブロック
    label(h, inst, "【翻訳結果】", IDC_LBL_TRANS);
    let trans = lv(h, inst, IDC_TRANS_LV);
    add_col(trans, 0, "日時", 120);
    add_col(trans, 1, "エンジン", 60);
    add_col(trans, 2, "方向", 55);
    add_col(trans, 3, "訳文", 200);
    detail_edit(h, inst, IDC_TRANS_DETAIL);
    let tr_combo = combo(h, inst, IDC_TR_COMBO);
    for (_, disp) in TR_ENGINES {
        combo_add(tr_combo, disp);
    }
    combo_set(tr_combo, 0);
    btn(h, inst, "再翻訳", IDC_BTN_RETRANS);
    btn(h, inst, "削除", IDC_BTN_DEL_TRANS);

    // 【解説結果】ブロック (下段全幅)
    label(h, inst, "【解説結果】", IDC_LBL_EXP);
    let exp = lv(h, inst, IDC_EXP_LV);
    add_col(exp, 0, "日時", 120);
    add_col(exp, 1, "プロファイル", 90);
    add_col(exp, 2, "ms", 50);
    add_col(exp, 3, "tok入/出", 70);
    add_col(exp, 4, "解説文", 320);
    detail_edit(h, inst, IDC_EXP_DETAIL);

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

const PAD: i32 = 8;
const BTN_H: i32 = 28;
const LBL_H: i32 = 18;

/// 各領域の矩形(レイアウト・描画・ヒットテストで共有)
struct Geo {
    /// 上段3列それぞれの (リスト, 詳細) 矩形
    cap_list: RECT,
    cap_text: RECT,
    cap_img: RECT,
    recog_list: RECT,
    recog_text: RECT,
    trans_list: RECT,
    trans_text: RECT,
    exp_list: RECT,
    exp_text: RECT,
    /// 各列の見出しY / ボタン行Y
    label_y: i32,
    btn_y: i32,
    exp_label_y: i32,
    /// 各列の左端X・列幅
    col_x: [i32; 3],
    col_w: i32,
}

fn geometry(h: HWND) -> Geo {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(h, &mut rc);
    }
    let w = rc.right.max(600);
    let ht = rc.bottom.max(400);

    let search_bottom = PAD + BTN_H;
    let label_y = search_bottom + PAD;
    let upper_top = label_y + LBL_H + 2;

    // 下段(解説)ブロックの高さは全体の約28%
    let exp_h = (ht as f32 * 0.28) as i32;
    let exp_label_y = ht - exp_h - PAD;
    let exp_top = exp_label_y + LBL_H + 2;
    let exp_bottom = ht - PAD;

    // 上段ブロックの底 = ボタン行の上
    let btn_y = exp_label_y - PAD - BTN_H;
    let upper_bottom = btn_y - 4;

    let col_w = (w - PAD * 4) / 3;
    let col_x = [PAD, PAD * 2 + col_w, PAD * 3 + col_w * 2];

    // 各列: 左45%リスト / 右55%詳細
    let list_w = (col_w as f32 * 0.45) as i32;
    let block = |cx: i32| {
        (
            RECT { left: cx, top: upper_top, right: cx + list_w, bottom: upper_bottom },
            RECT { left: cx + list_w + 4, top: upper_top, right: cx + col_w, bottom: upper_bottom },
        )
    };
    let (cap_list, cap_full) = block(col_x[0]);
    let (recog_list, recog_text) = block(col_x[1]);
    let (trans_list, trans_text) = block(col_x[2]);

    // 入力ブロックの右側: 上半分テキスト / 下半分画像 (§9.1)
    let mid = (cap_full.top + cap_full.bottom) / 2;
    let cap_text = RECT { left: cap_full.left, top: cap_full.top, right: cap_full.right, bottom: mid - 2 };
    let cap_img = RECT { left: cap_full.left, top: mid + 2, right: cap_full.right, bottom: cap_full.bottom };

    // 解説ブロック: 左30%リスト / 右70%詳細 (全幅)
    let exp_list_w = ((w - PAD * 2) as f32 * 0.30) as i32;
    let exp_list = RECT { left: PAD, top: exp_top, right: PAD + exp_list_w, bottom: exp_bottom };
    let exp_text = RECT { left: PAD + exp_list_w + 4, top: exp_top, right: w - PAD, bottom: exp_bottom };

    Geo {
        cap_list, cap_text, cap_img, recog_list, recog_text, trans_list, trans_text,
        exp_list, exp_text, label_y, btn_y, exp_label_y, col_x, col_w,
    }
}

/// ウィンドウサイズに合わせて子コントロールを配置
fn layout(h: HWND) {
    unsafe {
        let g = geometry(h);
        let mv = |id: i32, x: i32, y: i32, cw: i32, ch: i32| {
            let _ = SetWindowPos(
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
        let r = |rc: &RECT| (rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top);

        // 検索行 (全幅)
        let mut x = PAD;
        mv(IDC_SEARCH_EDIT, x, PAD, 240, BTN_H);
        x += 240 + gap;
        mv(IDC_EXE_COMBO, x, PAD, 160, 200);
        x += 160 + gap;
        mv(IDC_BTN_EXPORT, x, PAD, 90, BTN_H);
        let mut rc = RECT::default();
        let _ = GetClientRect(h, &mut rc);
        mv(IDC_BTN_REFRESH, rc.right - PAD - 90 - gap - 100, PAD, 90, BTN_H);
        mv(IDC_BTN_CLEAR, rc.right - PAD - 100, PAD, 100, BTN_H);

        // 見出しラベル
        mv(IDC_LBL_CAP, g.col_x[0], g.label_y, g.col_w, LBL_H);
        mv(IDC_LBL_RECOG, g.col_x[1], g.label_y, g.col_w, LBL_H);
        mv(IDC_LBL_TRANS, g.col_x[2], g.label_y, g.col_w, LBL_H);
        mv(IDC_LBL_EXP, PAD, g.exp_label_y, 300, LBL_H);

        // 上段3ブロック
        let (x0, y0, w0, h0) = r(&g.cap_list);
        mv(IDC_CAP_LV, x0, y0, w0, h0);
        let (x1, y1, w1, h1) = r(&g.cap_text);
        mv(IDC_CAP_DETAIL, x1, y1, w1, h1);
        let (x2, y2, w2, h2) = r(&g.recog_list);
        mv(IDC_RECOG_LV, x2, y2, w2, h2);
        let (x3, y3, w3, h3) = r(&g.recog_text);
        mv(IDC_RECOG_DETAIL, x3, y3, w3, h3);
        let (x4, y4, w4, h4) = r(&g.trans_list);
        mv(IDC_TRANS_LV, x4, y4, w4, h4);
        let (x5, y5, w5, h5) = r(&g.trans_text);
        mv(IDC_TRANS_DETAIL, x5, y5, w5, h5);

        // 各列のボタン行
        // 列1: 選択削除
        mv(IDC_BTN_DEL_CAP, g.col_x[0], g.btn_y, 90, BTN_H);
        // 列2: 再OCRコンボ+実行+削除+タグ
        let mut x = g.col_x[1];
        mv(IDC_OCR_COMBO, x, g.btn_y, 105, 200);
        x += 105 + 4;
        mv(IDC_BTN_REOCR, x, g.btn_y, 60, BTN_H);
        x += 60 + gap;
        mv(IDC_BTN_DEL_RECOG, x, g.btn_y, 50, BTN_H);
        x += 50 + gap;
        let tag_w = (g.col_x[1] + g.col_w - x - 70 - 4).max(60);
        mv(IDC_TAG_EDIT, x, g.btn_y, tag_w, BTN_H);
        mv(IDC_BTN_SAVE_TAG, x + tag_w + 4, g.btn_y, 70, BTN_H);
        // 列3: 再翻訳コンボ+実行+削除
        let mut x = g.col_x[2];
        mv(IDC_TR_COMBO, x, g.btn_y, 105, 200);
        x += 105 + 4;
        mv(IDC_BTN_RETRANS, x, g.btn_y, 60, BTN_H);
        x += 60 + gap;
        mv(IDC_BTN_DEL_TRANS, x, g.btn_y, 50, BTN_H);

        // 下段: 解説ブロック
        let (x6, y6, w6, h6) = r(&g.exp_list);
        mv(IDC_EXP_LV, x6, y6, w6, h6);
        let (x7, y7, w7, h7) = r(&g.exp_text);
        mv(IDC_EXP_DETAIL, x7, y7, w7, h7);
    }
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
            state: LIST_VIEW_ITEM_STATE_FLAGS(0x0003), // LVIS_SELECTED | LVIS_FOCUSED
            stateMask: LIST_VIEW_ITEM_STATE_FLAGS(0x0003),
            ..Default::default()
        };
        SendMessageW(lvh, LVM_SETITEMW, Some(WPARAM(0)), Some(LPARAM(&mut item as *mut _ as isize)));
        SendMessageW(lvh, LVM_ENSUREVISIBLE, Some(WPARAM(idx as usize)), Some(LPARAM(0)));
    }
}

fn lv_count(lvh: HWND) -> i32 {
    unsafe { SendMessageW(lvh, LVM_GETITEMCOUNT, Some(WPARAM(0)), Some(LPARAM(0))).0 as i32 }
}

fn truncate(s: &str, n: usize) -> String {
    let one_line: String = s.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
    if one_line.chars().count() > n {
        one_line.chars().take(n).collect::<String>() + "…"
    } else {
        one_line
    }
}

fn set_edit(id: i32, text: &str) {
    unsafe {
        // EDIT は \n だけだと改行されないため \r\n に正規化
        let normalized = text.replace("\r\n", "\n").replace('\n', "\r\n");
        let wide = to_wide(&normalized);
        let _ = SetWindowTextW(dlg_item(hwnd(), id), PCWSTR(wide.as_ptr()));
    }
}

fn pretty_json(s: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(s) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| s.to_string()),
        Err(_) => s.to_string(),
    }
}

// ---- データ読み込みとブロック間連動 (§9.2) ----

/// DBから再読込して入力(captures)一覧を更新 (検索欄・exeフィルタを適用)
fn reload() {
    let h = hwnd();
    let query = edit_text(h, IDC_SEARCH_EDIT);
    let exe_idx = combo_sel(h, IDC_EXE_COMBO);
    // index 0 は「全アプリ」
    let app_exe = if exe_idx == 0 { String::new() } else { combo_item_text(h, IDC_EXE_COMBO, exe_idx) };

    let caps = logdb::search_captures(&query, &app_exe, 1000);
    let cap_lv = dlg_item(h, IDC_CAP_LV);
    lv_clear(cap_lv);
    for (i, c) in caps.iter().enumerate() {
        let img = if c.image_path.is_some() { "✓" } else { "" };
        lv_add_row(cap_lv, i as i32, &[
            fmt_ts(c.ts_ms),
            c.mode.clone(),
            truncate(c.app_exe.as_deref().unwrap_or(""), 16),
            img.to_string(),
        ]);
    }
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.caps = caps;
        st.recogs.clear();
        st.trans.clear();
        st.exps.clear();
        st.sel_cap = None;
        st.sel_recog = None;
        st.sel_trans = None;
        st.sel_exp = None;
        st.image = None;
    });
    lv_clear(dlg_item(h, IDC_RECOG_LV));
    lv_clear(dlg_item(h, IDC_TRANS_LV));
    lv_clear(dlg_item(h, IDC_EXP_LV));
    set_edit(IDC_CAP_DETAIL, "");
    set_edit(IDC_RECOG_DETAIL, "");
    set_edit(IDC_TRANS_DETAIL, "");
    set_edit(IDC_EXP_DETAIL, "");
    unsafe {
        let _ = InvalidateRect(Some(h), None, true);
    }
}

/// 入力選択時: 詳細・画像を更新し、読み取り結果一覧を連動更新
fn on_cap_selected(idx: usize) {
    let h = hwnd();
    let cap = STATE.with(|s| s.borrow().caps.get(idx).cloned());
    let Some(cap) = cap else { return };

    // 入力詳細
    let mut d = format!("日時: {}\nモード: {}\n", fmt_ts(cap.ts_ms), cap.mode);
    if let Some(exe) = &cap.app_exe {
        d.push_str(&format!("実行ファイル: {exe}\n"));
    }
    if let Some(t) = &cap.app_title {
        d.push_str(&format!("タイトル: {t}\n"));
    }
    if let (Some(w), Some(hh)) = (cap.image_w, cap.image_h) {
        d.push_str(&format!("画像: {w}x{hh}\n"));
    }
    if let Some(p) = &cap.uia_path
        && !p.is_empty() {
            d.push_str(&format!("\n【UIAパス】\n{p}\n"));
        }
    set_edit(IDC_CAP_DETAIL, &d);

    // 画像デコード
    let image = cap.image_path.as_ref().and_then(|rel| decode_png(&logdb::logs_dir().join(rel)));

    // 読み取り結果一覧
    let recogs = logdb::recognitions_for(cap.id);
    let recog_lv = dlg_item(h, IDC_RECOG_LV);
    lv_clear(recog_lv);
    for (i, r) in recogs.iter().enumerate() {
        let text = if r.success { r.source_text.clone() } else { format!("[エラー] {}", r.error) };
        lv_add_row(recog_lv, i as i32, &[
            fmt_ts(r.ts_ms),
            r.engine.clone(),
            r.duration_ms.to_string(),
            truncate(&text, 60),
        ]);
    }
    let has_recog = !recogs.is_empty();
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.sel_cap = Some(idx);
        st.recogs = recogs;
        st.trans.clear();
        st.exps.clear();
        st.sel_recog = None;
        st.sel_trans = None;
        st.sel_exp = None;
        st.image = image;
    });
    lv_clear(dlg_item(h, IDC_TRANS_LV));
    lv_clear(dlg_item(h, IDC_EXP_LV));
    set_edit(IDC_RECOG_DETAIL, "");
    set_edit(IDC_TRANS_DETAIL, "");
    set_edit(IDC_EXP_DETAIL, "");
    if has_recog {
        lv_select(recog_lv, 0);
        on_recog_selected(0);
    }
    unsafe {
        let _ = InvalidateRect(Some(h), None, true);
    }
}

/// 読み取り結果選択時: 詳細を更新し、翻訳結果・解説結果一覧を連動更新
fn on_recog_selected(idx: usize) {
    let h = hwnd();
    let recog = STATE.with(|s| s.borrow().recogs.get(idx).cloned());
    let Some(recog) = recog else { return };

    // 読み取り詳細
    let mut d = format!(
        "日時: {}\n方式: {} / エンジン: {} / {}ms\n",
        fmt_ts(recog.ts_ms), recog.method, recog.engine, recog.duration_ms
    );
    if !recog.tags.is_empty() {
        d.push_str(&format!("タグ: {}\n", recog.tags));
    }
    if recog.success {
        d.push_str(&format!("\n【認識テキスト】\n{}", recog.source_text));
    } else {
        d.push_str(&format!("\n【エラー】\n{}", recog.error));
    }
    set_edit(IDC_RECOG_DETAIL, &d);

    // タグ入力欄
    unsafe {
        let wide = to_wide(&recog.tags);
        let _ = SetWindowTextW(dlg_item(h, IDC_TAG_EDIT), PCWSTR(wide.as_ptr()));
    }

    // 翻訳結果一覧
    let trans = logdb::translations_for(recog.id);
    let trans_lv = dlg_item(h, IDC_TRANS_LV);
    lv_clear(trans_lv);
    for (i, t) in trans.iter().enumerate() {
        let dir = format!("{}→{}", t.source_lang, t.target_lang);
        let text = if t.success { t.translated_text.clone() } else { format!("[エラー] {}", t.error) };
        lv_add_row(trans_lv, i as i32, &[
            fmt_ts(t.ts_ms),
            t.engine.clone(),
            dir,
            truncate(&text, 60),
        ]);
    }

    // 解説結果一覧
    let exps = logdb::explanations_for(recog.id);
    let exp_lv = dlg_item(h, IDC_EXP_LV);
    lv_clear(exp_lv);
    for (i, e) in exps.iter().enumerate() {
        let tok = match (e.tokens_in, e.tokens_out) {
            (Some(a), Some(b)) => format!("{a}/{b}"),
            _ => String::new(),
        };
        let text = if e.success { e.explanation_text.clone() } else { format!("[エラー] {}", e.error) };
        lv_add_row(exp_lv, i as i32, &[
            fmt_ts(e.ts_ms),
            e.llm_profile.clone(),
            e.duration_ms.to_string(),
            tok,
            truncate(&text, 80),
        ]);
    }

    let has_trans = !trans.is_empty();
    let has_exp = !exps.is_empty();
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.sel_recog = Some(idx);
        st.trans = trans;
        st.exps = exps;
        st.sel_trans = None;
        st.sel_exp = None;
    });
    set_edit(IDC_TRANS_DETAIL, "");
    set_edit(IDC_EXP_DETAIL, "");
    if has_trans {
        lv_select(trans_lv, 0);
        on_trans_selected(0);
    }
    if has_exp {
        let last = lv_count(exp_lv) - 1;
        lv_select(exp_lv, last);
        on_exp_selected(last as usize);
    }
}

/// 翻訳結果選択時: 詳細(訳文+JSON)を表示
fn on_trans_selected(idx: usize) {
    let t = STATE.with(|s| s.borrow().trans.get(idx).cloned());
    let Some(t) = t else { return };
    STATE.with(|s| s.borrow_mut().sel_trans = Some(idx));

    let mut d = format!(
        "日時: {}\nエンジン: {}",
        fmt_ts(t.ts_ms), t.engine
    );
    if let Some(p) = &t.llm_profile {
        d.push_str(&format!(" (プロファイル: {p})"));
    }
    d.push_str(&format!(
        "\n方向: {}→{} / {}ms{}",
        t.source_lang, t.target_lang, t.duration_ms,
        if t.cache_hit { " / キャッシュ" } else { "" }
    ));
    if let (Some(a), Some(b)) = (t.tokens_in, t.tokens_out) {
        d.push_str(&format!("\nトークン: 入力{a} / 出力{b}"));
    }
    if t.success {
        d.push_str(&format!("\n\n【訳文】\n{}", t.translated_text));
    } else {
        d.push_str(&format!("\n\n【エラー】\n{}", t.error));
    }
    if !t.request_json.is_empty() {
        d.push_str(&format!("\n\n【送信JSON】\n{}", pretty_json(&t.request_json)));
    }
    if !t.response_json.is_empty() {
        d.push_str(&format!("\n\n【受信JSON】\n{}", pretty_json(&t.response_json)));
    }
    set_edit(IDC_TRANS_DETAIL, &d);
}

/// 解説結果選択時: 詳細(解説文+送信プロンプト)を表示
fn on_exp_selected(idx: usize) {
    let e = STATE.with(|s| s.borrow().exps.get(idx).cloned());
    let Some(e) = e else { return };
    STATE.with(|s| s.borrow_mut().sel_exp = Some(idx));

    let mut d = format!(
        "日時: {}\nプロファイル: {} / {}ms",
        fmt_ts(e.ts_ms), e.llm_profile, e.duration_ms
    );
    if let (Some(a), Some(b)) = (e.tokens_in, e.tokens_out) {
        d.push_str(&format!("\nトークン: 入力{a} / 出力{b}"));
    }
    if e.success {
        d.push_str(&format!("\n\n【解説】\n{}", e.explanation_text));
    } else {
        d.push_str(&format!("\n\n【エラー】\n{}", e.error));
    }
    d.push_str(&format!("\n\n【送信プロンプト】\n{}", e.input_text));
    set_edit(IDC_EXP_DETAIL, &d);
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

/// 入力ブロック右下にキャプチャ画像を縮小描画する
fn paint_image(h: HWND) {
    // geometry() は STATE を借用しないが、借用順を固定するため先に取得する
    let g = geometry(h);
    STATE.with(|s| {
        let st = s.borrow();
        let Some((iw, ih, rgba)) = st.image.as_ref() else { return };
        unsafe {
            let img_left = g.cap_img.left;
            let img_top = g.cap_img.top;
            let img_w = (g.cap_img.right - g.cap_img.left).max(1);
            let img_h = (g.cap_img.bottom - g.cap_img.top).max(20);

            // アスペクト比維持で img_w×img_h に収める
            let scale = (img_w as f32 / *iw as f32).min(img_h as f32 / *ih as f32).min(1.0);
            let dw = (*iw as f32 * scale) as i32;
            let dh = (*ih as f32 * scale) as i32;

            let hdc = windows::Win32::Graphics::Gdi::GetDC(Some(h));
            // 背景を塗る
            let bg = windows::Win32::Graphics::Gdi::CreateSolidBrush(COLORREF(0x00202020));
            let area = RECT { left: img_left, top: img_top, right: img_left + img_w, bottom: img_top + img_h };
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
                img_top,
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

fn in_rect(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
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

/// 選択した入力の画像を、指定エンジンで再OCRして同じ capture に認識行を追記する(ワーカースレッド)。
fn start_reocr(h: HWND) {
    let sel = STATE.with(|s| {
        let st = s.borrow();
        st.sel_cap.and_then(|i| st.caps.get(i)).map(|c| (c.id, c.image_path.clone(), prompt_ctx_from_cap(c)))
    });
    let Some((capture_id, image_path, mut pc)) = sel else {
        unsafe { MessageBoxW(Some(h), w!("入力を選択してください。"), w!("再OCR"), MB_OK); }
        return;
    };
    let Some(rel) = image_path else {
        unsafe { MessageBoxW(Some(h), w!("この入力には画像がありません(デバッグモードで記録した画像のみ再OCRできます)。"), w!("再OCR"), MB_OK); }
        return;
    };
    let engine = OCR_ENGINES[combo_sel(h, IDC_OCR_COMBO).min(OCR_ENGINES.len() - 1)].0.to_string();
    let hwnd_isize = h.0 as isize;
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
            let hash = crate::capture::hash_hex(&cap);
            // ログビューアの「再OCR」は明示的な手動再実行ボタンのため、キャッシュがあっても
            // 常に実行する(オーバーレイ側の自動再認識とは異なり、都度の確認が目的のため)。
            // ハッシュ自体は後日オーバーレイ側の重複防止に使えるよう記録しておく。
            let t0 = std::time::Instant::now();
            pc.ocr_engine = engine.clone();
            let (text, err): (Option<String>, Option<String>) =
                match crate::ocr::run(&engine, &cfg, &cap, crate::ocr::Focus::All, &pc) {
                    Ok(o) => (Some(o.text), None),
                    Err(e) => (None, Some(e)),
                };
            let ms = t0.elapsed().as_millis();
            // 再OCR結果を同じ capture の認識行として追記 (SPECv0.4 §8.2.1)
            logdb::log_recognition(capture_id, "ocr", &engine, ms, text.as_deref(), err.as_deref(), Some(&hash));
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

/// capture 行からプロンプト置換用コンテキストを組み立てる (SPECv0.4 §7.1)
fn prompt_ctx_from_cap(c: &CaptureRow) -> crate::config::PromptContext {
    crate::config::PromptContext {
        app_title: c.app_title.clone().unwrap_or_default(),
        app_exe: c.app_exe.clone().unwrap_or_default(),
        uia_path: c.uia_path.clone().unwrap_or_default(),
        ..Default::default()
    }
}

/// 選択した読み取り結果の原文を、指定エンジンで再翻訳して追記する(ワーカースレッド)。
fn start_retranslate(h: HWND) {
    let sel = STATE.with(|s| {
        let st = s.borrow();
        let pc = st.sel_cap.and_then(|i| st.caps.get(i)).map(prompt_ctx_from_cap).unwrap_or_default();
        st.sel_recog.and_then(|i| st.recogs.get(i)).map(|r| (r.id, r.source_text.clone(), r.engine.clone(), pc))
    });
    let Some((recog_id, source, recog_engine, mut pc)) = sel else {
        unsafe { MessageBoxW(Some(h), w!("読み取り結果を選択してください。"), w!("再翻訳"), MB_OK); }
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
        // UIA経路の認識は ocr_engine を空にする (SPECv0.4 §7.1)
        if recog_engine != "uia" {
            pc.ocr_engine = recog_engine.clone();
        }
        match crate::translate::translate(&engine, &cfg, &source, &pc) {
            Ok(t) => {
                let ms = t0.elapsed().as_millis();
                let profile = (t.engine == "llm").then(|| cfg.active_api_profile.clone());
                logdb::log_translation(
                    recog_id, &t.engine, profile.as_deref(), &t.source_lang, &t.target_lang, ms,
                    t.cache_hit, Some(&t.text), None, t.detail.request_json.as_deref(),
                    t.detail.response_json.as_deref(), t.detail.tokens_in, t.detail.tokens_out,
                );
            }
            Err(e) => {
                let ms = t0.elapsed().as_millis();
                let profile = (engine == "llm").then(|| cfg.active_api_profile.clone());
                logdb::log_translation(
                    recog_id, &engine, profile.as_deref(), &cfg.source_lang, &cfg.target_lang, ms,
                    false, None, Some(&e), None, None, None, None,
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

/// リロード後、以前選択していた入力行を復元する
fn restore_cap_selection(h: HWND, old_idx: Option<usize>) {
    if let Some(old_idx) = old_idx {
        let cap_lv = dlg_item(h, IDC_CAP_LV);
        let count = lv_count(cap_lv);
        if count > 0 {
            let new_idx = if (old_idx as i32) < count { old_idx } else { (count - 1) as usize };
            lv_select(cap_lv, new_idx as i32);
            on_cap_selected(new_idx);
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
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let g = geometry(h);
            if in_rect(&g.cap_img, x, y) {
                // 画像領域クリック → 原寸(1:1)表示ウィンドウ
                open_image_1to1(h);
            }
            LRESULT(0)
        }
        WM_APP_RELOAD => {
            // 再OCR/再翻訳後のリロード: 前の選択アイテムを復元する
            let sel_before = STATE.with(|s| s.borrow().sel_cap);
            reload();
            restore_cap_selection(h, sel_before);
            LRESULT(0)
        }
        WM_NOTIFY => {
            let nmhdr = unsafe { &*(lparam.0 as *const NMHDR) };
            if nmhdr.code == LVN_ITEMCHANGED {
                let id = nmhdr.idFrom as i32;
                match id {
                    IDC_CAP_LV => {
                        if let Some(sel) = lv_selected(dlg_item(h, IDC_CAP_LV)) {
                            let cur = STATE.with(|s| s.borrow().sel_cap);
                            if cur != Some(sel) {
                                on_cap_selected(sel);
                            }
                        }
                    }
                    IDC_RECOG_LV => {
                        if let Some(sel) = lv_selected(dlg_item(h, IDC_RECOG_LV)) {
                            let cur = STATE.with(|s| s.borrow().sel_recog);
                            if cur != Some(sel) {
                                on_recog_selected(sel);
                            }
                        }
                    }
                    IDC_TRANS_LV => {
                        if let Some(sel) = lv_selected(dlg_item(h, IDC_TRANS_LV)) {
                            on_trans_selected(sel);
                        }
                    }
                    IDC_EXP_LV => {
                        if let Some(sel) = lv_selected(dlg_item(h, IDC_EXP_LV)) {
                            on_exp_selected(sel);
                        }
                    }
                    _ => {}
                }
            } else if nmhdr.code == LVN_KEYDOWN {
                let key_nm = unsafe { &*(lparam.0 as *const NMLVKEYDOWN) };
                if key_nm.wVKey == VK_DELETE.0 {
                    let del_id = match nmhdr.idFrom as i32 {
                        IDC_CAP_LV => Some(IDC_BTN_DEL_CAP),
                        IDC_RECOG_LV => Some(IDC_BTN_DEL_RECOG),
                        IDC_TRANS_LV => Some(IDC_BTN_DEL_TRANS),
                        _ => None,
                    };
                    if let Some(did) = del_id {
                        unsafe { let _ = SendMessageW(h, WM_COMMAND, Some(WPARAM(did as usize)), None); }
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
                IDC_BTN_DEL_CAP => {
                    let (sel_idx, cid) = STATE.with(|s| {
                        let st = s.borrow();
                        (st.sel_cap, st.sel_cap.and_then(|i| st.caps.get(i)).map(|c| c.id))
                    });
                    if let Some(cid) = cid {
                        logdb::delete_capture(cid);
                        reload();
                        restore_cap_selection(h, sel_idx);
                    }
                }
                IDC_BTN_DEL_RECOG => {
                    let (cap_idx, rid) = STATE.with(|s| {
                        let st = s.borrow();
                        (st.sel_cap, st.sel_recog.and_then(|i| st.recogs.get(i)).map(|r| r.id))
                    });
                    if let Some(rid) = rid {
                        logdb::delete_recognition(rid);
                        if let Some(ci) = cap_idx {
                            on_cap_selected(ci);
                        }
                    }
                }
                IDC_BTN_DEL_TRANS => {
                    let (recog_idx, tid) = STATE.with(|s| {
                        let st = s.borrow();
                        (st.sel_recog, st.sel_trans.and_then(|i| st.trans.get(i)).map(|t| t.id))
                    });
                    if let Some(tid) = tid {
                        logdb::delete_translation(tid);
                        if let Some(ri) = recog_idx {
                            on_recog_selected(ri);
                        }
                    }
                }
                IDC_BTN_SAVE_TAG => {
                    let recog_id = STATE.with(|s| {
                        let st = s.borrow();
                        st.sel_recog.and_then(|idx| st.recogs.get(idx).map(|r| r.id))
                    });
                    if let Some(rid) = recog_id {
                        let tags = edit_text(h, IDC_TAG_EDIT);
                        logdb::set_tags(rid, &tags);
                        // STATE側にも反映 (再選択なしで詳細を一致させる)
                        STATE.with(|s| {
                            let mut st = s.borrow_mut();
                            if let Some(i) = st.sel_recog
                                && let Some(r) = st.recogs.get_mut(i) {
                                    r.tags = tags.clone();
                                }
                        });
                        unsafe { MessageBoxW(Some(h), w!("タグを保存しました。"), w!("タグ保存"), MB_OK); }
                    }
                }
                IDC_BTN_EXPORT => {
                    let path = logdb::logs_dir().join("export.csv");
                    if let Ok(mut f) = std::fs::File::create(&path) {
                        use std::io::Write;
                        let _ = writeln!(f, "\u{FEFF}入力ID,日時,モード,アプリ,認識エンジン,原文,翻訳エンジン,訳文,タグ,解説");
                        let caps = STATE.with(|s| s.borrow().caps.clone());
                        let escape = |s: &str| format!("\"{}\"", s.replace("\"", "\"\""));
                        for c in caps {
                            for r in logdb::recognitions_for(c.id) {
                                let trans = logdb::translations_for(r.id);
                                let tr = trans.iter().rev().find(|t| t.success);
                                let exp = logdb::latest_explanation(r.id).unwrap_or_default();
                                let _ = writeln!(
                                    f,
                                    "{},{},{},{},{},{},{},{},{},{}",
                                    c.id,
                                    fmt_ts(r.ts_ms),
                                    c.mode,
                                    escape(c.app_exe.as_deref().unwrap_or("")),
                                    r.engine,
                                    escape(&r.source_text),
                                    tr.map(|t| t.engine.clone()).unwrap_or_default(),
                                    escape(&tr.map(|t| t.translated_text.clone()).unwrap_or_default()),
                                    escape(&r.tags),
                                    escape(&exp),
                                );
                            }
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
