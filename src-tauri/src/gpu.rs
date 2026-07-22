//! GPU capability probe (DXGI + D3D12) — decides whether the 3B reformat model
//! is offered. Called once at startup; ANY COM failure yields a safe no-GPU
//! answer instead of panicking (this runs before the UI exists).
//!
//! Required `windows` crate features (INTEGRATION owner adds these to Cargo.toml
//! alongside the existing list — additive, do not remove any):
//!   Win32_Graphics_Dxgi       — CreateDXGIFactory1, IDXGIFactory1, IDXGIAdapter1,
//!                               DXGI_ADAPTER_DESC1, DXGI_ADAPTER_FLAG_SOFTWARE
//!   Win32_Graphics_Direct3D   — D3D_FEATURE_LEVEL_11_0
//!   Win32_Graphics_Direct3D12 — D3D12CreateDevice, ID3D12Device
//!   Win32_Foundation          — LUID inside DXGI_ADAPTER_DESC1 (ALREADY enabled)

use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{D3D12CreateDevice, ID3D12Device};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};

pub struct GpuInfo {
    pub vram_mb: u64,
    pub offer_gpu_3b: bool,
}

/// Largest dedicated-VRAM real GPU decides the gate. Never panics.
pub fn probe() -> GpuInfo {
    let vram_mb = unsafe { max_gpu_vram_mb() }.unwrap_or(0);
    GpuInfo { vram_mb, offer_gpu_3b: gate(vram_mb) }
}

// ponytail: heuristic knob, retune if a fleet machine mis-gates. iGPUs report
// only a tiny dedicated carve-out (128–512MB) so fall below 4GB automatically.
fn gate(vram_mb: u64) -> bool {
    vram_mb >= 4096
}

/// None if any COM call fails — treated as "no capable GPU" by `probe()`.
unsafe fn max_gpu_vram_mb() -> Option<u64> {
    let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
    let mut best: usize = 0;
    let mut i = 0u32;
    // EnumAdapters1 returns Err (DXGI_ERROR_NOT_FOUND) past the last adapter.
    while let Ok(adapter) = factory.EnumAdapters1(i) {
        i += 1;
        let Ok(desc) = adapter.GetDesc1() else { continue };
        // Skip WARP/software renderers by bit test — no fragile name matching.
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        // "Real modern GPU" signal: must create a device at FL 11_0, then drop.
        let mut device: Option<ID3D12Device> = None;
        if D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device).is_err() {
            continue;
        }
        best = best.max(desc.DedicatedVideoMemory);
    }
    Some((best / (1024 * 1024)) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_threshold() {
        assert!(!gate(0));
        assert!(!gate(512)); // iGPU carve-out
        assert!(!gate(4095));
        assert!(gate(4096)); // exactly the 4GB line
        assert!(gate(8192));
    }

    #[test]
    fn probe_is_sane() {
        // Hardware-dependent: must not panic and the gate must be applied
        // consistently to whatever VRAM this machine reports.
        let info = probe();
        assert_eq!(info.offer_gpu_3b, gate(info.vram_mb));
        assert!(info.vram_mb > 0, "expected a real display adapter with dedicated VRAM");
    }
}
