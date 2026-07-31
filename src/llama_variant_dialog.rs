// llama.cppインストール時のビルド種別選択ダイアログ (SPECv0.5.5)
// CPU/CUDA(NVIDIA)/Radeon(AMD)から選ばせ、検出したGPUベンダーに応じて推奨マークを付ける。
// input_dialog.rs と同じ「専用ウィンドウクラス+モーダルメッセージループ」パターンで実装する。
use crate::gpu_detect::GpuInfo;
use crate::llama_install::LlamaVariant;
use crate::util::to_wide;
use std::cell::RefCell;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CLIP_DEFAULT_PRECIS, COLOR_BTNFACE, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH,
    DEFAULT_QUALITY, FW_NORMAL, OUT_DEFAULT_PRECIS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BS_AUTORADIOBUTTON, CreateWindowExW, DefWindowProcW,
    DestroyWindow, DispatchMessageW, GetMessageW, GetWindowRect, HMENU, IDC_ARROW, IsWindow,
    LoadCursorW, MSG, RegisterClassW, SW_SHOW, SendMessageW, SetForegroundWindow, ShowWindow,
    TranslateMessage, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_KEYDOWN, WM_SETFONT,
    WNDCLASSW, WS_CAPTION, WS_CHILD, WS_EX_TOPMOST, WS_GROUP, WS_POPUPWINDOW, WS_SYSMENU,
    WS_TABSTOP, WS_VISIBLE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN};
use windows::core::{PCWSTR, w};

const IDC_RADIO_CPU: i32 = 201;
const IDC_RADIO_CUDA: i32 = 202;
const IDC_RADIO_RADEON: i32 = 203;
const IDC_OK: i32 = 204;
const IDC_CANCEL: i32 = 205;
const DLG_W: i32 = 420;
const DLG_H: i32 = 250;

thread_local! {
    static RESULT: RefCell<Option<LlamaVariant>> = const { RefCell::new(None) };
    static HAS_RESULT: RefCell<bool> = const { RefCell::new(false) };
    static CLOSED_WITHOUT_RESULT: RefCell<bool> = const { RefCell::new(false) };
}

/// ビルド種別選択ダイアログをモーダル表示する。「OK」で選択した種別、「キャンセル」/×でNone。
pub fn show(parent: HWND, gpu: GpuInfo) -> Option<LlamaVariant> {
    RESULT.with(|r| *r.borrow_mut() = None);
    HAS_RESULT.with(|r| *r.borrow_mut() = false);
    CLOSED_WITHOUT_RESULT.with(|r| *r.borrow_mut() = false);

    unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class_name = w!("FocusTranslatorLlamaVariantClass");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH((COLOR_BTNFACE.0 + 1) as usize as *mut _),
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);

        let (x, y) = {
            let mut r = RECT::default();
            if GetWindowRect(parent, &mut r).is_ok() {
                (r.left + (r.right - r.left - DLG_W) / 2, r.top + (r.bottom - r.top - DLG_H) / 2)
            } else {
                (200, 200)
            }
        };

        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOPMOST,
            class_name,
            w!("llama.cppのビルド種別を選択"),
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
            return None;
        };

        let _ = EnableWindow(parent, false);

        let font = CreateFontW(
            18, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS, DEFAULT_QUALITY, DEFAULT_PITCH.0 as u32, w!("Meiryo"),
        );

        let mark = |present: bool| if present { "○ 対応GPUを検出" } else { "✕ 対応GPU未検出" };
        let radios: [(i32, String, bool); 3] = [
            (IDC_RADIO_CPU, "CPU版 (全ての環境で動作)".to_string(), false),
            (IDC_RADIO_CUDA, format!("CUDA版 (NVIDIA GPU)  {}", mark(gpu.nvidia)), gpu.nvidia),
            (IDC_RADIO_RADEON, format!("Radeon版 (AMD GPU, HIP)  {}", mark(gpu.amd)), gpu.amd),
        ];
        // 既定選択: NVIDIA検出ならCUDA、AMD検出ならRadeon、どちらも無ければCPU
        let default_id = if gpu.nvidia {
            IDC_RADIO_CUDA
        } else if gpu.amd {
            IDC_RADIO_RADEON
        } else {
            IDC_RADIO_CPU
        };

        let mut ry = 16;
        for (i, (id, label, _)) in radios.iter().enumerate() {
            let style = WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_AUTORADIOBUTTON as u32)
                | if i == 0 { WS_GROUP } else { WINDOW_STYLE(0) };
            let wide = to_wide(label);
            let radio = CreateWindowExW(
                Default::default(),
                w!("BUTTON"),
                PCWSTR(wide.as_ptr()),
                style,
                16,
                ry,
                DLG_W - 48,
                24,
                Some(hwnd),
                Some(HMENU(*id as *mut _)),
                Some(instance.into()),
                None,
            )
            .unwrap();
            let _ = SendMessageW(radio, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(0)));
            if *id == default_id {
                let _ = SendMessageW(radio, BM_SETCHECK, Some(WPARAM(1)), Some(LPARAM(0)));
            }
            ry += 34;
        }

        let ok_btn = CreateWindowExW(
            Default::default(),
            w!("BUTTON"),
            w!("OK"),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP,
            DLG_W - 190,
            ry + 10,
            80,
            32,
            Some(hwnd),
            Some(HMENU(IDC_OK as *mut _)),
            Some(instance.into()),
            None,
        )
        .unwrap();
        let _ = SendMessageW(ok_btn, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(0)));

        let cancel_btn = CreateWindowExW(
            Default::default(),
            w!("BUTTON"),
            w!("キャンセル"),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP,
            DLG_W - 100,
            ry + 10,
            80,
            32,
            Some(hwnd),
            Some(HMENU(IDC_CANCEL as *mut _)),
            Some(instance.into()),
            None,
        )
        .unwrap();
        let _ = SendMessageW(cancel_btn, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(0)));

        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);

        let mut msg = MSG::default();
        while IsWindow(Some(hwnd)).as_bool() && GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_DESTROY && msg.hwnd == hwnd {
                break;
            }
            if msg.message == WM_KEYDOWN {
                let key = msg.wParam.0 as u16;
                if key == VK_RETURN.0 {
                    let _ = SendMessageW(hwnd, WM_COMMAND, Some(WPARAM(IDC_OK as usize)), Some(LPARAM(0)));
                    continue;
                } else if key == VK_ESCAPE.0 {
                    let _ = SendMessageW(hwnd, WM_COMMAND, Some(WPARAM(IDC_CANCEL as usize)), Some(LPARAM(0)));
                    continue;
                }
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            if HAS_RESULT.with(|r| *r.borrow()) || CLOSED_WITHOUT_RESULT.with(|r| *r.borrow()) {
                let _ = DestroyWindow(hwnd);
                HAS_RESULT.with(|r| *r.borrow_mut() = false);
                CLOSED_WITHOUT_RESULT.with(|r| *r.borrow_mut() = false);
            }
        }

        let _ = EnableWindow(parent, true);
        let _ = SetForegroundWindow(parent);
    }
    RESULT.with(|r| r.borrow_mut().take())
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            if id == IDC_OK {
                unsafe {
                    let checked = |ctl_id: i32| -> bool {
                        let h = windows::Win32::UI::WindowsAndMessaging::GetDlgItem(Some(hwnd), ctl_id).unwrap_or_default();
                        SendMessageW(h, BM_GETCHECK, Some(WPARAM(0)), Some(LPARAM(0))).0 == 1
                    };
                    let variant = if checked(IDC_RADIO_CUDA) {
                        LlamaVariant::Cuda
                    } else if checked(IDC_RADIO_RADEON) {
                        LlamaVariant::Radeon
                    } else {
                        LlamaVariant::Cpu
                    };
                    RESULT.with(|r| *r.borrow_mut() = Some(variant));
                    HAS_RESULT.with(|r| *r.borrow_mut() = true);
                }
                return LRESULT(0);
            } else if id == IDC_CANCEL {
                CLOSED_WITHOUT_RESULT.with(|r| *r.borrow_mut() = true);
                return LRESULT(0);
            }
        }
        WM_CLOSE => {
            CLOSED_WITHOUT_RESULT.with(|r| *r.borrow_mut() = true);
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
