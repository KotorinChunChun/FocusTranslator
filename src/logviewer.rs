// ログビューア (SPECv0.4 §9: 4工程ツリー構造のブロック表示)
// 上段3ブロック: 【入力内容】captures →【読み取り結果】recognitions →【翻訳結果】translations
// 下段1ブロック: 【解説結果】explanations
// 各ブロックは「左: リストビュー / 右: 詳細テキスト(入力ブロックは画像も)」の構成。
// 検索行(部分一致・exeフィルタ・全削除等)は上部にウィンドウ全幅で配置する。
use crate::logdb::{self, CaptureRow, ExplainRow, RecogRow, TransRow};
use crate::ui_helpers::*;
use crate::util::to_wide;
use std::cell::RefCell;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, COLOR_BTNFACE, DIB_RGB_COLORS, HALFTONE, HBRUSH,
    HDC, InvalidateRect, SetStretchBltMode, StretchDIBits,
};
use windows::Win32::UI::Controls::{
    INITCOMMONCONTROLSEX, InitCommonControlsEx, LVCF_SUBITEM, LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW,
    LVIF_STATE, LVIF_TEXT, LVITEMW, LIST_VIEW_ITEM_STATE_FLAGS, LVM_DELETEALLITEMS,
    LVM_GETITEMCOUNT, LVM_GETNEXTITEM, LVM_INSERTCOLUMNW, LVM_INSERTITEMW,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMTEXTW, LVM_SETITEMW, LVM_ENSUREVISIBLE,
    LVN_ITEMCHANGED, LVN_KEYDOWN, NMLVKEYDOWN, LVS_EX_FULLROWSELECT, LVS_REPORT,
    LVS_SHOWSELALWAYS, LVS_SINGLESEL, NMHDR,
};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_DELETE,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    CBS_DROPDOWNLIST, CW_USEDEFAULT, CallWindowProcW,
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_WNDPROC, GetClientRect, GetCursorPos,
    HMENU, IDC_ARROW, IDC_SIZENS, IDC_SIZEWE,
    IsWindow, LoadCursorW, MB_ICONQUESTION, MB_OK, MB_YESNO, MessageBoxW,
    SW_SHOW, SW_SHOWNORMAL, SendMessageW, SetCursor, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowPos, SetWindowTextW, ShowWindow, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND,
    WM_DESTROY, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NOTIFY, WM_SETCURSOR,
    WM_SIZE, WNDCLASSW, WS_BORDER, WS_CHILD, WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW, WS_TABSTOP,
    WS_VISIBLE, WS_VSCROLL,
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
// テキスト追加(ログビューア拡張 §1): 常時表示の3行テキストボックス+追加+クリア
const IDC_ADD_TEXT_EDIT: i32 = 237;
const IDC_BTN_ADD_SAVE: i32 = 238; // "追加"
const IDC_BTN_ADD_CANCEL: i32 = 239; // "クリア"
// 解説結果ブロックのモデル選択・再解説・削除(ログビューア拡張 §2)
const IDC_EXP_COMBO: i32 = 244;
const IDC_BTN_REEXPLAIN: i32 = 245;
const IDC_BTN_DEL_EXP: i32 = 246;
// 全文検索ラベル・グループ枠(v0.4.4 バグ修正)
const IDC_LBL_SEARCH: i32 = 247;
const IDC_GRP_FILTER: i32 = 248;
const IDC_GRP_ADD: i32 = 249;
// 翻訳結果・解説結果の3列表示(リスト/入力プロンプト/結果テキスト)用プロンプト欄 (v0.4.8)
const IDC_TRANS_PROMPT: i32 = 250;
const IDC_EXP_PROMPT: i32 = 251;
/// 手動追加テキストの取得元アプリ名として一律で記録する値
const MANUAL_APP_NAME: &str = "FocusTranslator";

/// 再OCR/再翻訳のワーカースレッド完了通知(ビューア限定メッセージ)
const WM_APP_RELOAD: u32 = WM_APP + 30;

/// 再OCRエンジン(内部キー / 表示名)
const OCR_ENGINES: [(&str, &str); 4] = [
    ("oneocr", "OneOCR"),
    ("win", "Windows.Media.Ocr.dll"),
    ("paddle", "PaddleOCR"),
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
    /// 現在表示中のOCR対象画像のデコード済みRGBA (幅, 高さ, ピクセル)
    image: Option<(u32, u32, Vec<u8>)>,
    /// 現在表示中の対象アプリ全体画像 (SPECv0.5.2追補。無いレコードは None)
    full_image: Option<(u32, u32, Vec<u8>)>,
    /// full_image 内での image の位置 (x, y, w, h / 物理ピクセル座標)
    crop_rect: Option<(i32, i32, i32, i32)>,
}

/// 再OCR/再翻訳完了後、WM_APP_RELOAD で新規追加されたアイテムへフォーカスするための指示
enum ReloadFocus {
    None,
    /// 再OCR: 選択中captureの認識一覧の最新(末尾)行を選択する
    NewestRecog,
    /// 再翻訳: 指定した認識行を選択したうえで、その翻訳一覧の最新(末尾)行を選択する
    NewestTrans(i64),
}

thread_local! {
    static WND: RefCell<isize> = const { RefCell::new(0) };
    static REGISTERED: RefCell<bool> = const { RefCell::new(false) };
    static STATE: RefCell<State> = const { RefCell::new(State {
        caps: Vec::new(), recogs: Vec::new(), trans: Vec::new(), exps: Vec::new(),
        sel_cap: None, sel_recog: None, sel_trans: None, sel_exp: None,
        image: None, full_image: None, crop_rect: None,
    }) };
    static RELOAD_FOCUS: RefCell<ReloadFocus> = const { RefCell::new(ReloadFocus::None) };
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
        let title_w = crate::util::to_wide(&format!("{} ログビューア", crate::util::APP_DISPLAY_NAME));
        if let Ok(h) = CreateWindowExW(
            WS_EX_TOPMOST,
            class,
            PCWSTR(title_w.as_ptr()),
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

/// 複数行・編集可能・折返しのエディットを作る (【入力追加】グループの常時表示テキストボックス用)。
/// ES_WANTRETURN を付けて Enter キーが改行として入力されるようにする
/// (親ウィンドウは IsDialogMessageW を通すため、無指定だと既定ボタン相当の扱いになる)。
fn multiline_edit_editable(parent: HWND, inst: HINSTANCE, id: i32) -> HWND {
    unsafe {
        const ES_MULTILINE: u32 = 0x0004;
        const ES_AUTOVSCROLL: u32 = 0x0040;
        const ES_WANTRETURN: u32 = 0x1000;
        let h = CreateWindowExW(
            Default::default(),
            w!("EDIT"),
            w!(""),
            WS_CHILD
                | WS_VISIBLE
                | WS_BORDER
                | WS_VSCROLL
                | WS_TABSTOP
                | WINDOW_STYLE(ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN),
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
        subclass_detail(h);
        h
    }
}

/// BS_GROUPBOX でカテゴリ枠を作る (settings.rs の group() と同様のパターン)
fn group(parent: HWND, inst: HINSTANCE, text: &str, id: i32) {
    const BS_GROUPBOX: u32 = 0x0000_0007;
    ctl(parent, inst, w!("BUTTON"), text, WINDOW_STYLE(BS_GROUPBOX), 0, 0, 0, 0, id);
}

fn label(parent: HWND, inst: HINSTANCE, text: &str, id: i32) {
    ctl(parent, inst, w!("STATIC"), text, Default::default(), 0, 0, 0, 0, id);
}

fn build(h: HWND, inst: HINSTANCE) {
    // 【絞り込み】グループ: 全文検索ラベル + 検索欄 + アプリ絞り込みコンボ
    group(h, inst, "絞り込み", IDC_GRP_FILTER);
    label(h, inst, "全文検索", IDC_LBL_SEARCH);
    ctl(h, inst, w!("EDIT"), "", WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP, 0, 0, 0, 0, IDC_SEARCH_EDIT);
    let exe_combo = combo(h, inst, IDC_EXE_COMBO);
    crate::ui_helpers::combo_add_item(exe_combo, "全アプリ");
    for exe in logdb::get_unique_app_exes() {
        crate::ui_helpers::combo_add_item(exe_combo, &exe);
    }
    crate::ui_helpers::combo_set_sel(exe_combo, 0);
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
    // 【入力追加】グループ(ログビューア拡張 §1): 常時表示の3行テキストボックス+追加+クリア
    group(h, inst, "入力追加", IDC_GRP_ADD);
    multiline_edit_editable(h, inst, IDC_ADD_TEXT_EDIT);
    btn(h, inst, "追加", IDC_BTN_ADD_SAVE);
    btn(h, inst, "クリア", IDC_BTN_ADD_CANCEL);

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
        crate::ui_helpers::combo_add_item(ocr_combo, disp);
    }
    crate::ui_helpers::combo_set_sel(ocr_combo, 0);
    btn(h, inst, "再OCR", IDC_BTN_REOCR);
    btn(h, inst, "削除", IDC_BTN_DEL_RECOG);
    ctl(h, inst, w!("EDIT"), "", WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP, 0, 0, 0, 0, IDC_TAG_EDIT);
    btn(h, inst, "タグ保存", IDC_BTN_SAVE_TAG);

    // 【翻訳結果】ブロック (3列: リスト/入力プロンプト/結果テキスト)
    label(h, inst, "【翻訳結果】", IDC_LBL_TRANS);
    let trans = lv(h, inst, IDC_TRANS_LV);
    add_col(trans, 0, "日時", 120);
    add_col(trans, 1, "エンジン", 60);
    add_col(trans, 2, "方向", 55);
    add_col(trans, 3, "プロファイル", 90);
    add_col(trans, 4, "tok入/出", 70);
    add_col(trans, 5, "訳文", 200);
    detail_edit(h, inst, IDC_TRANS_PROMPT);
    detail_edit(h, inst, IDC_TRANS_DETAIL);
    let tr_combo = combo(h, inst, IDC_TR_COMBO);
    for (_, disp) in TR_ENGINES {
        crate::ui_helpers::combo_add_item(tr_combo, disp);
    }
    crate::ui_helpers::combo_set_sel(tr_combo, 0);
    btn(h, inst, "再翻訳", IDC_BTN_RETRANS);
    btn(h, inst, "削除", IDC_BTN_DEL_TRANS);

    // 【解説結果】ブロック (3列: リスト/入力プロンプト/結果テキスト)
    label(h, inst, "【解説結果】", IDC_LBL_EXP);
    let exp = lv(h, inst, IDC_EXP_LV);
    add_col(exp, 0, "日時", 120);
    add_col(exp, 1, "プロファイル", 90);
    add_col(exp, 2, "ms", 50);
    add_col(exp, 3, "tok入/出", 70);
    add_col(exp, 4, "解説文", 320);
    detail_edit(h, inst, IDC_EXP_PROMPT);
    detail_edit(h, inst, IDC_EXP_DETAIL);
    // モデル選択・再解説・選択削除(ログビューア拡張 §2)
    let exp_combo = combo(h, inst, IDC_EXP_COMBO);
    let cfg = crate::config::Config::load();
    let mut active_idx = 0usize;
    for (i, p) in cfg.api_profiles.iter().enumerate() {
        crate::ui_helpers::combo_add_item(exp_combo, &p.name);
        if p.name == cfg.active_api_profile {
            active_idx = i;
        }
    }
    if !cfg.api_profiles.is_empty() {
        crate::ui_helpers::combo_set_sel(exp_combo, active_idx);
    }
    btn(h, inst, "再解説", IDC_BTN_REEXPLAIN);
    btn(h, inst, "選択削除", IDC_BTN_DEL_EXP);

    // フォント適用
    unsafe {
        let font = make_font(13, false);
        let _ = windows::Win32::UI::WindowsAndMessaging::EnumChildWindows(
            Some(h),
            Some(set_font_proc),
            LPARAM(font.0 as isize),
        );
    }
    layout(h);
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



fn combo(parent: HWND, inst: HINSTANCE, id: i32) -> HWND {
    ctl(parent, inst, w!("COMBOBOX"), "", WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32), 0, 0, 0, 0, id)
}



const PAD: i32 = 8;
const BTN_H: i32 = 28;
const LBL_H: i32 = 18;
/// グループ枠タイトル分のオフセット (settings.rs の GTOP と同じ考え方)
const GTOP: i32 = 22;
/// 【入力追加】グループの3行テキストボックスの高さ
const ADD_EDIT_H: i32 = 60;
/// 【入力追加】グループ全体の高さ (タイトル + テキストボックス + 余白)
const ADD_GRP_H: i32 = GTOP + ADD_EDIT_H + 8;
/// スプリッター(ドラッグ境界)のヒット判定太さ
const SPLIT_T: i32 = 4;

thread_local! {
    /// 田の字スプリッターの位置 (グリッド領域に対する比率 0.0-1.0): (縦線Xの比率, 横線Yの比率)。
    /// 既定は左右比2:3 (v0.4.8)。
    static SPLIT: RefCell<(f32, f32)> = const { RefCell::new((0.4, 0.5)) };
    /// ドラッグ中のスプリッター軸 (1=縦線/左右, 2=横線/上下)
    static SPLIT_DRAG: RefCell<Option<u8>> = const { RefCell::new(None) };
}

/// 検索行(絞り込みグループ)を除いた、田の字グリッド領域の外枠 (left, top, right, bottom)
fn grid_bounds(h: HWND) -> (i32, i32, i32, i32) {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(h, &mut rc);
    }
    let w = rc.right.max(700);
    let ht = rc.bottom.max(500);
    let filter_grp_h = GTOP + BTN_H + 8;
    let grid_top = PAD + filter_grp_h + PAD;
    (PAD, grid_top, w - PAD, ht - PAD)
}

/// 各領域の矩形(レイアウト・描画・ヒットテストで共有)
struct Geo {
    cap_list: RECT,
    cap_text: RECT,
    /// OCR対象画像の表示領域 (左半分)
    cap_img_ocr: RECT,
    /// 対象アプリ全体画像の表示領域 (右半分。SPECv0.5.2追補: 2画像を左右に並べる)
    cap_img_full: RECT,
    cap_del_btn: RECT,
    add_group: RECT,
    add_edit: RECT,
    add_btn_save: RECT,
    add_btn_clear: RECT,
    recog_list: RECT,
    recog_text: RECT,
    trans_list: RECT,
    trans_prompt: RECT,
    trans_text: RECT,
    exp_list: RECT,
    exp_prompt: RECT,
    exp_text: RECT,
    filter_group: RECT,
    /// 上段(入力内容/翻訳結果)・下段(読み取り結果/解説結果)の見出しY (v0.4.8: 配置入替)
    label_y1: i32,
    label_y2: i32,
    /// 翻訳結果のボタン行Y(上段) / 読み取り結果・解説結果のボタン行Y(下段)
    row1_btn_y: i32,
    row2_btn_y: i32,
    /// 2列それぞれの左端X・列幅
    col_x: [i32; 2],
    col_w: [i32; 2],
    /// スプリッターのヒットテスト用矩形(縦線・横線)
    split_v: RECT,
    split_h: RECT,
}

/// リスト/入力プロンプト/結果テキストの3列 (1:1:1、間に4pxの余白) を計算する
fn three_cols(left: i32, w: i32, top: i32, bottom: i32) -> (RECT, RECT, RECT) {
    let seg = (w - 8) / 3;
    let list = RECT { left, top, right: left + seg, bottom };
    let prompt = RECT { left: left + seg + 4, top, right: left + seg * 2 + 4, bottom };
    let text = RECT { left: left + seg * 2 + 8, top, right: left + w, bottom };
    (list, prompt, text)
}

fn geometry(h: HWND) -> Geo {
    let (grid_left, grid_top, grid_right, grid_bottom) = grid_bounds(h);

    let filter_grp_h = GTOP + BTN_H + 8;
    let filter_grp_w = 8 + 70 + 6 + 200 + 6 + 160 + 8;
    let filter_group = RECT { left: PAD, top: PAD, right: PAD + filter_grp_w, bottom: PAD + filter_grp_h };

    // 田の字スプリッター(ドラッグで移動可能)。既定は左右比2:3 (v0.4.8)。
    let (rx, ry) = SPLIT.with(|s| *s.borrow());
    let split_x = (grid_left + ((grid_right - grid_left) as f32 * rx) as i32)
        .clamp(grid_left + 200, grid_right - 200);
    let split_y = (grid_top + ((grid_bottom - grid_top) as f32 * ry) as i32)
        .clamp(grid_top + 150, grid_bottom - 150);
    let split_v = RECT { left: split_x - SPLIT_T, top: grid_top, right: split_x + SPLIT_T, bottom: grid_bottom };
    let split_h = RECT { left: grid_left, top: split_y - SPLIT_T, right: grid_right, bottom: split_y + SPLIT_T };

    let col_x = [grid_left, split_x + 4];
    let col_w = [split_x - 4 - grid_left, grid_right - (split_x + 4)];

    let row1_bottom = split_y - 4;
    let row2_top = split_y + 4;

    let label_y1 = grid_top;
    let label_y2 = row2_top;
    let content_top1 = label_y1 + LBL_H + 2;
    let content_top2 = label_y2 + LBL_H + 2;

    // 【翻訳結果】(上段右): リスト/入力プロンプト/結果テキスト 1:1:1 + ボタン行 (v0.4.8: 位置入替)
    let row1_btn_y = row1_bottom - BTN_H;
    let trans_content_bottom = row1_btn_y - 4;
    let (trans_list, trans_prompt, trans_text) =
        three_cols(col_x[1], col_w[1], content_top1, trans_content_bottom);

    // 【読み取り結果】(下段左): リスト/詳細 50/50 + ボタン行 (v0.4.8: 位置入替)
    let row2_btn_y = grid_bottom - BTN_H;
    let row2_content_bottom = row2_btn_y - 4;
    let recog_list_w = (col_w[0] as f32 * 0.5) as i32;
    let recog_list = RECT { left: col_x[0], top: content_top2, right: col_x[0] + recog_list_w, bottom: row2_content_bottom };
    let recog_text = RECT { left: col_x[0] + recog_list_w + 4, top: content_top2, right: col_x[0] + col_w[0], bottom: row2_content_bottom };

    // 【解説結果】(下段右): リスト/入力プロンプト/結果テキスト 1:1:1 + ボタン行
    let (exp_list, exp_prompt, exp_text) =
        three_cols(col_x[1], col_w[1], content_top2, row2_content_bottom);

    // 【入力内容】(上段左): リスト/詳細+画像 → 選択削除ボタン行 → 【入力追加】グループ
    let cap_area_bottom = row1_bottom - (BTN_H + 4) - (ADD_GRP_H + 4);
    let cap_list_w = (col_w[0] as f32 * 0.5) as i32;
    let cap_list = RECT { left: col_x[0], top: content_top1, right: col_x[0] + cap_list_w, bottom: cap_area_bottom };
    let cap_full = RECT { left: col_x[0] + cap_list_w + 4, top: content_top1, right: col_x[0] + col_w[0], bottom: cap_area_bottom };
    // テキスト:画像 = 3:1 (§6)
    let cap_full_h = (cap_full.bottom - cap_full.top).max(40);
    let cap_text_h = cap_full_h * 3 / 4;
    let cap_text = RECT { left: cap_full.left, top: cap_full.top, right: cap_full.right, bottom: cap_full.top + cap_text_h - 2 };
    let cap_img = RECT { left: cap_full.left, top: cap_full.top + cap_text_h + 2, right: cap_full.right, bottom: cap_full.bottom };
    // OCR対象画像(左) / 対象アプリ全体画像(右) を横に並べる (SPECv0.5.2追補)
    let img_gap = 6;
    let img_half_w = ((cap_img.right - cap_img.left - img_gap) / 2).max(1);
    let cap_img_ocr = RECT { left: cap_img.left, top: cap_img.top, right: cap_img.left + img_half_w, bottom: cap_img.bottom };
    let cap_img_full = RECT { left: cap_img.left + img_half_w + img_gap, top: cap_img.top, right: cap_img.right, bottom: cap_img.bottom };

    let cap_del_btn_y = cap_area_bottom + 4;
    let cap_del_btn = RECT { left: col_x[0], top: cap_del_btn_y, right: col_x[0] + 90, bottom: cap_del_btn_y + BTN_H };

    let add_group_y = cap_del_btn_y + BTN_H + 4;
    let add_group = RECT { left: col_x[0], top: add_group_y, right: col_x[0] + col_w[0], bottom: add_group_y + ADD_GRP_H };
    let inner_top = add_group.top + GTOP;
    let inner_bottom = add_group.bottom - 8;
    let inner_left = add_group.left + 8;
    let inner_right = add_group.right - 8;
    let add_btn_w = 80;
    let add_edit = RECT { left: inner_left, top: inner_top, right: inner_right - add_btn_w - 6, bottom: inner_bottom };
    let mid_btn = inner_top + (inner_bottom - inner_top - 4) / 2;
    let add_btn_save = RECT { left: inner_right - add_btn_w, top: inner_top, right: inner_right, bottom: mid_btn };
    let add_btn_clear = RECT { left: inner_right - add_btn_w, top: mid_btn + 4, right: inner_right, bottom: inner_bottom };

    Geo {
        cap_list, cap_text, cap_img_ocr, cap_img_full, cap_del_btn,
        add_group, add_edit, add_btn_save, add_btn_clear,
        recog_list, recog_text, trans_list, trans_prompt, trans_text,
        exp_list, exp_prompt, exp_text,
        filter_group, label_y1, label_y2, row1_btn_y, row2_btn_y, col_x, col_w,
        split_v, split_h,
    }
}

/// ウィンドウサイズに合わせて子コントロールを配置
fn layout(h: HWND) {
    unsafe {
        let g = geometry(h);
        let mv = |id: i32, x: i32, y: i32, cw: i32, ch: i32| {
            let _ = SetWindowPos(
                crate::ui_helpers::get_dlg_item(h, id),
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

        // 【絞り込み】グループ: 全文検索ラベル + 検索欄 + アプリコンボ
        let (fx, fy, fw, fh) = r(&g.filter_group);
        mv(IDC_GRP_FILTER, fx, fy, fw, fh);
        let row_y = fy + GTOP;
        let label_y = row_y + (BTN_H - LBL_H) / 2;
        let mut x = fx + 8;
        mv(IDC_LBL_SEARCH, x, label_y, 70, LBL_H);
        x += 70 + 6;
        mv(IDC_SEARCH_EDIT, x, row_y, 200, BTN_H);
        x += 200 + 6;
        mv(IDC_EXE_COMBO, x, row_y, 160, 200);

        // グループの右側: CSV出力・最新に更新・ログを全削除
        let mut rc = RECT::default();
        let _ = GetClientRect(h, &mut rc);
        mv(IDC_BTN_EXPORT, fx + fw + gap, row_y, 90, BTN_H);
        mv(IDC_BTN_REFRESH, rc.right - PAD - 90 - gap - 100, row_y, 90, BTN_H);
        mv(IDC_BTN_CLEAR, rc.right - PAD - 100, row_y, 100, BTN_H);

        // 見出しラベル (2x2)。v0.4.8: 上段=入力内容/翻訳結果、下段=読み取り結果/解説結果
        mv(IDC_LBL_CAP, g.col_x[0], g.label_y1, g.col_w[0], LBL_H);
        mv(IDC_LBL_TRANS, g.col_x[1], g.label_y1, g.col_w[1], LBL_H);
        mv(IDC_LBL_RECOG, g.col_x[0], g.label_y2, g.col_w[0], LBL_H);
        mv(IDC_LBL_EXP, g.col_x[1], g.label_y2, g.col_w[1], LBL_H);

        // 【入力内容】(上段左)
        let (x0, y0, w0, h0) = r(&g.cap_list);
        mv(IDC_CAP_LV, x0, y0, w0, h0);
        let (x1, y1, w1, h1) = r(&g.cap_text);
        mv(IDC_CAP_DETAIL, x1, y1, w1, h1);
        let (xd, yd, wd, hd) = r(&g.cap_del_btn);
        mv(IDC_BTN_DEL_CAP, xd, yd, wd, hd);
        // 【入力追加】グループ: テキストボックス + 追加/クリアボタン
        let (gx, gy, gw, gh) = r(&g.add_group);
        mv(IDC_GRP_ADD, gx, gy, gw, gh);
        let (ex, ey, ew, eh) = r(&g.add_edit);
        mv(IDC_ADD_TEXT_EDIT, ex, ey, ew, eh);
        let (sx, sy, sw, sh) = r(&g.add_btn_save);
        mv(IDC_BTN_ADD_SAVE, sx, sy, sw, sh);
        let (cx, cy, cw, ch) = r(&g.add_btn_clear);
        mv(IDC_BTN_ADD_CANCEL, cx, cy, cw, ch);

        // 【読み取り結果】(下段左、v0.4.8: 位置入替)
        let (x2, y2, w2, h2) = r(&g.recog_list);
        mv(IDC_RECOG_LV, x2, y2, w2, h2);
        let (x3, y3, w3, h3) = r(&g.recog_text);
        mv(IDC_RECOG_DETAIL, x3, y3, w3, h3);
        let mut x = g.col_x[0];
        mv(IDC_OCR_COMBO, x, g.row2_btn_y, 105, 200);
        x += 105 + 4;
        mv(IDC_BTN_REOCR, x, g.row2_btn_y, 60, BTN_H);
        x += 60 + gap;
        mv(IDC_BTN_DEL_RECOG, x, g.row2_btn_y, 50, BTN_H);
        x += 50 + gap;
        let tag_w = (g.col_x[0] + g.col_w[0] - x - 70 - 4).max(60);
        mv(IDC_TAG_EDIT, x, g.row2_btn_y, tag_w, BTN_H);
        mv(IDC_BTN_SAVE_TAG, x + tag_w + 4, g.row2_btn_y, 70, BTN_H);

        // 【翻訳結果】(上段右、v0.4.8: 位置入替 + 3列表示)
        let (x4, y4, w4, h4) = r(&g.trans_list);
        mv(IDC_TRANS_LV, x4, y4, w4, h4);
        let (xp4, yp4, wp4, hp4) = r(&g.trans_prompt);
        mv(IDC_TRANS_PROMPT, xp4, yp4, wp4, hp4);
        let (x5, y5, w5, h5) = r(&g.trans_text);
        mv(IDC_TRANS_DETAIL, x5, y5, w5, h5);
        let mut x = g.col_x[1];
        mv(IDC_TR_COMBO, x, g.row1_btn_y, 105, 200);
        x += 105 + 4;
        mv(IDC_BTN_RETRANS, x, g.row1_btn_y, 60, BTN_H);
        x += 60 + gap;
        mv(IDC_BTN_DEL_TRANS, x, g.row1_btn_y, 50, BTN_H);

        // 【解説結果】(下段右、3列表示)
        let (x6, y6, w6, h6) = r(&g.exp_list);
        mv(IDC_EXP_LV, x6, y6, w6, h6);
        let (xp6, yp6, wp6, hp6) = r(&g.exp_prompt);
        mv(IDC_EXP_PROMPT, xp6, yp6, wp6, hp6);
        let (x7, y7, w7, h7) = r(&g.exp_text);
        mv(IDC_EXP_DETAIL, x7, y7, w7, h7);
        mv(IDC_EXP_COMBO, g.col_x[1], g.row2_btn_y, 160, 200);
        mv(IDC_BTN_REEXPLAIN, g.col_x[1] + 160 + 4, g.row2_btn_y, 80, BTN_H);
        mv(IDC_BTN_DEL_EXP, g.col_x[1] + 160 + 4 + 80 + gap, g.row2_btn_y, 90, BTN_H);
    }
}

fn fmt_ts(ts_ms: i64) -> String {
    if ts_ms <= 0 {
        return String::new();
    }
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    
    // ts_ms: UNIXエポック(1970-01-01)からのミリ秒
    // FILETIME: 1601-01-01からの100ナノ秒単位
    // 差分は 11644473600 秒
    let ft_val = (ts_ms as u64 * 10_000) + 116444736000000000;
    let ft = FILETIME {
        dwLowDateTime: (ft_val & 0xFFFFFFFF) as u32,
        dwHighDateTime: (ft_val >> 32) as u32,
    };
    
    let mut st_utc = SYSTEMTIME::default();
    let mut st_local = SYSTEMTIME::default();
    
    unsafe {
        let _ = FileTimeToSystemTime(&ft, &mut st_utc);
        // ローカルタイムゾーンに変換
        let _ = SystemTimeToTzSpecificLocalTime(None, &st_utc, &mut st_local);
    }
    
    format!(
        "{:04}/{:02}/{:02} {:02}:{:02}:{:02}",
        st_local.wYear, st_local.wMonth, st_local.wDay,
        st_local.wHour, st_local.wMinute, st_local.wSecond
    )
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
        let _ = SetWindowTextW(crate::ui_helpers::get_dlg_item(hwnd(), id), PCWSTR(wide.as_ptr()));
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
    let query = crate::ui_helpers::get_ctl_text(h, IDC_SEARCH_EDIT);
    let exe_idx = crate::ui_helpers::combo_get_sel(crate::ui_helpers::get_dlg_item(h, IDC_EXE_COMBO));
    // index 0 は「全アプリ」
    let app_exe = if exe_idx == 0 { String::new() } else { crate::ui_helpers::combo_get_item_text(crate::ui_helpers::get_dlg_item(h, IDC_EXE_COMBO), exe_idx) };

    let caps = logdb::search_captures(&query, &app_exe, 1000);
    let cap_lv = crate::ui_helpers::get_dlg_item(h, IDC_CAP_LV);
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
        st.full_image = None;
        st.crop_rect = None;
    });
    lv_clear(crate::ui_helpers::get_dlg_item(h, IDC_RECOG_LV));
    lv_clear(crate::ui_helpers::get_dlg_item(h, IDC_TRANS_LV));
    lv_clear(crate::ui_helpers::get_dlg_item(h, IDC_EXP_LV));
    set_edit(IDC_CAP_DETAIL, "");
    set_edit(IDC_RECOG_DETAIL, "");
    set_edit(IDC_TRANS_PROMPT, "");
    set_edit(IDC_TRANS_DETAIL, "");
    set_edit(IDC_EXP_PROMPT, "");
    set_edit(IDC_EXP_DETAIL, "");
    unsafe {
        // bErase=false: WM_ERASEBKGNDでの全消去→WM_PAINTでの再描画という二度塗りをなくし、
        // 画像パネルの点滅を防ぐ (paint_imageが画像の有無に関わらず自身の領域を必ず塗り切る)
        let _ = InvalidateRect(Some(h), None, false);
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
    if let Some(ct) = &cap.control_type
        && !ct.is_empty() {
            d.push_str(&format!("コントロール種類: {ct}\n"));
        }
    if let (Some(w), Some(hh)) = (cap.image_w, cap.image_h) {
        d.push_str(&format!("画像: {w}x{hh}\n"));
    }
    if let Some(fk) = &cap.focus_kind {
        let fy = cap.focus_y.map(|y| format!(" (Y={y:.0})")).unwrap_or_default();
        d.push_str(&format!("OCR基準: {fk}{fy}\n"));
    }
    if let (Some(fw), Some(fh)) = (cap.full_image_w, cap.full_image_h) {
        d.push_str(&format!("全体画像: {fw}x{fh}\n"));
    }
    if let Some(p) = &cap.uia_path
        && !p.is_empty() {
            d.push_str(&format!("\n【UIAパス】\n{p}\n"));
        }
    set_edit(IDC_CAP_DETAIL, &d);

    // 画像デコード (OCR対象画像 / 対象アプリ全体画像。SPECv0.5.2追補)
    let image = cap.image_path.as_ref().and_then(|rel| decode_png(&logdb::logs_dir().join(rel)));
    let full_image = cap.full_image_path.as_ref().and_then(|rel| decode_png(&logdb::logs_dir().join(rel)));
    let crop_rect = match (cap.crop_x, cap.crop_y, cap.crop_w, cap.crop_h) {
        (Some(x), Some(y), Some(w), Some(h)) => Some((x as i32, y as i32, w as i32, h as i32)),
        _ => None,
    };

    // 読み取り結果一覧
    let recogs = logdb::recognitions_for(cap.id);
    let recog_lv = crate::ui_helpers::get_dlg_item(h, IDC_RECOG_LV);
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
        st.full_image = full_image;
        st.crop_rect = crop_rect;
    });
    lv_clear(crate::ui_helpers::get_dlg_item(h, IDC_TRANS_LV));
    lv_clear(crate::ui_helpers::get_dlg_item(h, IDC_EXP_LV));
    set_edit(IDC_RECOG_DETAIL, "");
    set_edit(IDC_TRANS_PROMPT, "");
    set_edit(IDC_TRANS_DETAIL, "");
    set_edit(IDC_EXP_PROMPT, "");
    set_edit(IDC_EXP_DETAIL, "");
    if has_recog {
        lv_select(recog_lv, 0);
        on_recog_selected(0);
    }
    unsafe {
        // bErase=false: 二度塗りによる画像パネルの点滅を防ぐ (reload と同じ理由)
        let _ = InvalidateRect(Some(h), None, false);
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
        let _ = SetWindowTextW(crate::ui_helpers::get_dlg_item(h, IDC_TAG_EDIT), PCWSTR(wide.as_ptr()));
    }

    // 翻訳結果一覧
    let trans = logdb::translations_for(recog.id);
    let trans_lv = crate::ui_helpers::get_dlg_item(h, IDC_TRANS_LV);
    lv_clear(trans_lv);
    for (i, t) in trans.iter().enumerate() {
        let dir = format!("{}→{}", t.source_lang, t.target_lang);
        let tok = match (t.tokens_in, t.tokens_out) {
            (Some(a), Some(b)) => format!("{a}/{b}"),
            _ => String::new(),
        };
        let text = if t.success { t.translated_text.clone() } else { format!("[エラー] {}", t.error) };
        lv_add_row(trans_lv, i as i32, &[
            fmt_ts(t.ts_ms),
            t.engine.clone(),
            dir,
            t.llm_profile.clone().unwrap_or_default(),
            tok,
            truncate(&text, 60),
        ]);
    }

    // 解説結果一覧
    let exps = logdb::explanations_for(recog.id);
    let exp_lv = crate::ui_helpers::get_dlg_item(h, IDC_EXP_LV);
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

/// 翻訳結果選択時: 2列目に送信JSON(入力プロンプト相当)、3列目に訳文のみを表示する
/// (日時・エンジン・トークン等はリストビューの列で確認できるためテキストには含めない; v0.4.8)。
fn on_trans_selected(idx: usize) {
    let t = STATE.with(|s| s.borrow().trans.get(idx).cloned());
    let Some(t) = t else { return };
    STATE.with(|s| s.borrow_mut().sel_trans = Some(idx));

    let prompt = if !t.request_json.is_empty() {
        pretty_json(&t.request_json)
    } else {
        STATE.with(|s| {
            let st = s.borrow();
            if let Some(r_idx) = st.sel_recog {
                st.recogs.get(r_idx).map(|r| r.source_text.clone()).unwrap_or_default()
            } else {
                String::new()
            }
        })
    };
    set_edit(IDC_TRANS_PROMPT, &prompt);

    let body = if t.success {
        t.translated_text.clone()
    } else {
        format!("[エラー]\n{}", t.error)
    };
    set_edit(IDC_TRANS_DETAIL, &body);
}

/// 解説結果選択時: 2列目に送信プロンプト、3列目に解説文のみを表示する
/// (日時・プロファイル・トークン等はリストビューの列で確認できるためテキストには含めない; v0.4.8)。
fn on_exp_selected(idx: usize) {
    let e = STATE.with(|s| s.borrow().exps.get(idx).cloned());
    let Some(e) = e else { return };
    STATE.with(|s| s.borrow_mut().sel_exp = Some(idx));

    set_edit(IDC_EXP_PROMPT, &e.input_text);

    let body = if e.success {
        e.explanation_text.clone()
    } else {
        format!("[エラー]\n{}", e.error)
    };
    set_edit(IDC_EXP_DETAIL, &body);
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

/// 画像を area 内へアスペクト比維持で縮小描画する(背景は暗灰色で塗る)。
/// 戻り値は実際の描画位置とスケール (赤枠など重ね描画する座標計算に使う)。
fn draw_scaled_image(hdc: HDC, area: RECT, iw: u32, ih: u32, rgba: &[u8]) -> (i32, i32, f32) {
    unsafe {
        let img_w = (area.right - area.left).max(1);
        let img_h = (area.bottom - area.top).max(20);

        // アスペクト比維持で img_w×img_h に収める
        let scale = (img_w as f32 / iw as f32).min(img_h as f32 / ih as f32).min(1.0);
        let dw = (iw as f32 * scale) as i32;
        let dh = (ih as f32 * scale) as i32;
        // 表示領域内で上下左右中央寄せ (§6)
        let draw_x = area.left + (img_w - dw) / 2;
        let draw_y = area.top + (img_h - dh) / 2;

        // 背景を塗る
        let bg = windows::Win32::Graphics::Gdi::CreateSolidBrush(COLORREF(0x00202020));
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
                biWidth: iw as i32,
                biHeight: -(ih as i32), // トップダウン
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
            draw_x,
            draw_y,
            dw,
            dh,
            0,
            0,
            iw as i32,
            ih as i32,
            Some(bgra.as_ptr() as *const _),
            &bmi,
            DIB_RGB_COLORS,
            windows::Win32::Graphics::Gdi::SRCCOPY,
        );
        (draw_x, draw_y, scale)
    }
}

/// 入力ブロック右下にキャプチャ画像を縮小描画する。OCR対象画像(左)と対象アプリ全体画像
/// (右)を並べ、全体画像には抽出範囲を赤枠で示す (SPECv0.5.2追補)。
/// area を背景色(暗灰色)で塗りつぶすだけ。画像が無いときに前回の描画を消すのに使う
/// (WM_ERASEBKGNDに頼らず、この関数とdraw_scaled_imageの塗りつぶしだけで領域を
/// 常に埋め切ることで、選択変更のたびに全体を消去→再描画する二度塗りをなくし点滅を防ぐ)。
fn clear_area(hdc: HDC, area: RECT) {
    unsafe {
        let bg = windows::Win32::Graphics::Gdi::CreateSolidBrush(COLORREF(0x00202020));
        windows::Win32::Graphics::Gdi::FillRect(hdc, &area, bg);
        let _ = windows::Win32::Graphics::Gdi::DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(bg.0));
    }
}

fn paint_image(h: HWND) {
    // geometry() は STATE を借用しないが、borrow順を固定するため先に取得する
    let g = geometry(h);
    STATE.with(|s| {
        let st = s.borrow();
        unsafe {
            let hdc = windows::Win32::Graphics::Gdi::GetDC(Some(h));
            match st.image.as_ref() {
                Some((iw, ih, rgba)) => {
                    draw_scaled_image(hdc, g.cap_img_ocr, *iw, *ih, rgba);
                }
                None => clear_area(hdc, g.cap_img_ocr),
            }
            match st.full_image.as_ref() {
                Some((iw, ih, rgba)) => {
                    let (dx, dy, scale) = draw_scaled_image(hdc, g.cap_img_full, *iw, *ih, rgba);
                    if let Some((cx, cy, cw, ch)) = st.crop_rect {
                        let r = RECT {
                            left: dx + (cx as f32 * scale).round() as i32,
                            top: dy + (cy as f32 * scale).round() as i32,
                            right: dx + ((cx + cw) as f32 * scale).round() as i32,
                            bottom: dy + ((cy + ch) as f32 * scale).round() as i32,
                        };
                        crate::image_preview::draw_red_box(hdc, r, 3);
                    }
                }
                None => clear_area(hdc, g.cap_img_full),
            }
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
    let engine = OCR_ENGINES[crate::ui_helpers::combo_get_sel(crate::ui_helpers::get_dlg_item(h, IDC_OCR_COMBO)).min(OCR_ENGINES.len() - 1)].0.to_string();
    let hwnd_isize = h.0 as isize;
    RELOAD_FOCUS.with(|f| *f.borrow_mut() = ReloadFocus::NewestRecog);
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
    let engine = TR_ENGINES[crate::ui_helpers::combo_get_sel(crate::ui_helpers::get_dlg_item(h, IDC_TR_COMBO)).min(TR_ENGINES.len() - 1)].0.to_string();
    let hwnd_isize = h.0 as isize;
    RELOAD_FOCUS.with(|f| *f.borrow_mut() = ReloadFocus::NewestTrans(recog_id));
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

/// 選択した読み取り結果に対し、選択したLLMプロファイルで解説を(再)生成して追記する
/// (ログビューア拡張 §2)。まだ解説が無いテキストへの新規生成にも使える。
fn start_reexplain(h: HWND) {
    let sel = STATE.with(|s| {
        let st = s.borrow();
        let cap = st.sel_cap.and_then(|i| st.caps.get(i)).cloned();
        st.sel_recog.and_then(|i| st.recogs.get(i)).map(|r| (r.clone(), cap))
    });
    let Some((recog, cap)) = sel else {
        unsafe { MessageBoxW(Some(h), w!("読み取り結果を選択してください。"), w!("再解説"), MB_OK); }
        return;
    };
    if recog.source_text.trim().is_empty() {
        unsafe { MessageBoxW(Some(h), w!("原文が空のため解説できません。"), w!("再解説"), MB_OK); }
        return;
    }
    let profile_name = crate::ui_helpers::combo_get_item_text(crate::ui_helpers::get_dlg_item(h, IDC_EXP_COMBO), crate::ui_helpers::combo_get_sel(crate::ui_helpers::get_dlg_item(h, IDC_EXP_COMBO)));
    if profile_name.is_empty() {
        unsafe {
            MessageBoxW(
                Some(h),
                w!("LLM APIプロファイルが設定されていません。設定画面で追加してください。"),
                w!("再解説"),
                MB_OK,
            );
        }
        return;
    }
    // 最新の成功した翻訳結果があればプレースホルダに使う (SPECv0.4 §7.1)
    let (translated_text, tr_engine) = logdb::translations_for(recog.id)
        .into_iter()
        .rev()
        .find(|t| t.success)
        .map(|t| (t.translated_text, t.engine))
        .unwrap_or_default();
    let mut pc = cap.map(|c| prompt_ctx_from_cap(&c)).unwrap_or_default();
    pc.original_text = recog.source_text.clone();
    pc.translated_text = translated_text;
    pc.ocr_engine = if recog.engine == "uia" || recog.engine == "manual" { String::new() } else { recog.engine.clone() };
    pc.tr_engine = tr_engine;
    let recog_id = recog.id;
    let hwnd_isize = h.0 as isize;
    std::thread::spawn(move || {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }
        let cfg = crate::config::Config::load();
        let notify = || unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                Some(HWND(hwnd_isize as *mut _)),
                WM_APP_RELOAD,
                WPARAM(0),
                LPARAM(0),
            );
        };
        let Some(prof) = cfg.api_profiles.iter().find(|p| p.name == profile_name) else {
            notify();
            return;
        };
        let prompt = cfg.fill_prompt(&prof.explain_prompt, &pc);
        let t0 = std::time::Instant::now();
        let result = crate::llm_api::call(prof, &crate::llm_api::LlmRequest::text(&prompt));
        let ms = t0.elapsed().as_millis();
        match result {
            Ok(res) => logdb::log_explanation(
                recog_id, &prof.name, ms, &prompt, Some(&res.text), None, res.tokens_in, res.tokens_out,
            ),
            Err(e) => logdb::log_explanation(recog_id, &prof.name, ms, &prompt, None, Some(&e), None, None),
        }
        notify();
    });
}

/// リロード後、以前選択していた入力行を復元する
fn restore_cap_selection(h: HWND, old_idx: Option<usize>) {
    if let Some(old_idx) = old_idx {
        let cap_lv = crate::ui_helpers::get_dlg_item(h, IDC_CAP_LV);
        let count = lv_count(cap_lv);
        if count > 0 {
            let new_idx = if (old_idx as i32) < count { old_idx } else { (count - 1) as usize };
            lv_select(cap_lv, new_idx as i32);
            on_cap_selected(new_idx);
        }
    }
}

/// 選択中4階層のID一覧(【最新に更新】での選択復元・再OCR/再翻訳後の対象特定に使う)
fn current_selection_ids() -> (Option<i64>, Option<i64>, Option<i64>, Option<i64>) {
    STATE.with(|s| {
        let st = s.borrow();
        (
            st.sel_cap.and_then(|i| st.caps.get(i)).map(|c| c.id),
            st.sel_recog.and_then(|i| st.recogs.get(i)).map(|r| r.id),
            st.sel_trans.and_then(|i| st.trans.get(i)).map(|t| t.id),
            st.sel_exp.and_then(|i| st.exps.get(i)).map(|e| e.id),
        )
    })
}

/// 【最新に更新】用: リロード後、入力→読み取り→翻訳→解説の各選択をID一致で
/// 存在する範囲で復元する。
fn restore_full_selection(h: HWND, saved: (Option<i64>, Option<i64>, Option<i64>, Option<i64>)) {
    let (cap_id, recog_id, trans_id, exp_id) = saved;
    let Some(cap_id) = cap_id else { return };
    let Some(idx) = STATE.with(|s| s.borrow().caps.iter().position(|c| c.id == cap_id)) else { return };
    lv_select(crate::ui_helpers::get_dlg_item(h, IDC_CAP_LV), idx as i32);
    on_cap_selected(idx);

    if let Some(rid) = recog_id
        && let Some(ridx) = STATE.with(|s| s.borrow().recogs.iter().position(|r| r.id == rid))
    {
        lv_select(crate::ui_helpers::get_dlg_item(h, IDC_RECOG_LV), ridx as i32);
        on_recog_selected(ridx);
    }
    if let Some(tid) = trans_id
        && let Some(tidx) = STATE.with(|s| s.borrow().trans.iter().position(|t| t.id == tid))
    {
        lv_select(crate::ui_helpers::get_dlg_item(h, IDC_TRANS_LV), tidx as i32);
        on_trans_selected(tidx);
    }
    if let Some(eid) = exp_id
        && let Some(eidx) = STATE.with(|s| s.borrow().exps.iter().position(|e| e.id == eid))
    {
        lv_select(crate::ui_helpers::get_dlg_item(h, IDC_EXP_LV), eidx as i32);
        on_exp_selected(eidx);
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
            if in_rect(&g.split_v, x, y) {
                SPLIT_DRAG.with(|d| *d.borrow_mut() = Some(1));
                unsafe { SetCapture(h); }
            } else if in_rect(&g.split_h, x, y) {
                SPLIT_DRAG.with(|d| *d.borrow_mut() = Some(2));
                unsafe { SetCapture(h); }
            } else if in_rect(&g.cap_img_ocr, x, y) {
                // OCR対象画像クリック → プレビューウィンドウ
                let img = STATE.with(|s| s.borrow().image.clone());
                crate::image_preview::open_preview(h, crate::image_preview::ImgKind::Ocr, img, None);
            } else if in_rect(&g.cap_img_full, x, y) {
                // 全体画像クリック → プレビューウィンドウ(赤枠付き)
                let (img, box_rect) = STATE.with(|s| {
                    let st = s.borrow();
                    (st.full_image.clone(), st.crop_rect)
                });
                crate::image_preview::open_preview(h, crate::image_preview::ImgKind::Full, img, box_rect);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let axis = SPLIT_DRAG.with(|d| *d.borrow());
            if let Some(axis) = axis {
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                let (grid_left, grid_top, grid_right, grid_bottom) = grid_bounds(h);
                SPLIT.with(|s| {
                    let mut sp = s.borrow_mut();
                    match axis {
                        1 => {
                            let rx = (x - grid_left) as f32 / (grid_right - grid_left).max(1) as f32;
                            sp.0 = rx.clamp(0.0, 1.0);
                        }
                        2 => {
                            let ry = (y - grid_top) as f32 / (grid_bottom - grid_top).max(1) as f32;
                            sp.1 = ry.clamp(0.0, 1.0);
                        }
                        _ => {}
                    }
                });
                layout(h);
                unsafe {
                    let _ = InvalidateRect(Some(h), None, true);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if SPLIT_DRAG.with(|d| d.borrow_mut().take()).is_some() {
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let axis = SPLIT_DRAG.with(|d| *d.borrow());
            let cursor = if let Some(axis) = axis {
                Some(if axis == 1 { IDC_SIZEWE } else { IDC_SIZENS })
            } else {
                let mut pt = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut pt);
                    let _ = ScreenToClient(h, &mut pt);
                }
                let g = geometry(h);
                if in_rect(&g.split_v, pt.x, pt.y) {
                    Some(IDC_SIZEWE)
                } else if in_rect(&g.split_h, pt.x, pt.y) {
                    Some(IDC_SIZENS)
                } else {
                    None
                }
            };
            if let Some(cursor) = cursor {
                unsafe {
                    let _ = SetCursor(Some(LoadCursorW(None, cursor).unwrap_or_default()));
                }
                return LRESULT(1);
            }
            unsafe { DefWindowProcW(h, msg, wparam, lparam) }
        }
        WM_APP_RELOAD => {
            // 再OCR/再翻訳後のリロード: 前の選択アイテムを復元したうえで、新規追加された
            // アイテム(認識行/翻訳行)へフォーカスを当てる
            let sel_before = STATE.with(|s| s.borrow().sel_cap);
            let focus = RELOAD_FOCUS.with(|f| std::mem::replace(&mut *f.borrow_mut(), ReloadFocus::None));
            reload();
            restore_cap_selection(h, sel_before);
            match focus {
                ReloadFocus::NewestRecog => {
                    let recog_lv = crate::ui_helpers::get_dlg_item(h, IDC_RECOG_LV);
                    let n = lv_count(recog_lv);
                    if n > 0 {
                        lv_select(recog_lv, n - 1);
                        on_recog_selected((n - 1) as usize);
                    }
                }
                ReloadFocus::NewestTrans(recog_id) => {
                    if let Some(ridx) = STATE.with(|s| s.borrow().recogs.iter().position(|r| r.id == recog_id)) {
                        lv_select(crate::ui_helpers::get_dlg_item(h, IDC_RECOG_LV), ridx as i32);
                        on_recog_selected(ridx);
                    }
                    let trans_lv = crate::ui_helpers::get_dlg_item(h, IDC_TRANS_LV);
                    let n = lv_count(trans_lv);
                    if n > 0 {
                        lv_select(trans_lv, n - 1);
                        on_trans_selected((n - 1) as usize);
                    }
                }
                ReloadFocus::None => {}
            }
            LRESULT(0)
        }
        WM_NOTIFY => {
            let nmhdr = unsafe { &*(lparam.0 as *const NMHDR) };
            if nmhdr.code == LVN_ITEMCHANGED {
                let id = nmhdr.idFrom as i32;
                match id {
                    IDC_CAP_LV => {
                        if let Some(sel) = lv_selected(crate::ui_helpers::get_dlg_item(h, IDC_CAP_LV)) {
                            let cur = STATE.with(|s| s.borrow().sel_cap);
                            if cur != Some(sel) {
                                on_cap_selected(sel);
                            }
                        }
                    }
                    IDC_RECOG_LV => {
                        if let Some(sel) = lv_selected(crate::ui_helpers::get_dlg_item(h, IDC_RECOG_LV)) {
                            let cur = STATE.with(|s| s.borrow().sel_recog);
                            if cur != Some(sel) {
                                on_recog_selected(sel);
                            }
                        }
                    }
                    IDC_TRANS_LV => {
                        if let Some(sel) = lv_selected(crate::ui_helpers::get_dlg_item(h, IDC_TRANS_LV)) {
                            on_trans_selected(sel);
                        }
                    }
                    IDC_EXP_LV => {
                        if let Some(sel) = lv_selected(crate::ui_helpers::get_dlg_item(h, IDC_EXP_LV)) {
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
                IDC_BTN_REFRESH => {
                    // 更新後、現在フォーカスの当たっているアイテムをID一致で再選択する
                    let saved = current_selection_ids();
                    reload();
                    restore_full_selection(h, saved);
                }
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
                            MessageBoxW(Some(h), w!("ログを削除しました。"), crate::util::display_name_pcwstr(), MB_OK);
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
                IDC_BTN_REEXPLAIN => start_reexplain(h),
                IDC_BTN_DEL_EXP => {
                    let (recog_idx, eid) = STATE.with(|s| {
                        let st = s.borrow();
                        (st.sel_recog, st.sel_exp.and_then(|i| st.exps.get(i)).map(|e| e.id))
                    });
                    if let Some(eid) = eid {
                        logdb::delete_explanation(eid);
                        if let Some(ri) = recog_idx {
                            on_recog_selected(ri);
                        }
                    }
                }
                IDC_BTN_ADD_CANCEL => unsafe {
                    let _ = SetWindowTextW(crate::ui_helpers::get_dlg_item(h, IDC_ADD_TEXT_EDIT), w!(""));
                },
                IDC_BTN_ADD_SAVE => {
                    let text = crate::ui_helpers::get_multiline_text(h, IDC_ADD_TEXT_EDIT);
                    if text.trim().is_empty() {
                        unsafe {
                            MessageBoxW(Some(h), w!("テキストを入力してください。"), w!("テキスト追加"), MB_OK);
                        }
                    } else {
                        // 取得元アプリ名は一律【FocusTranslator】として記録する
                        let cid = logdb::log_capture(
                            "manual", Some(MANUAL_APP_NAME), Some(MANUAL_APP_NAME), None, None, None, false,
                            logdb::CaptureExtent::default(),
                        );
                        if let Some(cid) = cid {
                            logdb::log_recognition(cid, "manual", "manual", 0, Some(&text), None, None);
                        }
                        unsafe {
                            let _ = SetWindowTextW(crate::ui_helpers::get_dlg_item(h, IDC_ADD_TEXT_EDIT), w!(""));
                        }
                        reload();
                        // 追加したアイテムにフォーカスを当てる (captures は id DESC なので先頭が最新)
                        let cap_lv = crate::ui_helpers::get_dlg_item(h, IDC_CAP_LV);
                        if lv_count(cap_lv) > 0 {
                            lv_select(cap_lv, 0);
                            on_cap_selected(0);
                        }
                    }
                }
                IDC_BTN_SAVE_TAG => {
                    let recog_id = STATE.with(|s| {
                        let st = s.borrow();
                        st.sel_recog.and_then(|idx| st.recogs.get(idx).map(|r| r.id))
                    });
                    if let Some(rid) = recog_id {
                        let tags = crate::ui_helpers::get_ctl_text(h, IDC_TAG_EDIT);
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
