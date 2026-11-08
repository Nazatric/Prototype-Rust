//! GPU-independent status-screen blitter (Android).
//!
//! `wgpu` needs a working Vulkan driver and a *configured* surface.  If
//! GPU bring-up fails the app used to die (or show an empty black
//! surface) before the user could even see the "Allow all files access"
//! prompt.  This module is the FALLBACK for exactly that case: it writes
//! the pre-rendered RGBA screen **directly into the ANativeWindow
//! buffer** via the NDK's `ANativeWindow_lock` / `unlockAndPost` — the
//! same mechanism NativeActivity used before GLES or Vulkan existed.
//! No adapter, device, surface, pipeline or shader is involved.
//!
//! IMPORTANT: `ANativeWindow_lock` connects a CPU producer
//! (`NATIVE_WINDOW_API_CPU`) that a later Vulkan swapchain cannot
//! displace, so [`crate::app::App`] tries wgpu FIRST and only calls this
//! blitter after GPU bring-up has definitively failed.  The two
//! producers are never mixed on one surface.
//!
//! # Safety
//!
//! Raw `ANativeWindow` FFI; all unsafe code is confined to the
//! `#[cfg(target_os = "android")]` implementation below.  On desktop a
//! no-op stub keeps the shared [`crate::app::App`] compiling unchanged.

#![allow(unsafe_code)]

/// Draw `rgba` (tightly packed `sw × sh` RGBA8) into `window`'s native
/// buffer, aspect-preserving with dark letterbox bars.  Returns `false`
/// when there is no usable native window (caller keeps retrying).
#[cfg(not(target_os = "android"))]
pub fn draw(_window: &winit::window::Window, _rgba: &[u8], _sw: u32, _sh: u32) -> bool {
    false
}

/// `ANativeWindow_Buffer` from `<android/native_window.h>`.
#[cfg(target_os = "android")]
#[repr(C)]
struct NativeWindowBuffer {
    width: i32,
    height: i32,
    /// Stride in PIXELS (not bytes) for RGBA_8888.
    stride: i32,
    format: i32,
    bits: *mut std::ffi::c_void,
    reserved: [u32; 6],
}

/// `AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM` / `WINDOW_FORMAT_RGBA_8888`.
#[cfg(target_os = "android")]
const RGBA_8888: i32 = 1;

#[cfg(target_os = "android")]
#[link(name = "android")]
extern "C" {
    fn ANativeWindow_setBuffersGeometry(
        window: *mut std::ffi::c_void,
        width: i32,
        height: i32,
        format: i32,
    ) -> i32;
    fn ANativeWindow_lock(
        window: *mut std::ffi::c_void,
        out_buffer: *mut NativeWindowBuffer,
        dirty_bounds: *mut std::ffi::c_void,
    ) -> i32;
    fn ANativeWindow_unlockAndPost(window: *mut std::ffi::c_void) -> i32;
}

/// Draw `rgba` (tightly packed `sw × sh` RGBA8) into `window`'s native
/// buffer, aspect-preserving with dark letterbox bars.  Returns `false`
/// when there is no usable native window (caller keeps retrying).
#[cfg(target_os = "android")]
pub fn draw(window: &winit::window::Window, rgba: &[u8], sw: u32, sh: u32) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::ffi::c_void;

    let Ok(handle) = window.window_handle() else {
        return false;
    };
    let RawWindowHandle::AndroidNdk(h) = handle.as_raw() else {
        return false;
    };
    let native = h.a_native_window.as_ptr() as *mut c_void;
    if native.is_null() || rgba.len() < (sw * sh * 4) as usize || sw == 0 || sh == 0 {
        return false;
    }

    // SAFETY: `native` came from winit's raw window handle and lives as
    // long as the Window we were handed.  We hold the lock only for the
    // duration of the copy and always unlock before returning.
    unsafe {
        // 0×0 = "use the window's natural buffer size"; format = RGBA8.
        let _ = ANativeWindow_setBuffersGeometry(native, 0, 0, RGBA_8888);
        let mut buf: NativeWindowBuffer = std::mem::zeroed();
        if ANativeWindow_lock(native, &mut buf, std::ptr::null_mut()) != 0 {
            return false;
        }
        let ok = blit_locked(&buf, rgba, sw, sh);
        let _ = ANativeWindow_unlockAndPost(native);
        ok
    }
}

/// Copy the source image into an already-locked RGBA_8888 buffer.
///
/// # Safety
/// `buf.bits` must point at a locked buffer of at least
/// `buf.height × buf.stride × 4` writable bytes.
#[cfg(target_os = "android")]
unsafe fn blit_locked(buf: &NativeWindowBuffer, rgba: &[u8], sw: u32, sh: u32) -> bool {
    let bw = buf.width.max(0) as u32;
    let bh = buf.height.max(0) as u32;
    let stride = buf.stride.max(0) as u32;
    if bw == 0 || bh == 0 || stride == 0 || buf.bits.is_null() || buf.format != RGBA_8888 {
        return false;
    }
    let dst = buf.bits as *mut u8;
    let scale = (bw as f64 / sw as f64).min(bh as f64 / sh as f64);
    let dw = ((sw as f64) * scale).max(1.0) as u32;
    let dh = ((sh as f64) * scale).max(1.0) as u32;
    let ox = bw.saturating_sub(dw) / 2;
    let oy = bh.saturating_sub(dh) / 2;

    for y in 0..bh {
        let row = (y as usize) * (stride as usize) * 4;
        for x in 0..bw {
            let di = row + (x as usize) * 4;
            let (r, g, b, a) = if x >= ox && x < ox + dw && y >= oy && y < oy + dh {
                let sx = ((x - ox) as u64 * sw as u64 / dw as u64).min(sw as u64 - 1) as u32;
                let sy = ((y - oy) as u64 * sh as u64 / dh as u64).min(sh as u64 - 1) as u32;
                let si = ((sy * sw + sx) * 4) as usize;
                (rgba[si], rgba[si + 1], rgba[si + 2], rgba[si + 3])
            } else {
                (14u8, 18u8, 26u8, 255u8)
            };
            // SAFETY: `di` stays inside `height × stride × 4` bytes of
            // the locked buffer (stride >= width by the NDK contract).
            unsafe {
                *dst.add(di) = r;
                *dst.add(di + 1) = g;
                *dst.add(di + 2) = b;
                *dst.add(di + 3) = a;
            }
        }
    }
    true
}
