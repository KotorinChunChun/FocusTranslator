// 接続されているGPUベンダーの検出 (SPECv0.5.5: llama.cppインストール時のビルド種別
// 推奨表示 [CUDA/Radeon] に使う)。DXGIアダプター列挙のみで判定し、ドライバの詳細
// (CUDA/ROCmのランタイムが実際に入っているか等)までは見ない — あくまで「対応しうる
// GPUが挿さっているか」の目安であり、最終的な動作可否はユーザー自身の判断に委ねる。
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};

const VENDOR_NVIDIA: u32 = 0x10DE;
const VENDOR_AMD_1: u32 = 0x1002;
const VENDOR_AMD_2: u32 = 0x1022;

#[derive(Clone, Copy, Default)]
pub struct GpuInfo {
    pub nvidia: bool,
    pub amd: bool,
}

/// 接続中のGPUベンダーを検出する。取得自体に失敗した場合は両方falseを返す
/// (未検出として扱い、呼び出し側はCPU版を既定候補にする)。
pub fn detect() -> GpuInfo {
    let mut info = GpuInfo::default();
    let factory: windows::core::Result<IDXGIFactory1> = unsafe { CreateDXGIFactory1() };
    let Ok(factory) = factory else {
        return info;
    };
    let mut i = 0u32;
    loop {
        let adapter = unsafe { factory.EnumAdapters1(i) };
        let Ok(adapter) = adapter else { break };
        if let Ok(desc) = unsafe { adapter.GetDesc1() } {
            match desc.VendorId {
                VENDOR_NVIDIA => info.nvidia = true,
                VENDOR_AMD_1 | VENDOR_AMD_2 => info.amd = true,
                _ => {}
            }
        }
        i += 1;
    }
    info
}
