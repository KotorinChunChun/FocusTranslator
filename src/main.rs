// Focus Translator v0.3 — カーソル位置翻訳ツール (FocusTranslator_SPECv0.3.md 準拠)
// 右Ctrlホールド中のみ、マウスポインタ直下のテキスト1行を認識・翻訳して
// カーソル近傍にオーバーレイ表示するタスクトレイ常駐ツール。
#![windows_subsystem = "windows"]

mod app_state;
mod capture;
mod capture_plan;
mod chip_handler;
mod config;
mod detect;
mod engine;
mod image_edit;
mod llm_api;
mod logdb;
mod logviewer;
mod ocr;
mod oneocr;
mod onnx_translate;
mod onnx_translate_install;
mod overlay;
mod overlay_layout;
mod paddle_install;
mod paddle_ocr;
mod prompt_edit;
mod region;
mod settings;
#[cfg(test)]
mod test_util;
mod translate;
mod tray;
mod uia;
mod ui_helpers;
mod util;
mod worker;
mod input_dialog;

use config::Config;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HINSTANCE,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, RegisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DispatchMessageW, GetMessageW, IsDialogMessageW,
    MB_ICONWARNING, MB_OK, MSG, MessageBoxW, RegisterClassW, SetTimer, TranslateMessage,
    WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::w;



fn main() {
    // 多重起動防止
    // 設定リセットによる自己再起動 (--restart-wait) は、旧プロセスがミューテックスを
    // 解放し終える前に起動してしまう可能性があるため、既存プロセスありと判定されても
    // 短時間リトライしてから諦める。通常の多重起動時はリトライ対象外で即終了する。
    let restart_wait = std::env::args().any(|a| a == "--restart-wait");
    unsafe {
        let mut mutex = CreateMutexW(None, false, w!("FocusTranslator_Singleton"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // 重複と判定されたハンドルはこの時点で閉じる。閉じずに次のCreateMutexWで
            // 上書きすると、そのハンドル自体がミューテックスを生かし続けてしまい、
            // 旧プロセスがとっくに終了していても ERROR_ALREADY_EXISTS が解消されない
            // (自分自身の未クローズハンドルによる自己デッドロック)。
            if let Ok(h) = mutex {
                let _ = CloseHandle(h);
            }
            if !restart_wait {
                return;
            }
            let mut acquired = false;
            for _ in 0..20 {
                std::thread::sleep(std::time::Duration::from_millis(250));
                mutex = CreateMutexW(None, false, w!("FocusTranslator_Singleton"));
                if GetLastError() != ERROR_ALREADY_EXISTS {
                    acquired = true;
                    break;
                }
                if let Ok(h) = mutex {
                    let _ = CloseHandle(h);
                }
            }
            if !acquired {
                return;
            }
        }
        let _ = mutex;
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    std::panic::set_hook(Box::new(|info| {
        util::app_log(&format!("panic: {info}"));
    }));

    let instance: HINSTANCE = unsafe {
        HINSTANCE(GetModuleHandleW(None).map(|m| m.0).unwrap_or(std::ptr::null_mut()))
    };
    let mut cfg = Config::load();

    // メイン(非表示)ウィンドウ
    let main = unsafe {
        let class = w!("FocusTranslatorMain");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(app_state::wndproc),
            hInstance: instance,
            hIcon: app_state::app_icon(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        CreateWindowExW(
            Default::default(),
            class,
            w!("FocusTranslator"),
            WS_OVERLAPPED,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
        .expect("メインウィンドウの作成に失敗")
    };

    let overlay = overlay::create(instance);
    tray::add_icon(main);

    // 範囲指定ホットキー (SPEC §11: 衝突時は通知)
    let (mods, vk) = cfg.region_hotkey_parsed();
    unsafe {
        if RegisterHotKey(Some(main), app_state::HOTKEY_REGION, HOT_KEY_MODIFIERS(mods), vk).is_err() {
            MessageBoxW(
                Some(main),
                w!("範囲指定ホットキーを登録できませんでした。他のアプリと衝突しています。設定画面で変更してください。"),
                w!("Focus Translator"),
                MB_OK | MB_ICONWARNING,
            );
        }
        SetTimer(Some(main), app_state::TIMER_POLL, cfg.poll_ms, None);
    }

    if !cfg.first_launch_done {
        unsafe {
            let msg = windows::core::HSTRING::from("初回起動です。\n高精度な画面認識(PaddleOCR)と、ONNX翻訳モデルをダウンロードしますか？\n（※画面認識は標準でOneOCR(Windows 11内蔵)を使用します。ONNXを導入しないとローカル翻訳ができません）");
            let title = w!("Focus Translator - 初回セットアップ");
            let result = MessageBoxW(Some(main), &msg, title, windows::Win32::UI::WindowsAndMessaging::MB_YESNO | windows::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION);
            if result == windows::Win32::UI::WindowsAndMessaging::IDYES {
                settings::open(instance, main);
            }
        }
        cfg.first_launch_done = true;
        cfg.save();
    }

    app_state::init(cfg, instance, main, overlay);

    // メッセージループ
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if settings::is_open() && IsDialogMessageW(settings::hwnd(), &msg).as_bool() {
                continue;
            }
            if logviewer::is_open() && IsDialogMessageW(logviewer::hwnd(), &msg).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
