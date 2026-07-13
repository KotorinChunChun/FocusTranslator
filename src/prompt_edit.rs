// プロンプト編集ウィンドウ (SPEC v0.4.7)
// テンプレート編集・変数確認・送信内容プレビューを1画面で行う3ペイン構成のウィンドウ。
// - モードA (ctx=None): 設定画面から。ペイン1+2のみ。テンプレートの保存だけ行う。
// - モードB (ctx=Some): オーバーレイ「解説プロンプトを編集して送信」から。ペイン3で
//   置換済みプロンプトを編集し on_submit で送信できる。
// 単一インスタンス。モーダルにせず main のメッセージループをそのまま使う。
use std::cell::RefCell;
use std::ffi::c_void;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_WINDOW, CreateFontW, DEFAULT_CHARSET,
    DEFAULT_PITCH, FONT_OUTPUT_PRECISION, FW_NORMAL, HBRUSH,
};
use windows::Win32::UI::Controls::{
    INITCOMMONCONTROLSEX, InitCommonControlsEx, ICC_LISTVIEW_CLASSES, LVCF_SUBITEM, LVCF_TEXT,
    LVCF_WIDTH, LVCOLUMNW, LVIF_TEXT, LVITEMW, LVM_INSERTCOLUMNW, LVM_INSERTITEMW,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMTEXTW, LVS_EX_FULLROWSELECT, LVS_REPORT,
    LVS_SHOWSELALWAYS, LVS_SINGLESEL, NMHDR, NMITEMACTIVATE, NM_DBLCLK,
};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetDlgItem,
    GetWindowLongPtrW, HMENU, HWND_TOPMOST, IDC_ARROW, IDYES, IsWindow, LoadCursorW,
    MB_ICONQUESTION, MB_OK, MB_YESNO, MessageBoxW, PostMessageW, RegisterClassW, SW_SHOW,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SendMessageW, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_DESTROY,
    WM_NOTIFY, WM_SETFONT, WM_SIZE, WNDCLASSW, WS_BORDER, WS_CHILD, WS_EX_APPWINDOW,
    WS_EX_CLIENTEDGE, WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, w};

use crate::config::{Config, PromptContext};
use crate::ui_helpers::{get_multiline_text, set_multiline_text};
use crate::util::to_wide;

/// プロンプトの種別
#[derive(Clone, Copy, PartialEq)]
pub enum PromptKind {
    Translate,
    Ocr,
    Explain,
}

/// プロファイル名と該当種別のテンプレート (呼び出し元のメモリ状態を渡す)
pub struct ProfilePrompt {
    pub name: String,
    pub template: String,
}

// コントロールID (SPEC v0.4.7 §5.1)
const IDC_PROFILE: i32 = 100;
const IDC_VARLIST: i32 = 101;
const IDC_TEMPLATE: i32 = 102;
const IDC_SAVE: i32 = 103;
const IDC_PREVIEW: i32 = 104;
const IDC_REGEN: i32 = 105;
const IDC_SUBMIT: i32 = 106;
// ペインの見出しラベル (WM_SIZE で再配置するためIDを持たせる)
const IDC_LBL_TEMPLATE: i32 = 110;
const IDC_LBL_PREVIEW: i32 = 111;

// windows クレートに定義がないスタイル・通知コード
const ES_MULTILINE: u32 = 0x0004;
const ES_AUTOVSCROLL: u32 = 0x0040;
const ES_WANTRETURN: u32 = 0x1000;
const CBS_DROPDOWNLIST: u32 = 0x0003;
const EN_CHANGE: u32 = 0x0300;
const CBN_SELCHANGE: u32 = 1;
const EM_REPLACESEL: u32 = 0x00C2;

const PAD: i32 = 10;
const COMBO_H: i32 = 26;
const LBL_H: i32 = 18;
const BTN_W: i32 = 100;
const BTN_H: i32 = 28;
/// ペイン1の固定幅 (モードA: 値列なし / モードB: 値列あり)
const PANE1_W_A: i32 = 320;
const PANE1_W_B: i32 = 440;
/// ウィンドウ初期サイズ (HD 1280x720 のワークエリアに収まること)
const WIN_W_A: i32 = 800;
const WIN_W_B: i32 = 1260;
const WIN_H: i32 = 520;

/// プレースホルダ変数の一覧 (SPECv0.4 §7.1): (変数名, 意味)
const VARS: [(&str, &str); 9] = [
    ("source_lang", "翻訳元言語 (例: en)"),
    ("target_lang", "翻訳先言語 (例: ja)"),
    ("original_text", "OCR/UIAで取得した原文"),
    ("translated_text", "訳文 (翻訳前は空)"),
    ("app_title", "対象アプリのタイトル"),
    ("app_exe", "対象アプリの実行ファイル名"),
    ("uia_path", "UIA要素のパス"),
    ("ocr_engine", "OCRエンジン名"),
    ("tr_engine", "翻訳エンジン名"),
];

struct State {
    /// プロファイル名と該当種別のテンプレート。保存時に該当要素も更新し、
    /// プロファイルを切り替えて戻ったときに保存済みの内容が出るようにする。
    profiles: Vec<ProfilePrompt>,
    /// 現在ペイン2に読み込まれているプロファイルのindex
    cur_idx: usize,
    /// Some(_) でモードB
    ctx: Option<PromptContext>,
    on_save: Box<dyn Fn(&str, &str) -> bool>,
    on_submit: Option<Box<dyn FnOnce(String, String)>>,
    /// ペイン2/3の未保存編集フラグ (§4.6)
    tmpl_dirty: bool,
    preview_dirty: bool,
    /// プログラムからの SetWindowText 中は EN_CHANGE を無視する
    suppress_change: bool,
}

thread_local! {
    static WND: RefCell<isize> = const { RefCell::new(0) };
    static MODE_B: RefCell<bool> = const { RefCell::new(false) };
    static REGISTERED: RefCell<bool> = const { RefCell::new(false) };
}

pub fn hwnd() -> HWND {
    HWND(WND.with(|w| *w.borrow()) as *mut _)
}

pub fn is_open() -> bool {
    let h = hwnd();
    !h.is_invalid() && unsafe { IsWindow(Some(h)).as_bool() }
}

/// GWLP_USERDATA に格納した State への可変参照 (メッセージループは単一スレッド前提)
unsafe fn state_mut<'a>(h: HWND) -> Option<&'a mut State> {
    let ptr = unsafe { GetWindowLongPtrW(h, GWLP_USERDATA) };
    if ptr == 0 { None } else { Some(unsafe { &mut *(ptr as *mut State) }) }
}

/// テンプレートが未保存なら破棄確認を出す (§4.6)。true = 続行してよい。
fn confirm_discard_template(h: HWND) -> bool {
    let dirty = unsafe { state_mut(h) }.map(|s| s.tmpl_dirty).unwrap_or(false);
    if !dirty {
        return true;
    }
    let r = unsafe {
        MessageBoxW(
            Some(h),
            w!("テンプレートの変更が保存されていません。破棄しますか?"),
            w!("Focus Translator"),
            MB_YESNO | MB_ICONQUESTION,
        )
    };
    r == IDYES
}

/// ウィンドウが開いていれば破棄確認の上で閉じる。false = ユーザーがキャンセル。
pub fn try_close() -> bool {
    if !is_open() {
        return true;
    }
    let h = hwnd();
    if !confirm_discard_template(h) {
        return false;
    }
    unsafe {
        let _ = DestroyWindow(h);
    }
    true
}

/// 設定画面クローズ時の連動クローズ: モードAで開いている場合のみ閉じる。
/// false = ユーザーが破棄をキャンセルした (設定画面のクローズも中止すること)。
pub fn close_for_settings() -> bool {
    if !is_open() || MODE_B.with(|m| *m.borrow()) {
        return true;
    }
    try_close()
}

/// テンプレート保存の既定実装: Config を直接永続化し、main へ設定再読込を通知、
/// 設定画面が開いていればメモリ上 PROFILES も同期する (§4.3)。
/// 該当名のプロファイルが存在しない場合は false (呼び出し側で警告を出す)。
pub fn save_prompt_to_config(kind: PromptKind, name: &str, template: &str) -> bool {
    let mut cfg = Config::load();
    let Some(p) = cfg.api_profiles.iter_mut().find(|p| p.name == name) else {
        return false;
    };
    match kind {
        PromptKind::Translate => p.translate_prompt = template.to_string(),
        PromptKind::Ocr => p.ocr_prompt = template.to_string(),
        PromptKind::Explain => p.explain_prompt = template.to_string(),
    }
    cfg.save();
    unsafe {
        let _ = PostMessageW(
            Some(crate::app_state::main_hwnd()),
            crate::app_state::WM_APP_CFG,
            WPARAM(0),
            LPARAM(0),
        );
    }
    crate::settings::update_prompt_in_memory(name, kind, template);
    true
}

/// ウィンドウタイトル (§2.1)
fn window_title(kind: PromptKind, mode_b: bool) -> &'static str {
    if mode_b {
        return "解説プロンプトの編集と送信";
    }
    match kind {
        PromptKind::Translate => "翻訳プロンプトの編集",
        PromptKind::Ocr => "OCRプロンプトの編集",
        PromptKind::Explain => "解説プロンプトの編集",
    }
}

/// 変数の現在値 (§3.1): source_lang / target_lang は Config、残りは PromptContext 由来
fn var_value(name: &str, cfg: &Config, ctx: &PromptContext) -> String {
    let v = match name {
        "source_lang" => cfg.source_lang.clone(),
        "target_lang" => cfg.target_lang.clone(),
        "original_text" => ctx.original_text.clone(),
        "translated_text" => ctx.translated_text.clone(),
        "app_title" => ctx.app_title.clone(),
        "app_exe" => ctx.app_exe.clone(),
        "uia_path" => ctx.uia_path.clone(),
        "ocr_engine" => ctx.ocr_engine.clone(),
        "tr_engine" => ctx.tr_engine.clone(),
        _ => String::new(),
    };
    // 長い値は1行に収まるよう先頭50文字+「…」で省略表示する
    let one_line: String = v.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
    if one_line.chars().count() > 50 {
        one_line.chars().take(50).collect::<String>() + "…"
    } else {
        one_line
    }
}

fn lv_add_col(lvh: HWND, idx: i32, text: &str, width: i32) {
    unsafe {
        let wide = to_wide(text);
        let mut col = LVCOLUMNW {
            mask: LVCF_TEXT | LVCF_WIDTH | LVCF_SUBITEM,
            cx: width,
            pszText: windows::core::PWSTR(wide.as_ptr() as *mut _),
            iSubItem: idx,
            ..Default::default()
        };
        SendMessageW(lvh, LVM_INSERTCOLUMNW, Some(WPARAM(idx as usize)), Some(LPARAM(&mut col as *mut _ as isize)));
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
            SendMessageW(lvh, LVM_SETITEMTEXTW, Some(WPARAM(row as usize)), Some(LPARAM(&mut sub as *mut _ as isize)));
        }
    }
}

/// プログラム由来の変更として (EN_CHANGE を無視して) マルチラインEDITへ書き込む
fn set_text_programmatic(h: HWND, id: i32, text: &str) {
    if let Some(s) = unsafe { state_mut(h) } {
        s.suppress_change = true;
    }
    set_multiline_text(h, id, text);
    if let Some(s) = unsafe { state_mut(h) } {
        s.suppress_change = false;
    }
}

/// ペイン3をテンプレート(ペイン2の現在値)から再生成する (§4.4)。
/// source_lang / target_lang / glossary は再生成時点の Config::load 値を使う。
fn regenerate_preview(h: HWND) {
    let Some(ctx) = unsafe { state_mut(h) }.and_then(|s| s.ctx.clone()) else {
        return;
    };
    let cfg = Config::load();
    let tmpl = get_multiline_text(h, IDC_TEMPLATE);
    let filled = cfg.fill_prompt(&tmpl, &ctx);
    set_text_programmatic(h, IDC_PREVIEW, &filled);
    if let Some(s) = unsafe { state_mut(h) } {
        s.preview_dirty = false;
    }
}

/// ペイン2へ profiles[idx] のテンプレートを読み込む
fn load_template(h: HWND, idx: usize) {
    let Some(s) = (unsafe { state_mut(h) }) else { return };
    let Some(tmpl) = s.profiles.get(idx).map(|p| p.template.clone()) else { return };
    s.cur_idx = idx;
    set_text_programmatic(h, IDC_TEMPLATE, &tmpl);
    if let Some(s) = unsafe { state_mut(h) } {
        s.tmpl_dirty = false;
    }
}

/// プロンプト編集ウィンドウを開く (SPEC v0.4.7 §5)。
/// pos: 表示位置 (スクリーン座標)。None なら既定位置。
/// ctx: Some(_) でモードB (値列・ペイン3・送信が有効になる)。
/// on_save: 保存ボタン。(プロファイル名, 新テンプレート) を受け取り成功なら true。
/// on_submit: 送信ボタン。(送信プロンプト, プロファイル名)。モードBのみ Some。
#[allow(clippy::too_many_arguments)]
pub fn open(
    inst: HINSTANCE,
    parent: HWND,
    pos: Option<(i32, i32)>,
    kind: PromptKind,
    profiles: Vec<ProfilePrompt>,
    active_idx: usize,
    ctx: Option<PromptContext>,
    on_save: Box<dyn Fn(&str, &str) -> bool>,
    on_submit: Option<Box<dyn FnOnce(String, String)>>,
) {
    if profiles.is_empty() {
        return;
    }
    // 単一インスタンス (§2): 既存ウィンドウは破棄確認の上で開き直す。キャンセルなら中止。
    if !try_close() {
        return;
    }
    let mode_b = ctx.is_some();
    let active_idx = active_idx.min(profiles.len() - 1);
    let state = Box::new(State {
        profiles,
        cur_idx: active_idx,
        ctx,
        on_save,
        on_submit,
        tmpl_dirty: false,
        preview_dirty: false,
        suppress_change: false,
    });
    let ptr = Box::into_raw(state) as isize;

    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_LISTVIEW_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);

        let class_name = w!("FocusTranslatorPromptEdit");
        REGISTERED.with(|r| {
            if !*r.borrow() {
                let wc = WNDCLASSW {
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(wndproc),
                    hInstance: inst,
                    hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                    hIcon: crate::app_state::app_icon(),
                    lpszClassName: class_name,
                    hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as isize as *mut c_void),
                    ..Default::default()
                };
                RegisterClassW(&wc);
                *r.borrow_mut() = true;
            }
        });

        let (x, y) = pos.unwrap_or((CW_USEDEFAULT, CW_USEDEFAULT));
        let win_w = if mode_b { WIN_W_B } else { WIN_W_A };
        let title = to_wide(window_title(kind, mode_b));
        // オーバーレイ/設定画面(WS_EX_TOPMOST)の背後に隠れないよう、このウィンドウもTOPMOSTにする
        let Ok(h) = CreateWindowExW(
            WS_EX_APPWINDOW | WS_EX_TOPMOST,
            class_name,
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            x,
            y,
            win_w,
            WIN_H,
            Some(parent),
            None,
            Some(inst),
            None,
        ) else {
            // ウィンドウを作れなければコールバックを解放して終了
            drop(Box::from_raw(ptr as *mut State));
            return;
        };
        SetWindowLongPtrW(h, GWLP_USERDATA, ptr);
        WND.with(|w| *w.borrow_mut() = h.0 as isize);
        MODE_B.with(|m| *m.borrow_mut() = mode_b);

        build_controls(h, inst, mode_b);

        // 初期内容の投入
        populate_profiles(h, active_idx);
        populate_vars(h, mode_b);
        load_template(h, active_idx);
        if mode_b {
            // 初期表示: 開いた時点のテンプレートに現在値を当てはめた結果 (§3.3)
            regenerate_preview(h);
        }

        // コントロール生成は CreateWindowExW (初回 WM_SIZE) の後なので、明示的に初回レイアウトを行う
        let mut rc = windows::Win32::Foundation::RECT::default();
        let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(h, &mut rc);
        layout(h, rc.right - rc.left, rc.bottom - rc.top);

        let _ = ShowWindow(h, SW_SHOW);
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
        let _ = SetForegroundWindow(h);
    }
}

fn build_controls(h: HWND, inst: HINSTANCE, mode_b: bool) {
    unsafe {
        // ペイン1: プロファイルコンボ + 変数一覧 (位置・サイズは WM_SIZE で確定する)
        let combo = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("COMBOBOX"),
            None,
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST),
            0, 0, 100, 200,
            Some(h),
            Some(HMENU(IDC_PROFILE as usize as *mut c_void)),
            Some(inst),
            None,
        )
        .unwrap_or_default();

        let lv = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            w!("SysListView32"),
            None,
            WS_CHILD | WS_VISIBLE | WS_TABSTOP
                | WINDOW_STYLE(LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS),
            0, 0, 100, 100,
            Some(h),
            Some(HMENU(IDC_VARLIST as usize as *mut c_void)),
            Some(inst),
            None,
        )
        .unwrap_or_default();
        SendMessageW(
            lv,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            Some(WPARAM(0)),
            Some(LPARAM(LVS_EX_FULLROWSELECT as isize)),
        );

        // ペイン2: テンプレート編集
        let mk_label = |text: PCWSTR, id: i32| {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                text,
                WS_CHILD | WS_VISIBLE,
                0, 0, 100, LBL_H,
                Some(h),
                Some(HMENU(id as usize as *mut c_void)),
                Some(inst),
                None,
            )
            .unwrap_or_default()
        };
        let mk_edit = |id: i32| {
            CreateWindowExW(
                WS_EX_CLIENTEDGE,
                w!("EDIT"),
                None,
                WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP | WS_VSCROLL
                    | WINDOW_STYLE(ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN),
                0, 0, 100, 100,
                Some(h),
                Some(HMENU(id as usize as *mut c_void)),
                Some(inst),
                None,
            )
            .unwrap_or_default()
        };
        let mk_btn = |text: PCWSTR, id: i32| {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("BUTTON"),
                text,
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                0, 0, BTN_W, BTN_H,
                Some(h),
                Some(HMENU(id as usize as *mut c_void)),
                Some(inst),
                None,
            )
            .unwrap_or_default()
        };

        let lbl2 = mk_label(w!("テンプレート"), IDC_LBL_TEMPLATE);
        let tmpl = mk_edit(IDC_TEMPLATE);
        let save = mk_btn(w!("保存"), IDC_SAVE);

        let mut ctls = vec![combo, lv, lbl2, tmpl, save];

        // ペイン3: 送信内容プレビュー (モードBのみ)
        if mode_b {
            let lbl3 = mk_label(w!("送信内容"), IDC_LBL_PREVIEW);
            let prev = mk_edit(IDC_PREVIEW);
            let regen = mk_btn(w!("再生成"), IDC_REGEN);
            let submit = mk_btn(w!("送信"), IDC_SUBMIT);
            ctls.extend([lbl3, prev, regen, submit]);
        }

        let font = CreateFontW(
            -14, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, DEFAULT_CHARSET, FONT_OUTPUT_PRECISION(0),
            CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, DEFAULT_PITCH.0.into(), w!("Yu Gothic UI"),
        );
        for ctl in ctls {
            let _ = SendMessageW(ctl, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(0)));
        }
    }
}

/// プロファイルコンボへ選択肢を投入する
fn populate_profiles(h: HWND, active_idx: usize) {
    unsafe {
        let Ok(combo) = GetDlgItem(Some(h), IDC_PROFILE) else { return };
        let names: Vec<String> = state_mut(h)
            .map(|s| s.profiles.iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default();
        for name in &names {
            let wide = to_wide(name);
            SendMessageW(combo, CB_ADDSTRING, Some(WPARAM(0)), Some(LPARAM(wide.as_ptr() as isize)));
        }
        SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(active_idx)), Some(LPARAM(0)));
    }
}

/// 変数一覧 ListView へ列と10変数の行を投入する (§3.1)
fn populate_vars(h: HWND, mode_b: bool) {
    let lv = unsafe { GetDlgItem(Some(h), IDC_VARLIST).unwrap_or_default() };
    lv_add_col(lv, 0, "変数名", 120);
    lv_add_col(lv, 1, "意味", if mode_b { 150 } else { 180 });
    if mode_b {
        lv_add_col(lv, 2, "値", 150);
    }
    let cfg = Config::load();
    let ctx = unsafe { state_mut(h) }.and_then(|s| s.ctx.clone()).unwrap_or_default();
    for (i, (name, desc)) in VARS.iter().enumerate() {
        let mut cols = vec![format!("{{{{{name}}}}}"), desc.to_string()];
        if mode_b {
            cols.push(var_value(name, &cfg, &ctx));
        }
        lv_add_row(lv, i as i32, &cols);
    }
}

/// コンボで選択中のプロファイルindex
fn selected_index(h: HWND) -> usize {
    unsafe {
        let idx = GetDlgItem(Some(h), IDC_PROFILE)
            .map(|cb| SendMessageW(cb, CB_GETCURSEL, Some(WPARAM(0)), Some(LPARAM(0))).0)
            .unwrap_or(-1);
        if idx < 0 { 0 } else { idx as usize }
    }
}

/// レイアウト (§3): ペイン1固定幅、残りをペイン2(・3)で分割
fn layout(h: HWND, w: i32, ht: i32) {
    let mode_b = MODE_B.with(|m| *m.borrow());
    let pane1_w = if mode_b { PANE1_W_B } else { PANE1_W_A };
    let x2 = PAD + pane1_w + PAD;
    let avail = (w - x2 - PAD).max(100);
    let (pane2_w, pane3_x, pane3_w) = if mode_b {
        let p2 = (avail - PAD) / 2;
        (p2, x2 + p2 + PAD, avail - PAD - p2)
    } else {
        (avail, 0, 0)
    };
    let edit_top = PAD + LBL_H + 4;
    let btn_y = ht - BTN_H - PAD;
    let edit_h = (btn_y - PAD / 2 - edit_top).max(50);

    let place = |id: i32, x: i32, y: i32, cw: i32, ch: i32| unsafe {
        if let Ok(ctl) = GetDlgItem(Some(h), id) {
            let _ = SetWindowPos(ctl, None, x, y, cw, ch, SWP_NOZORDER);
        }
    };
    // ペイン1
    place(IDC_PROFILE, PAD, PAD, pane1_w, 200);
    let lv_top = PAD + COMBO_H + 6;
    place(IDC_VARLIST, PAD, lv_top, pane1_w, (ht - lv_top - PAD).max(50));
    // ペイン2
    place(IDC_LBL_TEMPLATE, x2, PAD, pane2_w, LBL_H);
    place(IDC_TEMPLATE, x2, edit_top, pane2_w, edit_h);
    place(IDC_SAVE, x2 + pane2_w - BTN_W, btn_y, BTN_W, BTN_H);
    // ペイン3 (モードBのみ)
    if mode_b {
        place(IDC_LBL_PREVIEW, pane3_x, PAD, pane3_w, LBL_H);
        place(IDC_PREVIEW, pane3_x, edit_top, pane3_w, edit_h);
        place(IDC_REGEN, pane3_x + pane3_w - BTN_W * 2 - 8, btn_y, BTN_W, BTN_H);
        place(IDC_SUBMIT, pane3_x + pane3_w - BTN_W, btn_y, BTN_W, BTN_H);
    }
}

/// 保存ボタン (§4.3)
fn handle_save(h: HWND) {
    let idx = unsafe { state_mut(h) }.map(|s| s.cur_idx).unwrap_or(0);
    let tmpl = get_multiline_text(h, IDC_TEMPLATE);
    let Some(name) = unsafe { state_mut(h) }.and_then(|s| s.profiles.get(idx).map(|p| p.name.clone()))
    else {
        return;
    };
    let ok = unsafe { state_mut(h) }.map(|s| (s.on_save)(&name, &tmpl)).unwrap_or(false);
    if ok {
        if let Some(s) = unsafe { state_mut(h) } {
            if let Some(p) = s.profiles.get_mut(idx) {
                p.template = tmpl;
            }
            s.tmpl_dirty = false;
        }
    } else {
        unsafe {
            MessageBoxW(
                Some(h),
                w!("該当プロファイルが見つからないため保存できませんでした。\n設定画面で削除された可能性があります。"),
                w!("Focus Translator"),
                MB_OK,
            );
        }
    }
}

/// 再生成ボタン (§4.4): プレビューが手編集されている場合のみ破棄確認を出す
fn handle_regen(h: HWND) {
    let dirty = unsafe { state_mut(h) }.map(|s| s.preview_dirty).unwrap_or(false);
    if dirty {
        let r = unsafe {
            MessageBoxW(
                Some(h),
                w!("送信内容への編集は破棄されます。テンプレートから再生成しますか?"),
                w!("Focus Translator"),
                MB_YESNO | MB_ICONQUESTION,
            )
        };
        if r != IDYES {
            return;
        }
    }
    regenerate_preview(h);
}

/// 送信ボタン (§4.5): ペイン3の現在の内容を on_submit へ渡して閉じる。空なら何もしない。
fn handle_submit(h: HWND) {
    let text = get_multiline_text(h, IDC_PREVIEW);
    if text.is_empty() {
        return;
    }
    let Some(s) = (unsafe { state_mut(h) }) else { return };
    let Some(cb) = s.on_submit.take() else { return };
    let profile = s.profiles.get(s.cur_idx).map(|p| p.name.clone()).unwrap_or_default();
    cb(text, profile);
    // 送信後は未保存テンプレートがあっても確認せず閉じる (送信が主目的のため)
    if let Some(s) = unsafe { state_mut(h) } {
        s.tmpl_dirty = false;
    }
    unsafe {
        let _ = DestroyWindow(h);
    }
}

/// プロファイル切替 (§4.2): 未保存確認の上でテンプレートを読込み直し、モードBは再生成する
fn handle_profile_change(h: HWND) {
    let new_idx = selected_index(h);
    let cur_idx = unsafe { state_mut(h) }.map(|s| s.cur_idx).unwrap_or(0);
    if new_idx == cur_idx {
        return;
    }
    if !confirm_discard_template(h) {
        // キャンセル: コンボの選択を元に戻す
        unsafe {
            if let Ok(cb) = GetDlgItem(Some(h), IDC_PROFILE) {
                SendMessageW(cb, CB_SETCURSEL, Some(WPARAM(cur_idx)), Some(LPARAM(0)));
            }
        }
        return;
    }
    load_template(h, new_idx);
    // 切替は明示操作のためプレビューの破棄警告は出さず自動再生成する (§4.2)
    if MODE_B.with(|m| *m.borrow()) {
        regenerate_preview(h);
    }
}

/// 変数一覧のダブルクリック (§4.1): ペイン2のカーソル位置に {{変数名}} を挿入する
fn handle_var_dblclk(h: HWND, item: i32) {
    if item < 0 || item as usize >= VARS.len() {
        return;
    }
    let text = format!("{{{{{}}}}}", VARS[item as usize].0);
    unsafe {
        if let Ok(edit) = GetDlgItem(Some(h), IDC_TEMPLATE) {
            let wide = to_wide(&text);
            // EM_REPLACESEL は EN_CHANGE を発火するのでダーティ管理は共通処理に任せる
            SendMessageW(edit, EM_REPLACESEL, Some(WPARAM(1)), Some(LPARAM(wide.as_ptr() as isize)));
            let _ = SetFocus(Some(edit));
        }
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let notif = ((wparam.0 >> 16) & 0xFFFF) as u32;
            match id {
                IDC_SAVE => handle_save(h),
                IDC_REGEN => handle_regen(h),
                IDC_SUBMIT => handle_submit(h),
                IDC_PROFILE if notif == CBN_SELCHANGE => handle_profile_change(h),
                IDC_TEMPLATE | IDC_PREVIEW if notif == EN_CHANGE => {
                    // ダーティ管理 (§4.6)。プログラムからの書込み中は除外する。
                    if let Some(s) = unsafe { state_mut(h) }
                        && !s.suppress_change
                    {
                        if id == IDC_TEMPLATE {
                            s.tmpl_dirty = true;
                        } else {
                            s.preview_dirty = true;
                        }
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_NOTIFY => {
            let nmhdr = unsafe { &*(lparam.0 as *const NMHDR) };
            if nmhdr.idFrom as i32 == IDC_VARLIST && nmhdr.code == NM_DBLCLK {
                let nmia = unsafe { &*(lparam.0 as *const NMITEMACTIVATE) };
                handle_var_dblclk(h, nmia.iItem);
            }
            LRESULT(0)
        }
        WM_SIZE => {
            let w = (lparam.0 & 0xFFFF) as i32;
            let ht = ((lparam.0 >> 16) & 0xFFFF) as i32;
            layout(h, w, ht);
            LRESULT(0)
        }
        WM_CLOSE => {
            // 未保存テンプレートの破棄確認 (§4.6)。No なら閉じない。
            if confirm_discard_template(h) {
                unsafe {
                    let _ = DestroyWindow(h);
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                // 未送信で閉じられた場合もここで State (コールバック含む) を解放する
                let ptr = GetWindowLongPtrW(h, GWLP_USERDATA);
                if ptr != 0 {
                    SetWindowLongPtrW(h, GWLP_USERDATA, 0);
                    let _ = Box::from_raw(ptr as *mut State);
                }
            }
            WND.with(|w| *w.borrow_mut() = 0);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(h, msg, wparam, lparam) },
    }
}
