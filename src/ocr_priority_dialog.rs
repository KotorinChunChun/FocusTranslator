// UIA優先度制御・実行しないアプリの管理ダイアログ (SPECv0.5.5 §2)
//
// 2つの独立した一覧を管理する:
// - OCR優先アプリリスト (`config.ocr_priority_apps`): TeamViewer/RDP/VMware等、リモート画面・
//   仮想環境を表示するアプリでは、UIAで取得できるのは自アプリ(操作ウィンドウ)の要素であって
//   実際に画面に映っているリモート/仮想環境側の内容ではないため、登録したexeでは常にOCR
//   (画面キャプチャの文字認識)を優先する (worker::recognize_cycle の force_ocr 判定)。
// - 実行しないアプリリスト (`config.disabled_apps`): 登録したexe上ではホットキーを押しても
//   一切認識・オーバーレイ表示を行わない (worker::recognize_cycle 冒頭の早期return判定)。
//
// 追加方法は共通化されている: 起動中のアプリ一覧から選ぶか、手入力欄に直接入力するか、
// ポインタで対象ウィンドウをクリックする(結果は手入力欄に転記されるだけ)。いずれの方法でも
// 最終的に手入力欄の内容を経由し、【OCR優先アプリリストに追加】/【実行しないアプリリストに
// 追加】のどちらかのボタンを押して初めて確定する。
// ポインタ指定は input_dialog.rs 等と同じ「専用ウィンドウクラス+モーダルメッセージループ」の
// 中で WM_TIMER を使い、GetAsyncKeyState(VK_LBUTTON) をポーリングして次のクリックを検出する
// (FocusTranslator本体のホットキー検出と同じ手法。グローバルフックは使わない)。
use crate::config::Config;
use crate::ui_helpers::*;
use std::cell::RefCell;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::COLOR_BTNFACE;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    CBN_SELCHANGE, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    EnumChildWindows, EnumWindows, GA_ROOT, GW_OWNER, GWL_EXSTYLE, GetAncestor, GetCursorPos,
    GetMessageW, GetWindow, GetWindowLongW, GetWindowRect, IDC_ARROW, IsWindow, IsWindowVisible,
    KillTimer, LoadCursorW, MSG, RegisterClassW, SW_SHOW, SetForegroundWindow, SetTimer,
    ShowWindow, TranslateMessage, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_TIMER, WNDCLASSW,
    WS_CAPTION, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUPWINDOW, WS_SYSMENU, WindowFromPoint,
};
use windows::core::w;

const IDC_LIST_OCR: i32 = 201;
const IDC_REMOVE_OCR: i32 = 202;
const IDC_RUNNING: i32 = 203;
const IDC_REFRESH_RUNNING: i32 = 204;
const IDC_MANUAL: i32 = 206;
const IDC_ADD_TO_OCR: i32 = 207;
const IDC_POINT: i32 = 208;
const IDC_CLOSE: i32 = 209;
const IDC_LIST_DISABLED: i32 = 210;
const IDC_REMOVE_DISABLED: i32 = 211;
const IDC_ADD_TO_DISABLED: i32 = 212;
const IDC_MOVE_TO_DISABLED: i32 = 213;
const IDC_MOVE_TO_OCR: i32 = 214;

const DLG_W: i32 = 820;
const DLG_H: i32 = 454;
const MARGIN_L: i32 = 12;
const MARGIN_R: i32 = 40;
const TIMER_POINT_POLL: usize = 1;
const POINT_BTN_IDLE: &str = "ポインタで指定";
const POINT_BTN_WAITING: &str = "クリック待ち…(もう一度押して中止)";
/// 起動中のアプリ一覧に表示するウィンドウタイトルの先頭何文字まで表示するか
const RUNNING_TITLE_CHARS: usize = 20;

/// どちらの一覧を対象にした操作か
#[derive(Clone, Copy, PartialEq)]
enum AppList {
    /// OCR優先アプリリスト (config.ocr_priority_apps)
    OcrPriority,
    /// 実行しないアプリリスト (config.disabled_apps)
    Disabled,
}

impl AppList {
    fn listbox_id(self) -> i32 {
        match self {
            AppList::OcrPriority => IDC_LIST_OCR,
            AppList::Disabled => IDC_LIST_DISABLED,
        }
    }
}

thread_local! {
    static WND: RefCell<isize> = const { RefCell::new(0) };
    /// ポインタ指定モード中か。true の間、WM_TIMERでクリックを監視する。
    static WAITING_FOR_CLICK: RefCell<bool> = const { RefCell::new(false) };
    /// 開始直後の「ポインタで指定」ボタン自身のクリック(マウスアップ)を誤検出しないための
    /// デバウンス。ボタンが一度「離された」ことを確認してから、次の「押された」を本物とみなす。
    static SAW_BUTTON_UP: RefCell<bool> = const { RefCell::new(false) };
    /// IDC_RUNNING コンボの表示行(index)と実exe名の対応。表示はタイトル+exe名のダブル表記
    /// のため、選択されたexe名を文字列パースに頼らずindexで引けるよう保持しておく。
    static RUNNING_EXES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

pub fn is_open() -> bool {
    let h = hwnd();
    !h.is_invalid() && unsafe { IsWindow(Some(h)).as_bool() }
}

pub fn hwnd() -> HWND {
    HWND(WND.with(|w| *w.borrow()) as *mut _)
}

fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 「タイトル先頭20文字 — exe名」形式の表示文字列を組み立てる。タイトルが空(フルスクリーン
/// のリモートデスクトップ等、タイトルバー自体が無いウィンドウ)ならexe名だけを表示する。
fn format_running_entry(title: &str, exe: &str) -> String {
    let t = truncate_chars(title.trim(), RUNNING_TITLE_CHARS);
    if t.is_empty() {
        exe.to_string()
    } else {
        format!("{t}  —  {exe}")
    }
}

/// 実行中の可視トップレベルウィンドウから (タイトル, exeファイル名) の一覧を集める。
/// ツールウィンドウ(タスクバーに出ない補助窓)・オーナー付きの子ウィンドウ(ダイアログ等)・
/// 極端に小さいウィンドウは除外する。タイトルの有無では絞り込まない — フルスクリーンの
/// リモートデスクトップ接続等はタイトルバーが無くタイトル文字列が空になるため、ここで
/// 弾くと一覧に出てこなくなる(実機で報告された不具合)。自分自身も除外する。
/// 既にどちらかの一覧に登録済みのexeも、選ぶ意味が無いため除外する。
fn enumerate_running_apps() -> Vec<(String, String)> {
    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() {
                return windows::core::BOOL(1);
            }
            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
                return windows::core::BOOL(1);
            }
            if let Ok(owner) = GetWindow(hwnd, GW_OWNER)
                && !owner.is_invalid()
            {
                return windows::core::BOOL(1);
            }
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_ok() {
                let w = rect.right - rect.left;
                let h = rect.bottom - rect.top;
                if w < 40 || h < 40 {
                    return windows::core::BOOL(1);
                }
            }
            let (exe, title) = crate::util::get_window_context(hwnd);
            if let Some(exe) = exe
                && !exe.eq_ignore_ascii_case("focus-translator.exe")
            {
                let list = &mut *(lparam.0 as *mut Vec<(String, String)>);
                if !list
                    .iter()
                    .any(|(_, e): &(String, String)| e.eq_ignore_ascii_case(&exe))
                {
                    list.push((title.unwrap_or_default(), exe));
                }
            }
        }
        windows::core::BOOL(1)
    }
    let mut list: Vec<(String, String)> = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(enum_proc),
            LPARAM(&mut list as *mut Vec<(String, String)> as isize),
        );
    }
    let cfg = Config::load();
    list.retain(|(_, exe)| {
        !cfg.ocr_priority_apps
            .iter()
            .any(|e| e.eq_ignore_ascii_case(exe))
            && !cfg
                .disabled_apps
                .iter()
                .any(|e| e.eq_ignore_ascii_case(exe))
    });
    list.sort_by(|a, b| a.1.to_ascii_lowercase().cmp(&b.1.to_ascii_lowercase()));
    list
}

/// 大文字小文字違いの重複を取り除く。取り除いた場合のみ true を返す。
fn dedup_case_insensitive(list: &mut Vec<String>) -> bool {
    let before = list.len();
    let mut seen: Vec<String> = Vec::new();
    list.retain(|e| {
        let dup = seen.iter().any(|s| s.eq_ignore_ascii_case(e));
        if !dup {
            seen.push(e.clone());
        }
        !dup
    });
    list.len() != before
}

/// 両方の一覧をリストボックスへ反映する。ついでに大文字小文字違いの重複が紛れ込んで
/// いれば取り除いて保存し直す(実機報告: 多重登録が発生していたための保険的なクリーンアップ)。
fn refresh_lists(h: HWND) {
    let mut cfg = Config::load();
    let changed_ocr = dedup_case_insensitive(&mut cfg.ocr_priority_apps);
    let changed_dis = dedup_case_insensitive(&mut cfg.disabled_apps);
    if changed_ocr || changed_dis {
        cfg.save();
    }
    let lb1 = get_dlg_item(h, IDC_LIST_OCR);
    listbox_reset(lb1);
    for exe in &cfg.ocr_priority_apps {
        listbox_add_item(lb1, exe);
    }
    let lb2 = get_dlg_item(h, IDC_LIST_DISABLED);
    listbox_reset(lb2);
    for exe in &cfg.disabled_apps {
        listbox_add_item(lb2, exe);
    }
}

fn refresh_running(h: HWND) {
    let apps = enumerate_running_apps();
    let display: Vec<String> = apps
        .iter()
        .map(|(t, e)| format_running_entry(t, e))
        .collect();
    let display_refs: Vec<&str> = display.iter().map(|s| s.as_str()).collect();
    // combo_fillは追記のみで既存項目をクリアしないため、都度リセットしないと表示上の項目数と
    // RUNNING_EXES(常に総入れ替え)の対応がズレていく(実機報告の不具合)。
    combo_reset(h, IDC_RUNNING);
    combo_fill(h, IDC_RUNNING, &display_refs, 0);
    RUNNING_EXES.with(|r| *r.borrow_mut() = apps.into_iter().map(|(_, e)| e).collect());
}

/// 手入力欄へ転記されたexe名に対応する項目が「起動中のアプリ」コンボにあれば選択状態を
/// 合わせる(見た目上、どちらの操作で入力したかに関わらず選択内容が一致するようにする)。
fn sync_running_combo_selection(h: HWND, exe: &str) {
    let idx = RUNNING_EXES.with(|r| r.borrow().iter().position(|e| e.eq_ignore_ascii_case(exe)));
    if let Some(idx) = idx {
        combo_select(h, IDC_RUNNING, idx);
    }
}

/// 追加操作の直後、「起動中のアプリ」コンボの選択位置を追加前と同じインデックスへ戻し、
/// 手入力欄もその項目のexe名に合わせる。exe名ではなく位置基準で戻すのは、追加した
/// exeは一覧から除外されて後続の項目が繰り上がるため、同じ位置には(ソート順で)次の
/// 項目が来ることを利用し、複数のアプリを連続登録しやすくするため。
fn reselect_running_after_add(h: HWND, prev_idx: usize) {
    let len = RUNNING_EXES.with(|r| r.borrow().len());
    if len == 0 {
        set_ctl_text(h, IDC_MANUAL, "");
        return;
    }
    let idx = prev_idx.min(len - 1);
    combo_select(h, IDC_RUNNING, idx);
    if let Some(exe) = RUNNING_EXES.with(|r| r.borrow().get(idx).cloned()) {
        set_ctl_text(h, IDC_MANUAL, &exe);
    }
}

/// 指定の一覧へexeを追加して即保存する。既に登録済み(前後の空白・大文字小文字違いを無視)
/// なら何もしない (実機報告の多重登録対策)。
fn add_app(h: HWND, which: AppList, exe: &str) {
    let exe = exe.trim();
    if exe.is_empty() {
        return;
    }
    let mut cfg = Config::load();
    let list = match which {
        AppList::OcrPriority => &mut cfg.ocr_priority_apps,
        AppList::Disabled => &mut cfg.disabled_apps,
    };
    if list.iter().any(|e| e.trim().eq_ignore_ascii_case(exe)) {
        return;
    }
    list.push(exe.to_string());
    cfg.save();
    refresh_lists(h);
    refresh_running(h); // 登録により起動中一覧から除外されるため更新する
    notify_main_reload();
}

fn remove_selected(h: HWND, which: AppList) {
    let Some(sel) = listbox_get_sel(get_dlg_item(h, which.listbox_id())) else {
        return;
    };
    let mut cfg = Config::load();
    let list = match which {
        AppList::OcrPriority => &mut cfg.ocr_priority_apps,
        AppList::Disabled => &mut cfg.disabled_apps,
    };
    if sel >= list.len() {
        return;
    }
    list.remove(sel);
    let new_len = list.len();
    cfg.save();
    refresh_lists(h);
    refresh_running(h);
    notify_main_reload();
    // 削除後も同じインデックスの行を選択状態にしておく(末尾を削除した場合は新しい末尾へ)
    if new_len > 0 {
        listbox_set_sel(get_dlg_item(h, which.listbox_id()), sel.min(new_len - 1));
    }
}

/// `src` の選択行を `dst` の末尾へ移す。移動先に大文字小文字違いを含む同名項目が
/// 既にある場合は重複追加せず、既存行のindexを返す。
fn move_list_item(src: &mut Vec<String>, dst: &mut Vec<String>, sel: usize) -> Option<usize> {
    if sel >= src.len() {
        return None;
    }
    let item = src.remove(sel);
    if let Some(existing) = dst.iter().position(|e| e.eq_ignore_ascii_case(&item)) {
        Some(existing)
    } else {
        dst.push(item);
        Some(dst.len() - 1)
    }
}

/// 選択中のアプリを反対側の一覧へ移して即保存する。
fn move_selected(h: HWND, from: AppList) {
    let Some(sel) = listbox_get_sel(get_dlg_item(h, from.listbox_id())) else {
        return;
    };
    let to = match from {
        AppList::OcrPriority => AppList::Disabled,
        AppList::Disabled => AppList::OcrPriority,
    };
    let mut cfg = Config::load();
    let moved_to = match from {
        AppList::OcrPriority => {
            move_list_item(&mut cfg.ocr_priority_apps, &mut cfg.disabled_apps, sel)
        }
        AppList::Disabled => {
            move_list_item(&mut cfg.disabled_apps, &mut cfg.ocr_priority_apps, sel)
        }
    };
    let Some(dest_sel) = moved_to else {
        return;
    };
    cfg.save();
    refresh_lists(h);
    refresh_running(h);
    notify_main_reload();
    listbox_set_sel(get_dlg_item(h, to.listbox_id()), dest_sel);
}

/// 設定変更をメインスレッドへ通知する(実行中のホールドサイクルへ即座に反映するため。
/// settings.rs の auto_save と同じ通知経路)。
fn notify_main_reload() {
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            Some(crate::app_state::main_hwnd()),
            crate::app_state::WM_APP_CFG,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

pub fn open(parent: HWND) {
    if is_open() {
        unsafe {
            let _ = SetForegroundWindow(hwnd());
        }
        return;
    }
    unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class_name = w!("FocusTranslatorOcrPriorityClass");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(
                (COLOR_BTNFACE.0 + 1) as usize as *mut _,
            ),
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);

        let (x, y) = {
            let mut r = RECT::default();
            if GetWindowRect(parent, &mut r).is_ok() {
                (r.left + 30, r.top + 30)
            } else {
                (150, 150)
            }
        };

        let Ok(win) = CreateWindowExW(
            WS_EX_TOPMOST,
            class_name,
            w!("アプリ別の動作設定 (OCR優先 / 実行しない)"),
            WS_POPUPWINDOW | WS_CAPTION | WS_SYSMENU,
            x,
            y,
            DLG_W,
            DLG_H,
            Some(parent),
            None,
            Some(instance.into()),
            None,
        ) else {
            return;
        };
        WND.with(|w| *w.borrow_mut() = win.0 as isize);
        WAITING_FOR_CLICK.with(|f| *f.borrow_mut() = false);

        let inst = instance.into();
        let content_w = DLG_W - MARGIN_L - MARGIN_R;
        let mut ly = 12;

        // 起動中のアプリを選ぶと、下の手入力欄へexe名が転記される(確定はしない)。
        // 右側の「更新」「ポインタで指定」ボタンは幅を揃え、コンボ/手入力欄も同じ幅で並ぶ
        // 一体の入力ブロックに見えるようにする。
        let top_btn_w = 200;
        let top_btn_x = MARGIN_L + content_w - top_btn_w;
        let top_field_w = top_btn_x - 134 - 8;
        label(win, inst, "起動中のアプリ", MARGIN_L, ly, 110);
        // 既定の200px(9件程度で頭打ち)だと項目数が多い環境でスクロールが必須になるため、
        // ドロップダウンの最大高さを広げてほぼ全件を一度に表示できるようにする。
        combo_h(win, inst, 134, ly - 2, top_field_w, 600, IDC_RUNNING);
        button(
            win,
            inst,
            "更新",
            top_btn_x,
            ly - 2,
            top_btn_w,
            IDC_REFRESH_RUNNING,
        );
        ly += 34;

        // 手入力(exe名): 起動中のアプリ選択・「ポインタで指定」の結果もここへ転記される
        label(win, inst, "手入力 (exe名)", MARGIN_L, ly, 110);
        edit(win, inst, 134, ly - 2, top_field_w, IDC_MANUAL);
        button(
            win,
            inst,
            POINT_BTN_IDLE,
            top_btn_x,
            ly - 2,
            top_btn_w,
            IDC_POINT,
        );
        ly += 40;

        // 2つの一覧を左右に並べる。追加・削除ボタンは各リストボックスの右上に配置し、
        // 「↓追加」(手入力欄の内容がリストへ落ちる)・「↑削除」(選択行がリストから抜ける)
        // の矢印表記で情報の移動方向を示す。
        let move_col_w = 78;
        let col_gap = 10;
        let col_w = (content_w - move_col_w - col_gap * 2) / 2;
        let col1_x = MARGIN_L;
        let move_x = col1_x + col_w + col_gap;
        let col2_x = move_x + move_col_w + col_gap;
        let btn_w = 70;
        let btn_gap = 4;
        let add1_x = col1_x + col_w - (btn_w * 2 + btn_gap);
        let remove1_x = add1_x + btn_w + btn_gap;
        let add2_x = col2_x + col_w - (btn_w * 2 + btn_gap);
        let remove2_x = add2_x + btn_w + btn_gap;
        label(
            win,
            inst,
            "OCR優先アプリリスト",
            col1_x,
            ly,
            add1_x - col1_x - 8,
        );
        label(
            win,
            inst,
            "実行しないアプリリスト",
            col2_x,
            ly,
            add2_x - col2_x - 8,
        );
        button(win, inst, "↓追加", add1_x, ly - 2, btn_w, IDC_ADD_TO_OCR);
        button(win, inst, "↑削除", remove1_x, ly - 2, btn_w, IDC_REMOVE_OCR);
        button(
            win,
            inst,
            "↓追加",
            add2_x,
            ly - 2,
            btn_w,
            IDC_ADD_TO_DISABLED,
        );
        button(
            win,
            inst,
            "↑削除",
            remove2_x,
            ly - 2,
            btn_w,
            IDC_REMOVE_DISABLED,
        );
        ly += 30;

        let list_h = 240;
        listbox(win, inst, col1_x, ly, col_w, list_h, IDC_LIST_OCR);
        listbox(win, inst, col2_x, ly, col_w, list_h, IDC_LIST_DISABLED);
        button(
            win,
            inst,
            "→ 移動",
            move_x,
            ly + 78,
            move_col_w,
            IDC_MOVE_TO_DISABLED,
        );
        button(
            win,
            inst,
            "← 移動",
            move_x,
            ly + 118,
            move_col_w,
            IDC_MOVE_TO_OCR,
        );
        ly += list_h + 16;

        button(
            win,
            inst,
            "閉じる",
            MARGIN_L + content_w - 80,
            ly,
            80,
            IDC_CLOSE,
        );

        let font = make_font(14, false);
        let _ = EnumChildWindows(Some(win), Some(set_font_proc), LPARAM(font.0 as isize));

        refresh_lists(win);
        refresh_running(win);

        let _ = ShowWindow(win, SW_SHOW);
        let _ = SetForegroundWindow(win);

        let mut msg = MSG::default();
        while IsWindow(Some(win)).as_bool() && GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_DESTROY && msg.hwnd == win {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = KillTimer(Some(win), TIMER_POINT_POLL);
        WND.with(|w| *w.borrow_mut() = 0);
        WAITING_FOR_CLICK.with(|f| *f.borrow_mut() = false);
        let _ = SetForegroundWindow(parent);
    }
}

fn start_waiting_for_click(h: HWND) {
    WAITING_FOR_CLICK.with(|f| *f.borrow_mut() = true);
    SAW_BUTTON_UP.with(|f| *f.borrow_mut() = false);
    set_ctl_text(h, IDC_POINT, POINT_BTN_WAITING);
    unsafe {
        SetTimer(Some(h), TIMER_POINT_POLL, 50, None);
    }
}

fn stop_waiting_for_click(h: HWND) {
    WAITING_FOR_CLICK.with(|f| *f.borrow_mut() = false);
    set_ctl_text(h, IDC_POINT, POINT_BTN_IDLE);
    unsafe {
        let _ = KillTimer(Some(h), TIMER_POINT_POLL);
    }
}

/// クリック監視のポーリング(50msごと)。ダイアログ自身へのクリックは無視して待機を継続する。
/// 検出できたら手入力欄にexe名を入れるだけで、登録の確定はユーザーがどちらかの【追加】
/// ボタンを押すまで行わない。
fn poll_for_click(h: HWND) {
    let button_down = unsafe { (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0 };
    let saw_up = SAW_BUTTON_UP.with(|f| *f.borrow());
    if !saw_up {
        if !button_down {
            SAW_BUTTON_UP.with(|f| *f.borrow_mut() = true);
        }
        return;
    }
    if !button_down {
        return;
    }
    // ここに来た時点で「離された後に新しく押された」= 本物のクリック
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    let hit = unsafe { WindowFromPoint(pt) };
    let root = unsafe { GetAncestor(hit, GA_ROOT) };
    if root == h {
        // 自分自身(このダイアログ)上のクリックは無視して待機を続ける
        SAW_BUTTON_UP.with(|f| *f.borrow_mut() = false);
        return;
    }
    let (exe, _title) = crate::util::get_window_context(root);
    stop_waiting_for_click(h);
    match exe {
        Some(exe) => {
            set_ctl_text(h, IDC_MANUAL, &exe);
            sync_running_combo_selection(h, &exe);
        }
        None => unsafe {
            windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                Some(h),
                w!("対象ウィンドウの実行ファイル名を取得できませんでした。"),
                w!("ポインタで指定"),
                windows::Win32::UI::WindowsAndMessaging::MB_OK,
            );
        },
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let notif = ((wparam.0 >> 16) & 0xFFFF) as u32;
            match id {
                IDC_REMOVE_OCR => remove_selected(h, AppList::OcrPriority),
                IDC_REMOVE_DISABLED => remove_selected(h, AppList::Disabled),
                IDC_MOVE_TO_DISABLED => move_selected(h, AppList::OcrPriority),
                IDC_MOVE_TO_OCR => move_selected(h, AppList::Disabled),
                IDC_REFRESH_RUNNING => refresh_running(h),
                // 起動中のアプリを選んだら、手入力欄へ転記するだけ(共通の追加フローに乗せる)
                IDC_RUNNING if notif == CBN_SELCHANGE => {
                    let idx = combo_get_sel(get_dlg_item(h, IDC_RUNNING));
                    if let Some(exe) = RUNNING_EXES.with(|r| r.borrow().get(idx).cloned()) {
                        set_ctl_text(h, IDC_MANUAL, &exe);
                    }
                }
                IDC_ADD_TO_OCR => {
                    let exe = get_ctl_text(h, IDC_MANUAL);
                    let prev_idx = combo_get_sel(get_dlg_item(h, IDC_RUNNING));
                    add_app(h, AppList::OcrPriority, &exe);
                    reselect_running_after_add(h, prev_idx);
                }
                IDC_ADD_TO_DISABLED => {
                    let exe = get_ctl_text(h, IDC_MANUAL);
                    let prev_idx = combo_get_sel(get_dlg_item(h, IDC_RUNNING));
                    add_app(h, AppList::Disabled, &exe);
                    reselect_running_after_add(h, prev_idx);
                }
                IDC_POINT => {
                    if WAITING_FOR_CLICK.with(|f| *f.borrow()) {
                        stop_waiting_for_click(h);
                    } else {
                        start_waiting_for_click(h);
                    }
                }
                IDC_CLOSE => unsafe {
                    let _ = DestroyWindow(h);
                },
                _ => {}
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_POINT_POLL {
                poll_for_click(h);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(h);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(h, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::move_list_item;

    #[test]
    fn move_list_item_は選択行を移動先末尾へ移す() {
        let mut src = vec!["a.exe".into(), "b.exe".into()];
        let mut dst = vec!["c.exe".into()];

        assert_eq!(move_list_item(&mut src, &mut dst, 0), Some(1));
        assert_eq!(src, ["b.exe"]);
        assert_eq!(dst, ["c.exe", "a.exe"]);
    }

    #[test]
    fn move_list_item_は移動先の同名項目を重複させない() {
        let mut src = vec!["APP.exe".into()];
        let mut dst = vec!["app.exe".into()];

        assert_eq!(move_list_item(&mut src, &mut dst, 0), Some(0));
        assert!(src.is_empty());
        assert_eq!(dst, ["app.exe"]);
    }
}
