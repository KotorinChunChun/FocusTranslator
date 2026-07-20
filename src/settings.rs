// 設定画面 (SPEC §12)
use crate::config::Config;
use crate::util::{self, to_wide};
use crate::ui_helpers::*;
use std::cell::RefCell;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, HFONT,
};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW,
};
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
    DestroyWindow, GetSystemMetrics,
    IDC_ARROW, IsWindow, LoadCursorW, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK,
    MB_YESNO, MessageBoxW, PostMessageW, RegisterClassW, SM_CYSCREEN, SW_SHOW, SW_SHOWNORMAL, SetForegroundWindow, ShowWindow, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND,
    WM_DESTROY, WNDCLASSW, WS_CAPTION, WS_EX_TOPMOST, WS_SYSMENU,
};
use windows::core::{PCWSTR, w};

const IDC_HOLDKEY: i32 = 101;
const IDC_POLL: i32 = 102;
const IDC_HOTKEY: i32 = 103;
const IDC_OCR: i32 = 104;
const IDC_TR: i32 = 105;
const IDC_LANG: i32 = 106;
const IDC_DEEPL: i32 = 107;
const IDC_GOOGLE: i32 = 108;
const IDC_PROF_LIST: i32 = 109;
const IDC_PROF_NEW: i32 = 110;
const IDC_AUTOSTART: i32 = 113;
const IDC_PERFLOG: i32 = 114;
const IDC_CONSENT_RESET: i32 = 115;
const IDC_CLOSE: i32 = 118;
const IDC_PADDLE_STATUS: i32 = 121;
const IDC_PADDLE_INSTALL: i32 = 122;
const IDC_ONNX_STATUS: i32 = 123;
const IDC_ONNX_INSTALL: i32 = 124;
const IDC_DEEPL_URL: i32 = 125;
const IDC_GOOGLE_URL: i32 = 126;
const IDC_PROF_SAVE: i32 = 127;
const IDC_SRCLANG: i32 = 128;
const IDC_LOG_ENABLED: i32 = 129;
const IDC_DEBUG_MODE: i32 = 130;
const IDC_LOG_MAX: i32 = 131;
const IDC_PROF_SAVEAS: i32 = 132;
const IDC_PROF_DEL: i32 = 133;
const IDC_PROF_NAME: i32 = 134;
const IDC_OPEN_LOG: i32 = 135;
const IDC_PROF_MODEL: i32 = 137;
const IDC_PROF_URL: i32 = 138;
const IDC_PROF_KEY: i32 = 139;
const IDC_PROF_TYPE: i32 = 140;
const IDC_DETECT_MODE: i32 = 145;
const IDC_DETECT_KEY: i32 = 146;
const IDC_PREVIEW_DETECT_MODE: i32 = 147;
const IDC_PIN_HOLD: i32 = 151;
/// プロンプト編集ウィンドウを開くボタン (SPECv0.4.7 §6.1)
const IDC_PROMPT_TR_BTN: i32 = 152;
const IDC_PROMPT_OCR_BTN: i32 = 153;
const IDC_PROMPT_EXP_BTN: i32 = 154;
const IDC_OCR_EXP: i32 = 155;
const IDC_TR_EXP: i32 = 156;
/// 選択中のプロファイルを既定LLMプロファイルにするボタン (OCR/翻訳/解説すべてで共用するため
/// LLMプロファイル設定グループ側に配置する)
const IDC_PROF_SET_DEFAULT: i32 = 157;
const IDC_RESET_SETTINGS: i32 = 158;
/// オーバーレイの配色テーマ (Windows既定/ライト/ダーク)
const IDC_OVERLAY_THEME: i32 = 159;
/// OneOCRが現PCで使用可能かの判定結果表示
const IDC_ONEOCR_STATUS: i32 = 160;
/// ローカルLLM (llama.cpp) 関連コントロール (SPECv0.5.2追補)
const IDC_LLAMA_BIN_STATUS: i32 = 161;
const IDC_LLAMA_BIN_INSTALL: i32 = 162;
const IDC_LLAMA_MODEL_STATUS: i32 = 163;
/// モデルの自動ダウンロード(既定の管理下ディレクトリへ導入)
const IDC_LLAMA_MODEL_INSTALL: i32 = 164;
const IDC_LLAMA_PORT: i32 = 165;
const IDC_LLAMA_TOGGLE: i32 = 166;
const IDC_LLAMA_SERVER_STATUS: i32 = 167;
const IDC_LLAMA_AUTOSTART: i32 = 168;
/// 既存GGUFファイルのパス(LM Studio等で導入済みのモデルを再利用する場合に指定)
const IDC_LLAMA_MODEL_PATH: i32 = 169;
/// ファイル選択ダイアログでモデルファイルを選ぶボタン
const IDC_LLAMA_MODEL_BROWSE: i32 = 170;
/// 左下欄外のバージョン情報表示 (SPECv0.5.2追補)
const IDC_VERSION_INFO: i32 = 171;
const IDC_GITHUB_LINK: i32 = 172;
/// mmproj (画像入力/VLM対応) 関連コントロール (SPECv0.5.2追補)。ローカルLLMサーバーは
/// 1プロセスで、mmprojファイルが指定されていれば同一ポートのまま画像入力にも対応する。
const IDC_LLAMA_MMPROJ_STATUS: i32 = 173;
const IDC_LLAMA_MMPROJ_INSTALL: i32 = 174;
const IDC_LLAMA_MMPROJ_PATH: i32 = 175;
const IDC_LLAMA_MMPROJ_BROWSE: i32 = 176;

/// エディットコントロールの通知コード (windows クレートに定義がないもの)
const EN_KILLFOCUS: u32 = 0x0200;
const BN_CLICKED: u32 = 0;

/// インストールスレッドからの完了通知 (settings ウィンドウ限定のメッセージ)
const WM_PADDLE_DONE: u32 = WM_APP + 10;
const WM_ONNX_DONE: u32 = WM_APP + 11;
const WM_LLAMA_BIN_DONE: u32 = WM_APP + 12;
const WM_LLAMA_MODEL_DONE: u32 = WM_APP + 13;
const WM_LLAMA_SERVER_DONE: u32 = WM_APP + 14;
/// モデルダウンロードの進捗通知 (10秒おき。lparamにBox<String>の進捗ラベル)
const WM_LLAMA_MODEL_PROGRESS: u32 = WM_APP + 15;
const WM_LLAMA_MMPROJ_DONE: u32 = WM_APP + 16;
const WM_LLAMA_MMPROJ_PROGRESS: u32 = WM_APP + 17;
/// 各APIキーの発行ページ(実際に確認済みの現行URL)
const DEEPL_KEY_URL: &str = "https://www.deepl.com/en/your-account/keys";
const GOOGLE_KEY_URL: &str = "https://console.cloud.google.com/apis/credentials";
/// 左下欄外のバージョン情報 (SPECv0.5.2追補)
const APP_VERSION_LABEL: &str = "なにこれ - FocusTranslator";
const APP_UPDATE_DATE: &str = "2026/7/20";
const GITHUB_RELEASES_URL: &str = "https://github.com/KotorinChunChun/FocusTranslator";

const HOLD_KEYS: [&str; 5] = ["RCtrl", "LCtrl", "RShift", "RAlt", "F8"];
const OCR_KEYS: [&str; 4] = ["oneocr", "win", "paddle", "llm"];
const OCR_DISP: [&str; 4] = [
    "OneOCR (oneocr.dll)",
    "Windows.Media.Ocr.dll",
    "PaddleOCR",
    "LLM(プロファイル)",
];
const TR_KEYS: [&str; 4] = ["local", "deepl", "google", "llm"];
const TR_DISP: [&str; 4] = ["ローカルONNX", "DeepL", "Google", "LLM(プロファイル)"];
const LANGS: [&str; 2] = ["ja", "en"];
const THEME_KEYS: [&str; 3] = ["system", "light", "dark"];
const THEME_DISP: [&str; 3] = ["Windows既定", "ライト", "ダーク"];

thread_local! {
    static WND: RefCell<isize> = const { RefCell::new(0) };
    static REGISTERED: RefCell<bool> = const { RefCell::new(false) };
    static FONT: RefCell<isize> = const { RefCell::new(0) };
    static PROFILES: RefCell<Vec<crate::config::ApiProfile>> = const { RefCell::new(Vec::new()) };
    /// 既定LLMプロファイル名 (OCR/翻訳/解説共通)。【既定にする】ボタンでのみ変更する。
    static DEFAULT_PROFILE: RefCell<String> = const { RefCell::new(String::new()) };
    /// 「新規」ボタン直後 (まだプロファイル保存していない) 状態。
    /// プロンプト欄のUIが無くなったため、この状態での保存は既定プロンプトを使う (SPECv0.4.7 §6.1)。
    static PENDING_NEW: RefCell<bool> = const { RefCell::new(false) };
    /// ウィンドウ破棄中フラグ: 子コントロール破棄過程の EN_KILLFOCUS で
    /// 不完全なUI状態が自動保存されるのを防ぐ。
    static CLOSING: RefCell<bool> = const { RefCell::new(false) };
}

pub fn hwnd() -> HWND {
    HWND(WND.with(|w| *w.borrow()) as *mut _)
}

pub fn is_open() -> bool {
    let h = hwnd();
    !h.is_invalid() && unsafe { IsWindow(Some(h)).as_bool() }
}

pub fn open(instance: HINSTANCE, _main: HWND) {
    if is_open() {
        unsafe {
            let _ = SetForegroundWindow(hwnd());
        }
        return;
    }
    unsafe {
        let class = w!("FocusTranslatorSettings");
        REGISTERED.with(|r| {
            if !*r.borrow() {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(wndproc),
                    hInstance: instance,
                    hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                    hIcon: crate::app_state::app_icon(),
                    hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(
                        (COLOR_BTNFACE.0 + 1) as usize as *mut _,
                    ),
                    lpszClassName: class,
                    ..Default::default()
                };
                RegisterClassW(&wc);
                *r.borrow_mut() = true;
            }
        });
        // 3列レイアウト (SPECv0.4 §5): グループボックスの実サイズ(LAYOUT_CLIENT_W/H)から
        // AdjustWindowRectEx で過不足のないウィンドウサイズを算出する (無駄な余白を残さない)。
        let style = WS_CAPTION | WS_SYSMENU;
        let ex_style = WS_EX_TOPMOST;
        let mut rect = windows::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: LAYOUT_CLIENT_W,
            bottom: LAYOUT_CLIENT_H,
        };
        let _ = windows::Win32::UI::WindowsAndMessaging::AdjustWindowRectEx(
            &mut rect, style, false, ex_style,
        );
        let (win_w, win_h) = (rect.right - rect.left, rect.bottom - rect.top);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let win_y = 10;
        let win_h = win_h.min(screen_h - 40);
        let title_w = crate::util::to_wide(&format!("{} 設定", crate::util::APP_DISPLAY_NAME));
        if let Ok(h) = CreateWindowExW(
            ex_style,
            class,
            PCWSTR(title_w.as_ptr()),
            style,
            CW_USEDEFAULT,
            win_y,
            win_w,
            win_h,
            None,
            None,
            Some(instance),
            None,
        ) {
            WND.with(|w| *w.borrow_mut() = h.0 as isize);
            build_controls(h, instance);
            populate(h);
            let _ = ShowWindow(h, SW_SHOW);
            let _ = SetForegroundWindow(h);
        }
    }
}

// レイアウト定数 (SPECv0.4 §5.1 3列レイアウト): グループ1・3 (各列の上段) は高さを揃え、
// グループ2・4 (各列の下段) も高さを揃えて、枠線の下端が列間でぴたり一致するようにする。
// これらの数値から必要な最小ウィンドウサイズを算出し (LAYOUT_CLIENT_W/H)、open() で
// AdjustWindowRectEx により過不足のないウィンドウサイズへ変換する。
const PAD: i32 = 10;
const COL_W: i32 = 408;
const STEP: i32 = 30;
const GTOP: i32 = 22; // グループ枠タイトル分のオフセット
const LAYOUT_COL_X: [i32; 3] = [PAD, PAD * 2 + COL_W, PAD * 3 + COL_W * 2];
const GROUP_GAP: i32 = 8; // 同列内でグループを縦に並べる際の間隔

const GROUP1_Y: i32 = 8;
const GROUP1_H: i32 = 178; // 5行 (最終行はEDIT高22) + 下余白14
const GROUP3_Y: i32 = GROUP1_Y;
const GROUP3_H: i32 = GROUP1_H; // グループ1と下端を揃える (OCR設定は内容的にはやや短い)

const GROUP2_Y: i32 = GROUP1_Y + GROUP1_H + GROUP_GAP;
const GROUP2_H: i32 = 242; // 7行 (最終行はボタン高26) + 下余白14
const GROUP4_Y: i32 = GROUP3_Y + GROUP3_H + GROUP_GAP;
const GROUP4_H: i32 = GROUP2_H; // グループ2と下端を揃える (翻訳設定は内容的にはやや短い)

const GROUP5_Y: i32 = 8;
const GROUP5_H: i32 = 242; // 7行 (最終行はボタン高26) + 下余白14。偶然グループ2/4と同高
const GROUP6_Y: i32 = GROUP5_Y + GROUP5_H + GROUP_GAP;
const GROUP6_H: i32 = 266; // 8行 (最終行はチェックボックス) + 下余白14

/// 全列の下端のうち最も低い位置 (この直下に閉じるボタン行を置く)
const LAYOUT_CONTENT_BOTTOM: i32 = {
    let left = GROUP2_Y + GROUP2_H;
    let mid = GROUP4_Y + GROUP4_H;
    let right = GROUP6_Y + GROUP6_H;
    let m = if left > mid { left } else { mid };
    if m > right { m } else { right }
};
const LAYOUT_BTN_Y: i32 = LAYOUT_CONTENT_BOTTOM + 16;
const LAYOUT_BTN_H: i32 = 26;
const LAYOUT_CLIENT_W: i32 = LAYOUT_COL_X[2] + COL_W + PAD;
const LAYOUT_CLIENT_H: i32 = LAYOUT_BTN_Y + LAYOUT_BTN_H + 14;

/// BS_GROUPBOX でカテゴリ枠を作る (SPECv0.4 §5.1)
fn group(h: HWND, inst: HINSTANCE, text: &str, x: i32, y: i32, w: i32, ht: i32) {
    const BS_GROUPBOX: u32 = 0x0000_0007;
    ctl(h, inst, w!("BUTTON"), text, WINDOW_STYLE(BS_GROUPBOX), x, y, w, ht, 0);
}

fn build_controls(h: HWND, inst: HINSTANCE) {
    // 3列レイアウト (SPECv0.4 §5.1): 左=操作/OCR、中=翻訳/その他、右=LLMプロファイル
    // グループ1・3 (各列の上段) は高さを揃えて下端を一致させ、グループ2・4 (各列の下段)
    // も高さを揃えて下端を一致させる。ウィンドウサイズは open() 側でこの下端に合わせて
    // 計算する (AdjustWindowRectEx) ため、ここでの数値がそのまま余白の詰まり具合を決める。
    let col_x = LAYOUT_COL_X;
    let inner = |cx: i32| cx + 12; // グループ内の左端
    let key_w = 160;

    // ---- 左列 グループ1: 操作 ----
    {
        let gx = col_x[0];
        let lx = inner(gx);
        let cx = gx + 152;
        group(h, inst, "1. 操作", gx, GROUP1_Y, COL_W, GROUP1_H);
        let mut y = GROUP1_Y + GTOP;
        label(h, inst, "キャプチャキー", lx, y + 2, 130);
        combo(h, inst, cx, y, 90, IDC_HOLDKEY);
        checkbox(h, inst, "領域表示", cx + 98, y + 2, 88, IDC_DETECT_MODE);
        y += STEP;
        label(h, inst, "プレビューキー", lx, y + 2, 130);
        combo(h, inst, cx, y, 90, IDC_DETECT_KEY);
        checkbox(h, inst, "領域表示", cx + 98, y + 2, 88, IDC_PREVIEW_DETECT_MODE);
        y += STEP;
        label(h, inst, "範囲指定ホットキー", lx, y + 2, 130);
        edit(h, inst, cx, y, 120, IDC_HOTKEY);
        y += STEP;
        label(h, inst, "監視周期 (ms)", lx, y + 2, 130);
        edit(h, inst, cx, y, 60, IDC_POLL);
        y += STEP;
        label(h, inst, "ピン留め長押し時間 (秒)", lx, y + 2, 130);
        edit(h, inst, cx, y, 60, IDC_PIN_HOLD);
    }

    // ---- 左列 グループ2: システム設定 (旧 5. その他の設定) ----
    {
        let gx = col_x[0];
        let lx = inner(gx);
        group(h, inst, "2. システム設定", gx, GROUP2_Y, COL_W, GROUP2_H);
        let mut y = GROUP2_Y + GTOP;
        checkbox(h, inst, "起動時に常駐する", lx, y, 170, IDC_AUTOSTART);
        checkbox(h, inst, "計測ログを有効化", lx + 180, y, 160, IDC_PERFLOG);
        y += STEP;
        checkbox(h, inst, "実行ログを記録 (原文/訳文を平文保存)", lx, y, 300, IDC_LOG_ENABLED);
        y += STEP;
        checkbox(h, inst, "デバッグモード (OCR画像をPNG保存)", lx, y, 280, IDC_DEBUG_MODE);
        y += STEP;
        label(h, inst, "保持上限", lx, y + 2, 60);
        edit(h, inst, lx + 66, y, 70, IDC_LOG_MAX);
        button(h, inst, "ログビューアを開く", lx + 150, y - 2, 130, IDC_OPEN_LOG);
        y += STEP;
        label(h, inst, "オーバーレイテーマ", lx, y + 2, 130);
        combo(h, inst, lx + 140, y, 130, IDC_OVERLAY_THEME);
        y += STEP;
        button(h, inst, "外部送信の同意状態をリセット", lx, y, 220, IDC_CONSENT_RESET);
        y += STEP;
        button(h, inst, "設定をリセット (アプリ再起動)", lx, y, 220, IDC_RESET_SETTINGS);
    }

    // ---- 中列 グループ3: OCR設定 (旧 2. OCR設定) ----
    {
        let gx = col_x[1];
        let lx = inner(gx);
        let cx = gx + 152;
        group(h, inst, "3. OCR設定", gx, GROUP3_Y, COL_W, GROUP3_H);
        let mut y = GROUP3_Y + GTOP;
        label(h, inst, "既定OCRエンジン", lx, y + 2, 130);
        combo(h, inst, cx, y, 170, IDC_OCR);
        y += STEP;
        ctl(h, inst, w!("STATIC"), "", WINDOW_STYLE(0), lx, y, COL_W - 24, 40, IDC_OCR_EXP); // 解説表示
        y += 44;
        label(h, inst, "OneOCR", lx, y + 2, 130);
        ctl(h, inst, w!("STATIC"), "確認中…", WINDOW_STYLE(0), cx, y + 2, 140, 20, IDC_ONEOCR_STATUS);
        y += STEP;
        label(h, inst, "PaddleOCR", lx, y + 2, 130);
        ctl(h, inst, w!("STATIC"), "確認中…", WINDOW_STYLE(0), cx, y + 2, 100, 20, IDC_PADDLE_STATUS);
        button(h, inst, "インストール", cx + 106, y - 2, 104, IDC_PADDLE_INSTALL);
    }

    // ---- 中列 グループ4: 翻訳設定 (旧 3. 翻訳設定) ----
    {
        let gx = col_x[1];
        let lx = inner(gx);
        let cx = gx + 152;
        group(h, inst, "4. 翻訳設定", gx, GROUP4_Y, COL_W, GROUP4_H);
        let mut y = GROUP4_Y + GTOP;
        label(h, inst, "既定翻訳エンジン", lx, y + 2, 130);
        combo(h, inst, cx, y, 170, IDC_TR);
        y += STEP;
        // 解説表示: OCR設定側と同じ高さ(40px)に揃える (旧60pxは1行分の余白が無駄だった)
        ctl(h, inst, w!("STATIC"), "", WINDOW_STYLE(0), lx, y, COL_W - 24, 40, IDC_TR_EXP);
        y += 44;
        label(h, inst, "ローカルONNX翻訳 (FuguMT)", lx, y + 2, 150);
        ctl(h, inst, w!("STATIC"), "確認中…", WINDOW_STYLE(0), cx, y + 2, 100, 20, IDC_ONNX_STATUS);
        button(h, inst, "インストール", cx + 106, y - 2, 104, IDC_ONNX_INSTALL);
        y += STEP;
        label(h, inst, "翻訳元言語 / 訳先言語", lx, y + 2, 140);
        combo(h, inst, cx, y, 70, IDC_SRCLANG);
        label(h, inst, "→", cx + 76, y + 2, 16);
        combo(h, inst, cx + 94, y, 70, IDC_LANG);
        y += STEP;
        label(h, inst, "DeepL APIキー", lx, y + 2, 130);
        password_edit(h, inst, cx, y, key_w, IDC_DEEPL);
        button(h, inst, "取得ページ", cx + key_w + 6, y - 2, 76, IDC_DEEPL_URL);
        y += STEP;
        label(h, inst, "Google Trans APIキー", lx, y + 2, 140);
        password_edit(h, inst, cx, y, key_w, IDC_GOOGLE);
        button(h, inst, "取得ページ", cx + key_w + 6, y - 2, 76, IDC_GOOGLE_URL);
    }

    // ---- 右列 グループ5: LLMプロファイル設定 ----
    {
        let gx = col_x[2];
        let lx = inner(gx);
        let cx = gx + 100;
        group(h, inst, "5. LLMプロファイル設定", gx, GROUP5_Y, COL_W, GROUP5_H);
        let mut y = GROUP5_Y + GTOP;
        label(h, inst, "プロファイル編集", lx, y + 2, 90);
        combo(h, inst, lx + 96, y, 150, IDC_PROF_LIST);
        y += STEP;
        button(h, inst, "新規", lx, y, 46, IDC_PROF_NEW);
        button(h, inst, "保存", lx + 50, y, 46, IDC_PROF_SAVE);
        button(h, inst, "別名保存", lx + 100, y, 66, IDC_PROF_SAVEAS);
        button(h, inst, "削除", lx + 170, y, 46, IDC_PROF_DEL);
        button(h, inst, "既定にする", lx + 222, y, 90, IDC_PROF_SET_DEFAULT);
        y += STEP;
        label(h, inst, "API登録名", lx, y + 2, 84);
        edit(h, inst, cx, y, 140, IDC_PROF_NAME);
        label(h, inst, "種別", cx + 150, y + 2, 36);
        combo(h, inst, cx + 188, y, 100, IDC_PROF_TYPE);
        y += STEP;
        label(h, inst, "API URL", lx, y + 2, 84);
        edit(h, inst, cx, y, 288, IDC_PROF_URL);
        y += STEP;
        label(h, inst, "APIキー", lx, y + 2, 84);
        password_edit(h, inst, cx, y, key_w, IDC_PROF_KEY);
        y += STEP;
        label(h, inst, "モデル名", lx, y + 2, 84);
        edit(h, inst, cx, y, 180, IDC_PROF_MODEL);
        y += STEP;
        // プロンプトは専用の編集ウィンドウで編集する (SPECv0.4.7 §1)
        label(h, inst, "プロンプト編集", lx, y + 4, 84);
        button(h, inst, "翻訳プロンプト", cx, y, 92, IDC_PROMPT_TR_BTN);
        button(h, inst, "OCRプロンプト", cx + 98, y, 92, IDC_PROMPT_OCR_BTN);
        button(h, inst, "解説プロンプト", cx + 196, y, 92, IDC_PROMPT_EXP_BTN);
    }

    // ---- 右列 グループ6: ローカルLLM (llama.cpp) (SPECv0.5.2追補) ----
    // ラベル/状態表示/操作ボタンの3列を全行で同じX位置・同じ幅に揃える。
    // パス欄+参照ボタンの行も、参照ボタンが他行のボタン列(BX)に揃うよう幅を合わせる。
    {
        let gx = col_x[2];
        let lx = inner(gx);
        const LABEL_W: i32 = 150;
        let vx = lx + LABEL_W + 8; // 状態表示・値入力の開始X
        const VALUE_W: i32 = 120;
        let bx = vx + VALUE_W + 8; // 操作ボタンの開始X (全行共通)
        const BTN_W: i32 = 96;
        let path_w = bx - lx - 8; // パス欄の幅 (参照ボタンがbxに揃うよう逆算)
        group(h, inst, "6. ローカルLLMサーバー (llama.cpp)", gx, GROUP6_Y, COL_W, GROUP6_H);
        let mut y = GROUP6_Y + GTOP;
        label(h, inst, "サーバープログラム本体", lx, y + 2, LABEL_W);
        ctl(h, inst, w!("STATIC"), "確認中…", WINDOW_STYLE(0), vx, y + 2, VALUE_W, 20, IDC_LLAMA_BIN_STATUS);
        button(h, inst, "インストール", bx, y - 2, BTN_W, IDC_LLAMA_BIN_INSTALL);
        y += STEP;
        // モデルファイルは既定パス(自動ダウンロード)またはLM Studio等で導入済みのGGUFの
        // 明示パスのどちらかを使う。空欄なら既定パスを使う (llama_install::resolve_model_path)。
        label(h, inst, "言語モデル(Gemma4-E2B)", lx, y + 2, LABEL_W);
        ctl(h, inst, w!("STATIC"), "確認中…", WINDOW_STYLE(0), vx, y + 2, VALUE_W, 20, IDC_LLAMA_MODEL_STATUS);
        button(h, inst, "ダウンロード", bx, y - 2, BTN_W, IDC_LLAMA_MODEL_INSTALL);
        y += STEP;
        edit(h, inst, lx, y, path_w, IDC_LLAMA_MODEL_PATH);
        button(h, inst, "参照…", bx, y - 2, BTN_W, IDC_LLAMA_MODEL_BROWSE);
        y += STEP;
        // mmproj(画像入力対応): 指定されていればサーバー起動時に --mmproj で渡され、
        // 同一ポートのまま画像入力(OCRのLLM経路)にも対応する。未指定ならテキスト専用。
        label(h, inst, "画像入力モデル(mmproj)", lx, y + 2, LABEL_W);
        ctl(h, inst, w!("STATIC"), "確認中…", WINDOW_STYLE(0), vx, y + 2, VALUE_W, 20, IDC_LLAMA_MMPROJ_STATUS);
        button(h, inst, "ダウンロード", bx, y - 2, BTN_W, IDC_LLAMA_MMPROJ_INSTALL);
        y += STEP;
        edit(h, inst, lx, y, path_w, IDC_LLAMA_MMPROJ_PATH);
        button(h, inst, "参照…", bx, y - 2, BTN_W, IDC_LLAMA_MMPROJ_BROWSE);
        y += STEP;
        label(h, inst, "サーバーポート", lx, y + 2, LABEL_W);
        edit(h, inst, vx, y, 70, IDC_LLAMA_PORT);
        y += STEP;
        label(h, inst, "サーバー状態", lx, y + 2, LABEL_W);
        ctl(h, inst, w!("STATIC"), "停止中", WINDOW_STYLE(0), vx, y + 2, VALUE_W, 20, IDC_LLAMA_SERVER_STATUS);
        button(h, inst, "起動", bx, y - 2, BTN_W, IDC_LLAMA_TOGGLE);
        y += STEP;
        checkbox(h, inst, "起動時にサーバーを自動起動する", lx, y, 260, IDC_LLAMA_AUTOSTART);
    }

    // ---- 下部ボタン領域 (右下; SPECv0.4 §5.2)
    // 設定は変更時に即座に保存されるため【閉じる】のみ (SPECv0.4.7 改)
    let right = col_x[2] + COL_W;
    button(h, inst, "閉じる", right - 86, LAYOUT_BTN_Y, 80, IDC_CLOSE);

    // ---- 左下欄外: バージョン情報 (SPECv0.5.2追補) ----
    let version_text = format!("{APP_VERSION_LABEL}  v{}  (更新日: {APP_UPDATE_DATE})", env!("CARGO_PKG_VERSION"));
    ctl(h, inst, w!("STATIC"), &version_text, WINDOW_STYLE(0), col_x[0], LAYOUT_BTN_Y + 4, 320, 20, IDC_VERSION_INFO);
    button(h, inst, "GitHub", col_x[0] + 326, LAYOUT_BTN_Y, 80, IDC_GITHUB_LINK);

    // フォント設定
    unsafe {
        let font: HFONT = make_font(13, false);
        FONT.with(|f| *f.borrow_mut() = font.0 as isize);
        let _ = windows::Win32::UI::WindowsAndMessaging::EnumChildWindows(
            Some(h),
            Some(set_font_proc),
            LPARAM(font.0 as isize),
        );
    }
}

fn populate(h: HWND) {
    let cfg = Config::load();
    combo_fill(
        h,
        IDC_HOLDKEY,
        &HOLD_KEYS,
        HOLD_KEYS.iter().position(|k| *k == cfg.hold_key).unwrap_or(0),
    );
    set_ctl_text(h, IDC_POLL, &cfg.poll_ms.to_string());
    set_ctl_text(h, IDC_PIN_HOLD, &cfg.pin_hold_seconds.to_string());
    set_ctl_text(h, IDC_HOTKEY, &cfg.region_hotkey);
    combo_fill(
        h,
        IDC_OCR,
        &OCR_DISP,
        OCR_KEYS.iter().position(|k| *k == cfg.default_ocr).unwrap_or(0),
    );
    combo_fill(
        h,
        IDC_TR,
        &TR_DISP,
        TR_KEYS.iter().position(|k| *k == cfg.default_translator).unwrap_or(0),
    );
    combo_fill(h, IDC_SRCLANG, &LANGS, LANGS.iter().position(|k| *k == cfg.source_lang).unwrap_or(1));
    combo_fill(h, IDC_LANG, &LANGS, LANGS.iter().position(|k| *k == cfg.target_lang).unwrap_or(0));
    set_ctl_text(h, IDC_DEEPL, &cfg.deepl_key());
    set_ctl_text(h, IDC_GOOGLE, &cfg.google_key());

    PROFILES.with(|p| *p.borrow_mut() = cfg.api_profiles.clone());
    // 既定LLMプロファイル名を保持する (active とは独立)。コンボの「(既定)」表示に使うため
    // refill_profile_combo より先に確定させておく。
    let default_name = if cfg.api_profiles.iter().any(|p| p.name == cfg.default_api_profile) {
        cfg.default_api_profile.clone()
    } else {
        cfg.api_profiles.first().map(|p| p.name.clone()).unwrap_or_default()
    };
    DEFAULT_PROFILE.with(|d| *d.borrow_mut() = default_name);
    let sel = cfg.api_profiles.iter().position(|p| p.name == cfg.active_api_profile).unwrap_or(0);
    refill_profile_combo(h, sel);

    combo_reset(h, IDC_PROF_TYPE);
    combo_fill(h, IDC_PROF_TYPE, &API_TYPE_DISP, 0);

    load_profile_to_ui(h, sel);
    update_prof_default_btn(h);
    check_set(h, IDC_AUTOSTART, cfg.autostart);
    check_set(h, IDC_PERFLOG, cfg.perf_log);
    check_set(h, IDC_LOG_ENABLED, cfg.log_enabled);
    check_set(h, IDC_DEBUG_MODE, cfg.debug_mode);
    check_set(h, IDC_DETECT_MODE, cfg.detect_enabled);
    combo_fill(
        h,
        IDC_DETECT_KEY,
        &HOLD_KEYS,
        HOLD_KEYS.iter().position(|k| *k == cfg.detect_key).unwrap_or(1), // 既定 LCtrl
    );
    check_set(h, IDC_PREVIEW_DETECT_MODE, cfg.preview_detect_enabled);
    combo_fill(
        h,
        IDC_OVERLAY_THEME,
        &THEME_DISP,
        THEME_KEYS.iter().position(|k| *k == cfg.overlay_theme).unwrap_or(0),
    );
    set_ctl_text(h, IDC_LOG_MAX, &cfg.log_max_records.to_string());
    set_ctl_text(h, IDC_LLAMA_PORT, &cfg.llama_port.to_string());
    set_ctl_text(h, IDC_LLAMA_MODEL_PATH, &cfg.llama_model_path);
    check_set(h, IDC_LLAMA_AUTOSTART, cfg.llama_auto_start);
    set_ctl_text(h, IDC_LLAMA_MMPROJ_PATH, &cfg.llama_mmproj_path);
    refresh_oneocr_status(h);
    refresh_paddle_status(h);
    refresh_onnx_status(h);
    refresh_llama_status(h);
    update_explanations(h);
}

/// OneOCRがこのPCで使用可能かの判定結果 (config::engine_available("oneocr") と同じ判定) を表示する。
/// PaddleOCRと異なり利用者が能動的に導入する手段が無い (Snipping Toolからの検出/自動コピー) ため、
/// インストールボタンは設けず判定結果のみ示す。
fn refresh_oneocr_status(h: HWND) {
    let available = crate::oneocr::available();
    set_ctl_text(h, IDC_ONEOCR_STATUS, if available { "利用可能です" } else { "利用できません" });
}

/// PaddleOCRの導入状況をステータス欄・ボタンに反映する
fn refresh_paddle_status(h: HWND) {
    let installed = crate::paddle_install::installed();
    set_ctl_text(h, IDC_PADDLE_STATUS, if installed { "導入済み" } else { "未導入" });
    unsafe {
        let _ = EnableWindow(windows::Win32::UI::WindowsAndMessaging::GetDlgItem(Some(h), IDC_PADDLE_INSTALL).unwrap_or_default(), !installed);
    }
}

/// 設定画面で現在選択中のローカル翻訳モデル種別
/// ローカルONNX翻訳モデル(FuguMT)の導入状況をステータス欄・ボタンに反映する
fn refresh_onnx_status(h: HWND) {
    let installed = crate::onnx_translate_install::installed();
    set_ctl_text(h, IDC_ONNX_STATUS, if installed { "導入済み" } else { "未導入" });
    unsafe {
        let _ = EnableWindow(windows::Win32::UI::WindowsAndMessaging::GetDlgItem(Some(h), IDC_ONNX_INSTALL).unwrap_or_default(), !installed);
    }
}

/// llama.cpp本体・モデルの両方が導入済みになったら、"LocalLLM" という名前のAPIプロファイルが
/// 無ければ自動登録する (SPECv0.5.2追補)。既に存在する場合は利用者の編集を尊重し上書きしない。
fn ensure_local_llm_profile_if_ready(h: HWND) {
    if !(crate::llama_install::installed() && crate::llama_install::model_installed()) {
        return;
    }
    let mut cfg = Config::load();
    if cfg.api_profiles.iter().any(|p| p.name == "LocalLLM") {
        return;
    }
    let port: u32 = get_ctl_text(h, IDC_LLAMA_PORT).trim().parse().unwrap_or(crate::llama_server::DEFAULT_PORT);
    let profile = crate::config::ApiProfile {
        name: "LocalLLM".into(),
        api_type: crate::config::ApiType::LlamaCpp,
        model_name: crate::config::ApiType::LlamaCpp.default_model().into(),
        api_url: format!("http://localhost:{port}/v1/chat/completions"),
        api_key_enc: String::new(),
        ocr_prompt: crate::config::DEFAULT_GEMINI_OCR_PROMPT.into(),
        translate_prompt: crate::config::DEFAULT_GEMINI_TRANSLATE_PROMPT.into(),
        explain_prompt: crate::config::DEFAULT_GEMINI_EXPLAIN_PROMPT.into(),
    };
    cfg.api_profiles.push(profile.clone());
    cfg.save();
    PROFILES.with(|p| p.borrow_mut().push(profile));
    let sel = PROFILES.with(|p| p.borrow().len().saturating_sub(1));
    refill_profile_combo(h, sel);
}

/// llama.cpp本体・モデルの導入状況とサーバー稼働状況をステータス欄・ボタンに反映する
/// (SPECv0.5.2追補)。稼働確認はポートへの疎通確認のため、未導入時はスキップする。
fn refresh_llama_status(h: HWND) {
    let bin_ok = crate::llama_install::installed();
    let override_path = get_ctl_text(h, IDC_LLAMA_MODEL_PATH);
    let model_path = crate::llama_install::resolve_model_path(&override_path);
    let model_ok = model_path.is_file();
    set_ctl_text(h, IDC_LLAMA_BIN_STATUS, if bin_ok { "導入済み" } else { "未導入" });
    set_ctl_text(h, IDC_LLAMA_MODEL_STATUS, if model_ok { "選択済み" } else { "未選択" });
    // ダウンロードボタンは既定の管理下ディレクトリに既にモデルがあれば無効化する
    // (SPECv0.5.2追補: 参照ボタンで指定した外部パスの有無は問わない。あくまで
    // 「このボタンでダウンロードした結果」の有無で判定する)。
    let downloaded = crate::llama_install::model_installed();
    unsafe {
        let _ = EnableWindow(windows::Win32::UI::WindowsAndMessaging::GetDlgItem(Some(h), IDC_LLAMA_BIN_INSTALL).unwrap_or_default(), !bin_ok);
        let _ = EnableWindow(windows::Win32::UI::WindowsAndMessaging::GetDlgItem(Some(h), IDC_LLAMA_MODEL_INSTALL).unwrap_or_default(), !downloaded);
    }
    // mmproj(画像入力対応)の状態: 任意項目のため、未選択でもサーバーはテキスト専用として起動できる
    let mmproj_path = crate::llama_install::resolve_mmproj_path(&get_ctl_text(h, IDC_LLAMA_MMPROJ_PATH));
    let mmproj_ok = mmproj_path.is_file();
    set_ctl_text(h, IDC_LLAMA_MMPROJ_STATUS, if mmproj_ok { "選択済み" } else { "未選択(任意)" });
    let mmproj_downloaded = crate::llama_install::mmproj_installed();
    unsafe {
        let _ = EnableWindow(windows::Win32::UI::WindowsAndMessaging::GetDlgItem(Some(h), IDC_LLAMA_MMPROJ_INSTALL).unwrap_or_default(), !mmproj_downloaded);
    }

    let port: u32 = get_ctl_text(h, IDC_LLAMA_PORT).trim().parse().unwrap_or(crate::llama_server::DEFAULT_PORT);
    let running = bin_ok && model_ok && crate::llama_server::is_running(port);
    set_ctl_text(h, IDC_LLAMA_SERVER_STATUS, if running { "稼働中" } else { "停止中" });
    set_ctl_text(h, IDC_LLAMA_TOGGLE, if running { "停止" } else { "起動" });
    // 起動ボタンは常に押せる状態にしておく。本体/モデル未導入で起動できない場合は
    // llama_server::start() が理由付きのエラーメッセージを返すので、そちらで案内する
    // (SPECv0.5.2追補: 押せない理由が分からずグレーアウトだけ見える状態を避ける)。
    unsafe {
        let _ = EnableWindow(windows::Win32::UI::WindowsAndMessaging::GetDlgItem(Some(h), IDC_LLAMA_TOGGLE).unwrap_or_default(), true);
    }
}

/// 選択されたOCRエンジンおよび翻訳エンジンに対する解説をStatic Textに反映する
fn update_explanations(h: HWND) {
    let ocr_idx = combo_sel(h, IDC_OCR).min(OCR_KEYS.len().saturating_sub(1));
    let ocr_desc = if OCR_KEYS.is_empty() { "" } else { match OCR_KEYS[ocr_idx] {
        "oneocr" => "【OneOCR】Windows11標準モデルを使用する、軽量・高速で標準的なOCRエンジンです。",
        "win" => "【Windows】内蔵OCR(MediaOCR)を使用します。Windows10対応ですが精度は劣ります。",
        "paddle" => "【PaddleOCR】高精度なOCRです。インストールが必要です。",
        "llm" => "【LLM】AIを使用して画像から直接テキストを抽出します。",
        _ => "",
    }};
    set_ctl_text(h, IDC_OCR_EXP, ocr_desc);

    let tr_idx = combo_sel(h, IDC_TR).min(TR_KEYS.len().saturating_sub(1));
    let tr_desc = if TR_KEYS.is_empty() { "" } else { match TR_KEYS[tr_idx] {
        "local" => "【ローカルONNX】オフラインで高速・安全に翻訳します。",
        "deepl" => "【DeepL】高精度な翻訳を行います。APIキーが必要です。",
        "google" => "【Google】Google翻訳を使用します。APIキーが必要です。",
        "llm" => "【LLM】AIプロファイルを使用して文脈に応じた翻訳を行います。",
        _ => "",
    }};
    set_ctl_text(h, IDC_TR_EXP, tr_desc);
}

/// APIプロファイル種別のコンボ表示順 (IDC_PROF_TYPE の選択indexと対応)
const API_TYPE_ORDER: [crate::config::ApiType; 4] = [
    crate::config::ApiType::Gemini,
    crate::config::ApiType::OpenAI,
    crate::config::ApiType::Claude,
    crate::config::ApiType::LlamaCpp,
];
const API_TYPE_DISP: [&str; 4] = ["Gemini", "OpenAI", "Claude", "llama.cpp"];

fn api_type_index(t: &crate::config::ApiType) -> usize {
    API_TYPE_ORDER.iter().position(|x| x == t).unwrap_or(0)
}

/// コンボの内容を全消去する
/// PROFILES の内容でプロファイル一覧コンボを再構築する。既定LLMプロファイルには
/// 末尾に「(既定)」を併記する (表示のみ。名前そのものやconfig.jsonには付与しない)。
fn refill_profile_combo(h: HWND, sel: usize) {
    let default_name = DEFAULT_PROFILE.with(|d| d.borrow().clone());
    let names: Vec<String> = PROFILES.with(|p| {
        p.borrow()
            .iter()
            .map(|x| if x.name == default_name { format!("{} (既定)", x.name) } else { x.name.clone() })
            .collect()
    });
    let strs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();

    combo_reset(h, IDC_PROF_LIST);
    combo_fill(h, IDC_PROF_LIST, &strs, sel);
}

/// 【既定にする】ボタンの有効/無効を更新する。新規未保存中、または選択中が既に既定なら無効。
fn update_prof_default_btn(h: HWND) {
    let is_pending = PENDING_NEW.with(|f| *f.borrow());
    let sel = combo_sel(h, IDC_PROF_LIST);
    let is_current_default = !is_pending
        && PROFILES.with(|p| {
            let default_name = DEFAULT_PROFILE.with(|d| d.borrow().clone());
            p.borrow().get(sel).map(|x| x.name == default_name).unwrap_or(false)
        });
    unsafe {
        let _ = EnableWindow(
            windows::Win32::UI::WindowsAndMessaging::GetDlgItem(Some(h), IDC_PROF_SET_DEFAULT).unwrap_or_default(),
            !is_pending && !is_current_default,
        );
    }
}

fn load_profile_to_ui(h: HWND, idx: usize) {
    PENDING_NEW.with(|f| *f.borrow_mut() = false);
    PROFILES.with(|p| {
        let profiles = p.borrow();
        if let Some(prof) = profiles.get(idx) {
            set_ctl_text(h, IDC_PROF_NAME, &prof.name);
            combo_select(h, IDC_PROF_TYPE, api_type_index(&prof.api_type));
            set_ctl_text(h, IDC_PROF_MODEL, &prof.model_name);
            set_ctl_text(h, IDC_PROF_URL, &prof.api_url);
            set_ctl_text(h, IDC_PROF_KEY, &prof.get_key());
        }
    });
}

/// GGUFファイル選択ダイアログを開く (SPECv0.5.2追補: LM Studio等で導入済みのモデルを
/// 再利用する場合に使う)。キャンセル時は None。
fn browse_gguf_file(h: HWND, path_ctl_id: i32) -> Option<String> {
    let mut file_buf = [0u16; 1024];
    // 既存の入力値を初期値として使う (ユーザーが既に手入力していた場合の起点)
    let current = get_ctl_text(h, path_ctl_id);
    if !current.trim().is_empty() {
        let wide = to_wide(current.trim());
        let n = wide.len().min(file_buf.len() - 1);
        file_buf[..n].copy_from_slice(&wide[..n]);
    }
    let filter = to_wide("GGUFモデル (*.gguf)\0*.gguf\0すべてのファイル (*.*)\0*.*\0\0");
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: h,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: windows::core::PWSTR(file_buf.as_mut_ptr()),
        nMaxFile: file_buf.len() as u32,
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST,
        ..Default::default()
    };
    let ok = unsafe { GetOpenFileNameW(&mut ofn) };
    if !ok.as_bool() {
        return None;
    }
    let len = file_buf.iter().position(|&c| c == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&file_buf[..len]))
}

/// インストールボタン押下時の共通処理: ボタン無効化→バックグラウンドDL→完了時に done_msg を通知
fn start_install(
    h: HWND,
    status_id: i32,
    button_id: i32,
    done_msg: u32,
    in_progress_label: &str,
    install_fn: impl FnOnce() -> Result<(), String> + Send + 'static,
) {
    unsafe {
        let _ = EnableWindow(windows::Win32::UI::WindowsAndMessaging::GetDlgItem(Some(h), button_id).unwrap_or_default(), false);
    }
    set_ctl_text(h, status_id, in_progress_label);
    let hwnd_isize = h.0 as isize;
    std::thread::spawn(move || {
        let result = install_fn();
        let (w, l) = match result {
            Ok(()) => (1usize, 0isize),
            Err(e) => (0usize, Box::into_raw(Box::new(e)) as isize),
        };
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd_isize as *mut _)), done_msg, WPARAM(w), LPARAM(l));
        }
    });
}

/// モデルダウンロードボタン押下時の処理 (SPECv0.5.2追補)。start_install() と異なり、
/// 10秒おきの進捗(%または受信済みMB)をWM_LLAMA_MODEL_PROGRESSで反映する。
fn start_model_install(h: HWND) {
    unsafe {
        let _ = EnableWindow(windows::Win32::UI::WindowsAndMessaging::GetDlgItem(Some(h), IDC_LLAMA_MODEL_INSTALL).unwrap_or_default(), false);
    }
    set_ctl_text(h, IDC_LLAMA_MODEL_STATUS, "取得中…");
    let hwnd_isize = h.0 as isize;
    std::thread::spawn(move || {
        let progress = move |downloaded: u64, total: Option<u64>| {
            let label = match total {
                Some(t) if t > 0 => format!("{}%", (downloaded * 100 / t).min(100)),
                _ => format!("{}MB取得済み", downloaded / 1_000_000),
            };
            let ptr = Box::into_raw(Box::new(label)) as isize;
            unsafe {
                let _ = PostMessageW(Some(HWND(hwnd_isize as *mut _)), WM_LLAMA_MODEL_PROGRESS, WPARAM(0), LPARAM(ptr));
            }
        };
        let result = crate::llama_install::install_model(progress);
        let (w, l) = match result {
            Ok(()) => (1usize, 0isize),
            Err(e) => (0usize, Box::into_raw(Box::new(e)) as isize),
        };
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd_isize as *mut _)), WM_LLAMA_MODEL_DONE, WPARAM(w), LPARAM(l));
        }
    });
}

/// mmproj(画像入力対応)のダウンロードボタン押下時の処理 (SPECv0.5.2追補)。
/// 進捗を10秒おきに反映する。
fn start_mmproj_install(h: HWND) {
    unsafe {
        let _ = EnableWindow(windows::Win32::UI::WindowsAndMessaging::GetDlgItem(Some(h), IDC_LLAMA_MMPROJ_INSTALL).unwrap_or_default(), false);
    }
    set_ctl_text(h, IDC_LLAMA_MMPROJ_STATUS, "取得中…");
    let hwnd_isize = h.0 as isize;
    std::thread::spawn(move || {
        let progress = move |downloaded: u64, total: Option<u64>| {
            let label = match total {
                Some(t) if t > 0 => format!("{}%", (downloaded * 100 / t).min(100)),
                _ => format!("{}MB取得済み", downloaded / 1_000_000),
            };
            let ptr = Box::into_raw(Box::new(label)) as isize;
            unsafe {
                let _ = PostMessageW(Some(HWND(hwnd_isize as *mut _)), WM_LLAMA_MMPROJ_PROGRESS, WPARAM(0), LPARAM(ptr));
            }
        };
        let result = crate::llama_install::install_mmproj(progress);
        let (w, l) = match result {
            Ok(()) => (1usize, 0isize),
            Err(e) => (0usize, Box::into_raw(Box::new(e)) as isize),
        };
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd_isize as *mut _)), WM_LLAMA_MMPROJ_DONE, WPARAM(w), LPARAM(l));
        }
    });
}

/// インストール完了通知 (WM_PADDLE_DONE / WM_ONNX_DONE) の共通処理
fn handle_install_done(h: HWND, wparam: WPARAM, lparam: LPARAM, refresh: fn(HWND), success_msg: &str) {
    if wparam.0 == 1 {
        refresh(h);
        unsafe {
            let wide = to_wide(success_msg);
            MessageBoxW(
                Some(h),
                PCWSTR(wide.as_ptr()),
                crate::util::display_name_pcwstr(),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    } else {
        let msg = unsafe { *Box::from_raw(lparam.0 as *mut String) };
        refresh(h);
        unsafe {
            let wide = to_wide(&msg);
            MessageBoxW(Some(h), PCWSTR(wide.as_ptr()), w!("インストールエラー"), MB_OK);
        }
    }
}

/// プロンプト編集ウィンドウの保存からの同期 (SPECv0.4.7 §4.3):
/// 設定画面が開いていればメモリ上 PROFILES の該当プロファイルの該当プロンプトを更新する。
/// 他フィールドの未保存編集 (UI上の入力) には触れない。
pub fn update_prompt_in_memory(name: &str, kind: crate::prompt_edit::PromptKind, text: &str) {
    if !is_open() {
        return;
    }
    PROFILES.with(|p| {
        if let Some(prof) = p.borrow_mut().iter_mut().find(|x| x.name == name) {
            match kind {
                crate::prompt_edit::PromptKind::Translate => prof.translate_prompt = text.to_string(),
                crate::prompt_edit::PromptKind::Ocr => prof.ocr_prompt = text.to_string(),
                crate::prompt_edit::PromptKind::Explain => prof.explain_prompt = text.to_string(),
            }
        }
    });
}

/// 設定の即時保存 (SPECv0.4.7 改): 変更をディスクへ保存し main へ再読込を通知する
fn auto_save(h: HWND, ask_consent: bool) {
    if CLOSING.with(|f| *f.borrow()) {
        return;
    }
    save(h, ask_consent);
    unsafe {
        let _ = PostMessageW(
            Some(crate::app_state::main_hwnd()),
            crate::app_state::WM_APP_CFG,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

/// プロファイル編集UI (名前/種別/URL/キー/モデル) が PROFILES の保存済み内容と異なるか
fn profile_ui_dirty(h: HWND) -> bool {
    if PENDING_NEW.with(|f| *f.borrow()) {
        return true;
    }
    let name = get_ctl_text(h, IDC_PROF_NAME).trim().to_string();
    PROFILES.with(|p| {
        let profiles = p.borrow();
        let Some(prof) = profiles.iter().find(|x| x.name == name) else {
            return true;
        };
        prof.api_type != API_TYPE_ORDER[combo_sel(h, IDC_PROF_TYPE).min(API_TYPE_ORDER.len() - 1)]
            || prof.model_name != get_ctl_text(h, IDC_PROF_MODEL).trim()
            || prof.api_url != get_ctl_text(h, IDC_PROF_URL).trim()
            || prof.get_key() != get_ctl_text(h, IDC_PROF_KEY).trim()
    })
}

/// プロファイル編集UIの内容で PROFILES を更新する (保存/別名保存の共通処理)。
/// プロンプトはUIに無いため、新規なら既定値、既存なら保存済みの値を引き継ぐ (SPECv0.4.7 §6.1)。
/// 成功時は該当プロファイルのindexを返し、コンボを再構築して設定も即保存する。
fn save_profile_from_ui(h: HWND, save_as: bool) -> Option<usize> {
    let name = get_ctl_text(h, IDC_PROF_NAME).trim().to_string();
    if name.is_empty() {
        unsafe {
            MessageBoxW(Some(h), w!("API登録名を入力してください"), w!("エラー"), MB_OK);
        }
        return None;
    }
    // プロンプトの引き継ぎ元: 新規=既定値 / 同名の既存=その値 / 別名複製=選択中プロファイルの値
    let (ocr_p, tr_p, exp_p) = PROFILES.with(|p| {
        let profiles = p.borrow();
        let src = if PENDING_NEW.with(|f| *f.borrow()) {
            None
        } else {
            profiles
                .iter()
                .find(|x| x.name == name)
                .or_else(|| profiles.get(combo_sel(h, IDC_PROF_LIST)))
        };
        match src {
            Some(s) => (s.ocr_prompt.clone(), s.translate_prompt.clone(), s.explain_prompt.clone()),
            None => (
                crate::config::DEFAULT_GEMINI_OCR_PROMPT.to_string(),
                crate::config::DEFAULT_GEMINI_TRANSLATE_PROMPT.to_string(),
                crate::config::DEFAULT_GEMINI_EXPLAIN_PROMPT.to_string(),
            ),
        }
    });
    let mut prof = crate::config::ApiProfile {
        name: name.clone(),
        api_type: API_TYPE_ORDER[combo_sel(h, IDC_PROF_TYPE).min(API_TYPE_ORDER.len() - 1)].clone(),
        model_name: get_ctl_text(h, IDC_PROF_MODEL).trim().to_string(),
        api_url: get_ctl_text(h, IDC_PROF_URL).trim().to_string(),
        api_key_enc: String::new(),
        ocr_prompt: ocr_p,
        translate_prompt: tr_p,
        explain_prompt: exp_p,
    };
    prof.set_key(get_ctl_text(h, IDC_PROF_KEY).trim());

    let saved = PROFILES.with(|p| {
        let mut profiles = p.borrow_mut();
        if !save_as {
            if let Some(existing) = profiles.iter_mut().find(|x| x.name == name) {
                *existing = prof.clone();
            } else {
                profiles.push(prof.clone());
            }
        } else {
            // 別名保存: 名前重複は拒否
            if profiles.iter().any(|x| x.name == name) {
                unsafe {
                    MessageBoxW(Some(h), w!("その名前は既に存在します"), w!("エラー"), MB_OK);
                }
                return None;
            }
            profiles.push(prof.clone());
        }
        profiles.iter().position(|p| p.name == name)
    })?;
    PENDING_NEW.with(|f| *f.borrow_mut() = false);
    refill_profile_combo(h, saved);
    auto_save(h, false);
    Some(saved)
}

/// プロンプト編集ボタン (SPECv0.4.7 §6.1): プロファイルが未保存なら保存確認→保存後に
/// プロンプト編集ウィンドウ (モードA) を開く。
fn open_prompt_editor(h: HWND, kind: crate::prompt_edit::PromptKind) {
    if profile_ui_dirty(h) {
        let r = unsafe {
            MessageBoxW(
                Some(h),
                w!("プロファイルが保存されていません。保存してからプロンプト編集を開きますか?"),
                crate::util::display_name_pcwstr(),
                MB_YESNO,
            )
        };
        if r.0 != 6 {
            // IDYES 以外は開かない
            return;
        }
        if save_profile_from_ui(h, false).is_none() {
            return;
        }
    }
    let name = get_ctl_text(h, IDC_PROF_NAME).trim().to_string();
    let (profiles, active_idx) = PROFILES.with(|p| {
        let profiles = p.borrow();
        let list: Vec<crate::prompt_edit::ProfilePrompt> = profiles
            .iter()
            .map(|x| crate::prompt_edit::ProfilePrompt {
                name: x.name.clone(),
                template: match kind {
                    crate::prompt_edit::PromptKind::Translate => x.translate_prompt.clone(),
                    crate::prompt_edit::PromptKind::Ocr => x.ocr_prompt.clone(),
                    crate::prompt_edit::PromptKind::Explain => x.explain_prompt.clone(),
                },
            })
            .collect();
        let idx = profiles.iter().position(|x| x.name == name).unwrap_or(0);
        (list, idx)
    });
    if profiles.is_empty() {
        return;
    }
    // 設定画面の近傍に表示する
    let pos = unsafe {
        let mut r = windows::Win32::Foundation::RECT::default();
        let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(h, &mut r);
        Some((r.left + 60, r.top + 60))
    };
    let inst = unsafe {
        HINSTANCE(
            windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                .map(|m| m.0)
                .unwrap_or(std::ptr::null_mut()),
        )
    };
    crate::prompt_edit::open(
        inst,
        h,
        pos,
        kind,
        profiles,
        active_idx,
        None,
        Box::new(move |n, t| crate::prompt_edit::save_prompt_to_config(kind, n, t)),
        None,
    );
}

/// 既定ブラウザで指定URLを開く
fn open_url(h: HWND, url: &str) {
    unsafe {
        let wide_op = to_wide("open");
        let wide_url = to_wide(url);
        let _ = ShellExecuteW(
            Some(h),
            PCWSTR(wide_op.as_ptr()),
            PCWSTR(wide_url.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

fn save(h: HWND, ask_consent: bool) {
    let mut cfg = Config::load();
    cfg.hold_key = HOLD_KEYS[combo_sel(h, IDC_HOLDKEY).min(HOLD_KEYS.len() - 1)].to_string();
    cfg.poll_ms = get_ctl_text(h, IDC_POLL).trim().parse().unwrap_or(100).clamp(20, 1000);
    cfg.pin_hold_seconds = get_ctl_text(h, IDC_PIN_HOLD).trim().parse().unwrap_or(3);
    let hk = get_ctl_text(h, IDC_HOTKEY);
    if crate::config::parse_hotkey(&hk).is_some() {
        cfg.region_hotkey = hk.trim().to_string();
    }
    cfg.default_ocr = OCR_KEYS[combo_sel(h, IDC_OCR).min(OCR_KEYS.len() - 1)].to_string();
    cfg.default_translator = TR_KEYS[combo_sel(h, IDC_TR).min(TR_KEYS.len() - 1)].to_string();
    cfg.source_lang = LANGS[combo_sel(h, IDC_SRCLANG).min(LANGS.len() - 1)].to_string();
    cfg.target_lang = LANGS[combo_sel(h, IDC_LANG).min(LANGS.len() - 1)].to_string();
    cfg.deepl_key_enc = util::dpapi_encrypt(get_ctl_text(h, IDC_DEEPL).trim());
    cfg.google_key_enc = util::dpapi_encrypt(get_ctl_text(h, IDC_GOOGLE).trim());
    
    PROFILES.with(|p| {
        cfg.api_profiles = p.borrow().clone();
    });
    // 既定LLMプロファイルは default_api_profile にのみ反映する。active_api_profile
    // (セッションで使用中のプロファイル) には触れず、現行オーバーレイに波及させない。
    // 【既定にする】ボタンでのみ変更される DEFAULT_PROFILE をそのまま書き込む。
    let default_name = DEFAULT_PROFILE.with(|d| d.borrow().clone());
    if cfg.api_profiles.iter().any(|p| p.name == default_name) {
        cfg.default_api_profile = default_name;
    }
    
    cfg.autostart = check_get(h, IDC_AUTOSTART);
    cfg.perf_log = check_get(h, IDC_PERFLOG);
    cfg.log_enabled = check_get(h, IDC_LOG_ENABLED);
    cfg.debug_mode = check_get(h, IDC_DEBUG_MODE);
    cfg.detect_enabled = check_get(h, IDC_DETECT_MODE);
    cfg.detect_key = HOLD_KEYS[combo_sel(h, IDC_DETECT_KEY).min(HOLD_KEYS.len() - 1)].to_string();
    cfg.preview_detect_enabled = check_get(h, IDC_PREVIEW_DETECT_MODE);
    cfg.overlay_theme = THEME_KEYS[combo_sel(h, IDC_OVERLAY_THEME).min(THEME_KEYS.len() - 1)].to_string();
    cfg.log_max_records = get_ctl_text(h, IDC_LOG_MAX).trim().parse().unwrap_or(5000).clamp(100, 100000);
    cfg.llama_auto_start = check_get(h, IDC_LLAMA_AUTOSTART);
    cfg.llama_port = get_ctl_text(h, IDC_LLAMA_PORT).trim().parse().unwrap_or(crate::llama_server::DEFAULT_PORT).clamp(1024, 65535);
    cfg.llama_model_path = get_ctl_text(h, IDC_LLAMA_MODEL_PATH).trim().to_string();
    cfg.llama_mmproj_path = get_ctl_text(h, IDC_LLAMA_MMPROJ_PATH).trim().to_string();

    // 既定エンジンがクラウド/外部送信を伴う場合の同意確認 (SPEC §9)。
    // 即時保存化に伴い、既定エンジンのコンボを変更したときだけ確認する
    // (毎回の自動保存でダイアログを出さないため)。
    if ask_consent {
        confirm_default_consents(h, &mut cfg);
    }

    cfg.save();
    apply_autostart(cfg.autostart);
}

fn confirm_default_consents(h: HWND, cfg: &mut Config) {
    // LLM経由の場合、実際に使われるのは既定LLMプロファイル。ローカル(非外部URL)なら
    // 外部送信は発生しないため同意を求めない (SPECv0.5.3)。
    let llm_external = cfg
        .api_profiles
        .iter()
        .find(|p| p.name == cfg.default_api_profile)
        .is_none_or(|p| p.is_external());
    unsafe {
        let tr_external = match cfg.default_translator.as_str() {
            "deepl" | "google" => true,
            "llm" => llm_external,
            _ => false,
        };
        if tr_external && !cfg.consent_text {
            let r = MessageBoxW(
                Some(h),
                w!("既定の翻訳エンジンはOCR済みテキストを外部サービスへ送信します。許可しますか?"),
                w!("外部送信の同意"),
                MB_YESNO,
            );
            cfg.consent_text = r.0 == 6; // IDYES
        }
        if cfg.default_ocr == "llm" && llm_external && !cfg.consent_image {
            let r = MessageBoxW(
                Some(h),
                w!("既定のOCRエンジンはキャプチャ画像を外部サービスへ送信します。許可しますか?"),
                w!("外部送信の同意"),
                MB_YESNO,
            );
            cfg.consent_image = r.0 == 6;
        }
    }
}

fn apply_autostart(enable: bool) {
    unsafe {
        let key = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        let name = w!("FocusTranslator");
        if enable {
            if let Ok(exe) = std::env::current_exe() {
                let wide = to_wide(&exe.to_string_lossy());
                let _ = RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key,
                    name,
                    REG_SZ.0,
                    Some(wide.as_ptr() as *const _),
                    (wide.len() * 2) as u32,
                );
            }
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, key, name);
        }
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let notif = ((wparam.0 >> 16) & 0xFFFF) as u32;
            // 設定の即時保存 (SPECv0.4.7 改): コンボは選択変更時、チェックボックスは
            // クリック時、エディットはフォーカス喪失時に自動保存する。
            // プロファイル編集欄 (名前/種別/URL/キー/モデル) は【保存】ボタンで確定するため対象外。
            match id {
                IDC_HOLDKEY | IDC_DETECT_KEY | IDC_SRCLANG | IDC_LANG | IDC_OVERLAY_THEME
                    if notif == windows::Win32::UI::WindowsAndMessaging::CBN_SELCHANGE =>
                {
                    auto_save(h, false);
                }
                IDC_OCR | IDC_TR
                    if notif == windows::Win32::UI::WindowsAndMessaging::CBN_SELCHANGE =>
                {
                    // 既定エンジンの変更は外部送信の同意確認を伴う
                    auto_save(h, true);
                    update_explanations(h);
                }
                IDC_AUTOSTART | IDC_PERFLOG | IDC_LOG_ENABLED | IDC_DEBUG_MODE | IDC_DETECT_MODE
                | IDC_PREVIEW_DETECT_MODE | IDC_LLAMA_AUTOSTART
                    if notif == BN_CLICKED =>
                {
                    auto_save(h, false);
                }
                IDC_POLL | IDC_PIN_HOLD | IDC_HOTKEY | IDC_DEEPL | IDC_GOOGLE | IDC_LOG_MAX
                    if notif == EN_KILLFOCUS =>
                {
                    auto_save(h, false);
                }
                IDC_LLAMA_PORT | IDC_LLAMA_MODEL_PATH | IDC_LLAMA_MMPROJ_PATH
                    if notif == EN_KILLFOCUS =>
                {
                    auto_save(h, false);
                    refresh_llama_status(h);
                }
                _ => {}
            }
            match id {
                IDC_CLOSE => unsafe {
                    // WM_CLOSE 経由でモードAプロンプト編集ウィンドウの連動クローズ処理を通す
                    let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
                },
                IDC_CONSENT_RESET => {
                    let mut cfg = Config::load();
                    cfg.consent_text = false;
                    cfg.consent_image = false;
                    cfg.save();
                    unsafe {
                        let _ = PostMessageW(
                            Some(crate::app_state::main_hwnd()),
                            crate::app_state::WM_APP_CFG,
                            WPARAM(0),
                            LPARAM(0),
                        );
                        MessageBoxW(
                            Some(h),
                            w!("外部送信の同意状態をリセットしました。"),
                            crate::util::display_name_pcwstr(),
                            MB_OK | MB_ICONINFORMATION,
                        );
                    }
                }
                IDC_RESET_SETTINGS => unsafe {
                    let r = MessageBoxW(
                        Some(h),
                        w!("設定を初期状態にリセットします。この操作は元に戻せません。\nリセット後、アプリを自動的に再起動します。よろしいですか?"),
                        w!("設定のリセット"),
                        MB_YESNO | MB_ICONWARNING,
                    );
                    if r.0 == 6 {
                        // IDYES
                        let _ = std::fs::remove_file(Config::path());
                        if let Ok(exe) = std::env::current_exe() {
                            // 旧プロセスがミューテックスを解放し終える前に新プロセスが起動する
                            // 可能性があるため、新プロセス側で再試行させる (main.rs 参照)。
                            let _ = std::process::Command::new(exe).arg("--restart-wait").spawn();
                        }
                        CLOSING.with(|f| *f.borrow_mut() = true);
                        let _ = DestroyWindow(h);
                        let _ = DestroyWindow(crate::app_state::main_hwnd());
                    }
                },
                IDC_PADDLE_INSTALL => {
                    start_install(
                        h,
                        IDC_PADDLE_STATUS,
                        IDC_PADDLE_INSTALL,
                        WM_PADDLE_DONE,
                        "ダウンロード中…",
                        crate::paddle_install::install,
                    );
                }
                IDC_ONNX_INSTALL => {
                    start_install(
                        h,
                        IDC_ONNX_STATUS,
                        IDC_ONNX_INSTALL,
                        WM_ONNX_DONE,
                        "ダウンロード中…",
                        crate::onnx_translate_install::install,
                    );
                }
                IDC_LLAMA_BIN_INSTALL => {
                    start_install(
                        h,
                        IDC_LLAMA_BIN_STATUS,
                        IDC_LLAMA_BIN_INSTALL,
                        WM_LLAMA_BIN_DONE,
                        "ダウンロード中…",
                        crate::llama_install::install_binary,
                    );
                }
                IDC_LLAMA_MODEL_INSTALL => unsafe {
                    // 初回ダウンロード前に容量(約3GB)を警告し、同意を得てから開始する (SPECv0.5.2追補)。
                    let r = MessageBoxW(
                        Some(h),
                        w!("Gemma 4 E2Bモデルをダウンロードします。ファイルサイズは約3GBあり、回線速度によっては数分〜数十分かかります。\nダウンロードを開始しますか?"),
                        w!("モデルのダウンロード確認"),
                        MB_YESNO | MB_ICONWARNING,
                    );
                    if r == windows::Win32::UI::WindowsAndMessaging::IDYES {
                        start_model_install(h);
                    }
                },
                IDC_LLAMA_TOGGLE => {
                    let port: u32 = get_ctl_text(h, IDC_LLAMA_PORT).trim().parse().unwrap_or(crate::llama_server::DEFAULT_PORT);
                    if crate::llama_server::is_running(port) {
                        if let Err(e) = crate::llama_server::stop() {
                            unsafe {
                                let wide = to_wide(&e);
                                MessageBoxW(Some(h), PCWSTR(wide.as_ptr()), w!("サーバー停止エラー"), MB_OK);
                            }
                        }
                        refresh_llama_status(h);
                    } else {
                        let model = crate::llama_install::resolve_model_path(&get_ctl_text(h, IDC_LLAMA_MODEL_PATH));
                        // mmprojが存在すれば画像入力対応込みで起動する。無ければテキスト専用。
                        let mmproj = Some(crate::llama_install::resolve_mmproj_path(&get_ctl_text(h, IDC_LLAMA_MMPROJ_PATH)))
                            .filter(|p| p.is_file());
                        start_install(
                            h,
                            IDC_LLAMA_SERVER_STATUS,
                            IDC_LLAMA_TOGGLE,
                            WM_LLAMA_SERVER_DONE,
                            "起動中…",
                            move || crate::llama_server::start(port, &model, mmproj.as_deref()),
                        );
                    }
                }
                IDC_LLAMA_MODEL_BROWSE => {
                    if let Some(path) = browse_gguf_file(h, IDC_LLAMA_MODEL_PATH) {
                        set_ctl_text(h, IDC_LLAMA_MODEL_PATH, &path);
                        auto_save(h, false);
                        refresh_llama_status(h);
                    }
                }
                IDC_LLAMA_MMPROJ_BROWSE => {
                    if let Some(path) = browse_gguf_file(h, IDC_LLAMA_MMPROJ_PATH) {
                        set_ctl_text(h, IDC_LLAMA_MMPROJ_PATH, &path);
                        auto_save(h, false);
                        refresh_llama_status(h);
                    }
                }
                IDC_LLAMA_MMPROJ_INSTALL => unsafe {
                    let r = MessageBoxW(
                        Some(h),
                        w!("mmproj(画像入力対応)ファイルをダウンロードします。ファイルサイズは約550MBあります。\nダウンロードを開始しますか?"),
                        w!("mmprojのダウンロード確認"),
                        MB_YESNO | MB_ICONWARNING,
                    );
                    if r == windows::Win32::UI::WindowsAndMessaging::IDYES {
                        start_mmproj_install(h);
                    }
                },
                IDC_GITHUB_LINK => open_url(h, GITHUB_RELEASES_URL),
                IDC_DEEPL_URL => open_url(h, DEEPL_KEY_URL),
                IDC_GOOGLE_URL => open_url(h, GOOGLE_KEY_URL),
                IDC_PROMPT_TR_BTN => open_prompt_editor(h, crate::prompt_edit::PromptKind::Translate),
                IDC_PROMPT_OCR_BTN => open_prompt_editor(h, crate::prompt_edit::PromptKind::Ocr),
                IDC_PROMPT_EXP_BTN => open_prompt_editor(h, crate::prompt_edit::PromptKind::Explain),
                IDC_PROF_LIST | IDC_PROF_TYPE => {
                    if notif == windows::Win32::UI::WindowsAndMessaging::CBN_SELCHANGE {
                        if id == IDC_PROF_LIST {
                            load_profile_to_ui(h, combo_sel(h, IDC_PROF_LIST));
                            update_prof_default_btn(h);
                            // アクティブプロファイルの変更を即保存する
                            auto_save(h, false);
                        } else {
                            // 種別切替: モデル名・URLをその種別の既定値に置き換える
                            let t = &API_TYPE_ORDER[combo_sel(h, IDC_PROF_TYPE).min(API_TYPE_ORDER.len() - 1)];
                            set_ctl_text(h, IDC_PROF_MODEL, t.default_model());
                            set_ctl_text(h, IDC_PROF_URL, t.default_url());
                        }
                    }
                }
                IDC_PROF_NEW => {
                    set_ctl_text(h, IDC_PROF_NAME, "");
                    set_ctl_text(h, IDC_PROF_URL, "");
                    set_ctl_text(h, IDC_PROF_KEY, "");
                    set_ctl_text(h, IDC_PROF_MODEL, "");
                    combo_select(h, IDC_PROF_TYPE, 0);
                    // プロンプトはUI欄が無いため、保存時に既定値 (DEFAULT_GEMINI_*) を使う
                    PENDING_NEW.with(|f| *f.borrow_mut() = true);
                    update_prof_default_btn(h);
                }
                IDC_PROF_SAVE | IDC_PROF_SAVEAS => {
                    let _ = save_profile_from_ui(h, id == IDC_PROF_SAVEAS);
                    update_prof_default_btn(h);
                }
                IDC_PROF_DEL => {
                    let deleted = PROFILES.with(|p| {
                        let mut profiles = p.borrow_mut();
                        if profiles.len() <= 1 {
                            unsafe { MessageBoxW(Some(h), w!("最低1つは残す必要があります"), w!("エラー"), MB_OK); }
                            return false;
                        }
                        let sel = combo_sel(h, IDC_PROF_LIST);
                        if let Some(target) = profiles.get(sel) {
                            let msg = to_wide(&format!("プロファイル「{}」を削除しますか?", target.name));
                            let r = unsafe {
                                MessageBoxW(Some(h), PCWSTR(msg.as_ptr()), w!("プロファイルの削除"), MB_YESNO | MB_ICONWARNING)
                            };
                            if r.0 != 6 {
                                // IDYES 以外は削除しない
                                return false;
                            }
                        }
                        if sel < profiles.len() {
                            let removed_name = profiles[sel].name.clone();
                            profiles.remove(sel);
                            // 既定プロファイルを削除した場合は残りの先頭へ繰り上げる (宙に浮いた
                            // default_api_profile 参照によるLLM機能の全停止を防ぐ)。
                            if DEFAULT_PROFILE.with(|d| *d.borrow() == removed_name)
                                && let Some(first) = profiles.first()
                            {
                                DEFAULT_PROFILE.with(|d| *d.borrow_mut() = first.name.clone());
                            }
                            true
                        } else {
                            false
                        }
                    });
                    if deleted {
                        refill_profile_combo(h, 0);
                        load_profile_to_ui(h, 0);
                        update_prof_default_btn(h);
                        auto_save(h, false);
                    }
                }
                IDC_PROF_SET_DEFAULT => {
                    let sel = combo_sel(h, IDC_PROF_LIST);
                    let name = PROFILES.with(|p| p.borrow().get(sel).map(|x| x.name.clone()));
                    if let Some(name) = name {
                        DEFAULT_PROFILE.with(|d| *d.borrow_mut() = name);
                        refill_profile_combo(h, sel);
                        update_prof_default_btn(h);
                        auto_save(h, false);
                    }
                }
                IDC_OPEN_LOG => {
                    let inst = unsafe {
                        windows::Win32::Foundation::HINSTANCE(
                            windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                                .map(|m| m.0)
                                .unwrap_or(std::ptr::null_mut()),
                        )
                    };
                    crate::logviewer::open(inst);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_PADDLE_DONE => {
            handle_install_done(
                h,
                wparam,
                lparam,
                refresh_paddle_status,
                "PaddleOCRのモデルをインストールしました。",
            );
            LRESULT(0)
        }
        WM_ONNX_DONE => {
            handle_install_done(
                h,
                wparam,
                lparam,
                refresh_onnx_status,
                "ローカルONNX翻訳モデルをインストールしました。",
            );
            LRESULT(0)
        }
        WM_LLAMA_BIN_DONE => {
            handle_install_done(h, wparam, lparam, refresh_llama_status, "llama.cppをインストールしました。");
            ensure_local_llm_profile_if_ready(h);
            LRESULT(0)
        }
        WM_LLAMA_MODEL_PROGRESS => {
            let label = unsafe { *Box::from_raw(lparam.0 as *mut String) };
            set_ctl_text(h, IDC_LLAMA_MODEL_STATUS, &label);
            LRESULT(0)
        }
        WM_LLAMA_MODEL_DONE => {
            if wparam.0 == 1 {
                // ダウンロード先(既定の管理下ディレクトリ)のパスをテキストボックスへ明示反映する
                // (SPECv0.5.2追補: 起動時にもこのパスがそのまま使われる)。
                let path = crate::llama_install::model_path();
                set_ctl_text(h, IDC_LLAMA_MODEL_PATH, &path.to_string_lossy());
                auto_save(h, false);
            }
            handle_install_done(h, wparam, lparam, refresh_llama_status, "Gemma 4 E2Bモデルを導入しました。");
            ensure_local_llm_profile_if_ready(h);
            LRESULT(0)
        }
        WM_LLAMA_SERVER_DONE => {
            handle_install_done(h, wparam, lparam, refresh_llama_status, "サーバーを起動しました。");
            LRESULT(0)
        }
        WM_LLAMA_MMPROJ_PROGRESS => {
            let label = unsafe { *Box::from_raw(lparam.0 as *mut String) };
            set_ctl_text(h, IDC_LLAMA_MMPROJ_STATUS, &label);
            LRESULT(0)
        }
        WM_LLAMA_MMPROJ_DONE => {
            if wparam.0 == 1 {
                // ダウンロード先(既定の管理下ディレクトリ)のパスをテキストボックスへ明示反映する
                // (SPECv0.5.2追補: 起動時にもこのパスがそのまま使われる)。
                set_ctl_text(h, IDC_LLAMA_MMPROJ_PATH, &crate::llama_install::mmproj_path().to_string_lossy());
                auto_save(h, false);
            }
            handle_install_done(h, wparam, lparam, refresh_llama_status, "mmproj(画像入力対応)ファイルを導入しました。");
            LRESULT(0)
        }
        WM_CLOSE => {
            // モードAのプロンプト編集ウィンドウを連動して閉じる (SPECv0.4.7)。
            // 未保存テンプレートの破棄をユーザーがキャンセルしたら設定画面も閉じない。
            if !crate::prompt_edit::close_for_settings() {
                return LRESULT(0);
            }
            CLOSING.with(|f| *f.borrow_mut() = true);
            unsafe {
                let _ = DestroyWindow(h);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            WND.with(|w| *w.borrow_mut() = 0);
            CLOSING.with(|f| *f.borrow_mut() = false);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(h, msg, wparam, lparam) },
    }
}
