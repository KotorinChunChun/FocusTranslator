// Windows.Graphics.Capture によるポインタ直下ウィンドウのキャプチャ (SPEC §5)
// BitBlt フォールバックは行わない。失敗時はエラーを返すのみ。
use std::time::Duration;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, HWND, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
use windows::core::Interface;

/// BGRA 生画像
#[derive(Clone)]
pub struct Captured {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// ウィンドウの画面上の矩形(DWM拡張フレーム境界を優先)
pub fn window_frame_rect(hwnd: HWND) -> RECT {
    unsafe {
        let mut r = RECT::default();
        let ok = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        );
        if ok.is_ok() {
            return r;
        }
        let _ = GetWindowRect(hwnd, &mut r);
        r
    }
}

/// HWND を WGC でキャプチャして1フレーム取得する
pub fn capture_window(hwnd: HWND) -> Result<Captured, String> {
    unsafe {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .map_err(|e| format!("D3D11デバイス作成失敗: {e}"))?;
        let device = device.ok_or("D3D11デバイスなし")?;
        let context = context.ok_or("D3D11コンテキストなし")?;

        let dxgi: IDXGIDevice = device.cast().map_err(|e| format!("DXGI取得失敗: {e}"))?;
        let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)
            .map_err(|e| format!("WinRTデバイス作成失敗: {e}"))?;
        let d3d_device: IDirect3DDevice = inspectable
            .cast()
            .map_err(|e| format!("IDirect3DDevice変換失敗: {e}"))?;

        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
            .map_err(|e| format!("WGC interop取得失敗: {e}"))?;
        let item: GraphicsCaptureItem = interop
            .CreateForWindow(hwnd)
            .map_err(|_| "このウィンドウは取得できません".to_string())?;
        let size = item.Size().map_err(|e| format!("サイズ取得失敗: {e}"))?;
        if size.Width <= 0 || size.Height <= 0 {
            return Err("このウィンドウは取得できません".into());
        }

        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &d3d_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )
        .map_err(|e| format!("フレームプール作成失敗: {e}"))?;
        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|_| "このウィンドウは取得できません".to_string())?;
        let _ = session.SetIsCursorCaptureEnabled(false);
        // 黄色い枠を消す(Win11では利用可能。失敗しても続行)
        let _ = session.SetIsBorderRequired(false);
        session
            .StartCapture()
            .map_err(|_| "このウィンドウは取得できません".to_string())?;

        // フレーム到着をポーリングで待つ(最大 ~600ms)
        let mut frame = None;
        for _ in 0..120 {
            if let Ok(f) = pool.TryGetNextFrame() {
                frame = Some(f);
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let result = (|| -> Result<Captured, String> {
            let frame = frame.ok_or("フレーム取得に失敗しました")?;
            let surface = frame.Surface().map_err(|e| format!("Surface取得失敗: {e}"))?;
            let access: IDirect3DDxgiInterfaceAccess = surface
                .cast()
                .map_err(|e| format!("DXGIアクセス失敗: {e}"))?;
            let tex: ID3D11Texture2D = access
                .GetInterface()
                .map_err(|e| format!("テクスチャ取得失敗: {e}"))?;

            let mut desc = D3D11_TEXTURE2D_DESC::default();
            tex.GetDesc(&mut desc);
            let mut sdesc = desc;
            sdesc.Usage = D3D11_USAGE_STAGING;
            sdesc.BindFlags = 0;
            sdesc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            sdesc.MiscFlags = 0;

            let mut staging: Option<ID3D11Texture2D> = None;
            device
                .CreateTexture2D(&sdesc, None, Some(&mut staging))
                .map_err(|e| format!("ステージング作成失敗: {e}"))?;
            let staging = staging.ok_or("ステージングなし")?;
            context.CopyResource(&staging, &tex);

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| format!("Map失敗: {e}"))?;
            let w = desc.Width;
            let h = desc.Height;
            let mut bgra = vec![0u8; (w * h * 4) as usize];
            let src = mapped.pData as *const u8;
            for row in 0..h {
                let s = src.add((row * mapped.RowPitch) as usize);
                let d = bgra.as_mut_ptr().add((row * w * 4) as usize);
                std::ptr::copy_nonoverlapping(s, d, (w * 4) as usize);
            }
            context.Unmap(&staging, 0);
            Ok(Captured { width: w, height: h, bgra })
        })();

        // セッション停止(Close)。失敗は無視。
        let _ = session.Close();
        let _ = pool.Close();
        result
    }
}

/// 画像から矩形を切り出す(範囲は自動でクランプ)
pub fn crop(img: &Captured, x: i32, y: i32, w: i32, h: i32) -> Option<Captured> {
    let x0 = x.max(0) as u32;
    let y0 = y.max(0) as u32;
    if x0 >= img.width || y0 >= img.height {
        return None;
    }
    let x1 = ((x + w).max(0) as u32).min(img.width);
    let y1 = ((y + h).max(0) as u32).min(img.height);
    if x1 <= x0 + 4 || y1 <= y0 + 4 {
        return None;
    }
    let cw = x1 - x0;
    let ch = y1 - y0;
    let mut out = vec![0u8; (cw * ch * 4) as usize];
    for row in 0..ch {
        let s = ((y0 + row) * img.width + x0) as usize * 4;
        let d = (row * cw) as usize * 4;
        out[d..d + (cw * 4) as usize].copy_from_slice(&img.bgra[s..s + (cw * 4) as usize]);
    }
    Some(Captured { width: cw, height: ch, bgra: out })
}

/// BGRA → PNG エンコード(外部OCR/Gemini送信用)
pub fn to_png(img: &Captured) -> Vec<u8> {
    let mut rgba = vec![0u8; img.bgra.len()];
    for i in (0..img.bgra.len()).step_by(4) {
        rgba[i] = img.bgra[i + 2];
        rgba[i + 1] = img.bgra[i + 1];
        rgba[i + 2] = img.bgra[i];
        rgba[i + 3] = 255;
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width, img.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        if let Ok(mut writer) = enc.write_header() {
            let _ = writer.write_image_data(&rgba);
        }
    }
    out
}
