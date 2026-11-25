//! macOS backend. M0 status: displays and the Screen Recording permission are
//! real (CoreGraphics); recording is not implemented yet. The capture work
//! (ScreenCaptureKit + AVFoundation + AVAssetWriter in a Swift shim) is the M0
//! spike in docs/08-roadmap.md.

use std::ffi::c_void;

use lumos_model::sources::{Device, Display, PermissionState, PermissionStatus, Rect};
use lumos_model::{ToolError, ToolResult};

use crate::{CaptureBackend, CaptureSession, SessionConfig};

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayBounds(display: u32) -> CGRect;
    fn CGDisplayCopyDisplayMode(display: u32) -> *mut c_void;
    fn CGDisplayModeGetPixelWidth(mode: *mut c_void) -> usize;
    fn CGDisplayModeGetRefreshRate(mode: *mut c_void) -> f64;
    fn CGDisplayModeRelease(mode: *mut c_void);
    fn CGPreflightScreenCaptureAccess() -> bool;
}

pub struct MacBackend;

#[async_trait::async_trait]
impl CaptureBackend for MacBackend {
    fn name(&self) -> &'static str {
        "macos"
    }
    fn can_record(&self) -> bool {
        false
    }
    async fn permissions(&self) -> PermissionStatus {
        // SAFETY: plain C call with no arguments.
        let screen = if unsafe { CGPreflightScreenCaptureAccess() } { PermissionState::Granted } else { PermissionState::Denied };
        PermissionStatus {
            screen,
            // Camera/mic need AVFoundation, which arrives with the Swift shim.
            camera: PermissionState::Unknown,
            microphone: PermissionState::Unknown,
            accessibility: PermissionState::Unknown,
        }
    }
    async fn displays(&self) -> ToolResult<Vec<Display>> {
        let mut ids = [0u32; 16];
        let mut count = 0u32;
        // SAFETY: buffer of 16 ids, CoreGraphics writes at most `max` entries.
        let err = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
        if err != 0 {
            return Err(ToolError::backend(format!("CGGetActiveDisplayList failed ({err})")));
        }
        // SAFETY: no arguments.
        let main = unsafe { CGMainDisplayID() };
        let mut out = Vec::new();
        for (i, &id) in ids[..count as usize].iter().enumerate() {
            // SAFETY: `id` came from CGGetActiveDisplayList; the mode is released below.
            let (bounds, scale, hz) = unsafe {
                let b = CGDisplayBounds(id);
                let mode = CGDisplayCopyDisplayMode(id);
                let (px, hz) = if mode.is_null() {
                    (b.size.width as usize, 0.0)
                } else {
                    let r = (CGDisplayModeGetPixelWidth(mode), CGDisplayModeGetRefreshRate(mode));
                    CGDisplayModeRelease(mode);
                    r
                };
                (b, if b.size.width > 0.0 { px as f64 / b.size.width } else { 1.0 }, hz)
            };
            out.push(Display {
                id: id.to_string(),
                name: if id == main { "Main display".into() } else { format!("Display {}", i + 1) },
                main: id == main,
                bounds: Rect { x: bounds.origin.x, y: bounds.origin.y, w: bounds.size.width, h: bounds.size.height },
                scale,
                refresh_hz: hz,
            });
        }
        Ok(out)
    }
    async fn cameras(&self) -> ToolResult<Vec<Device>> {
        Err(not_yet("listing cameras"))
    }
    async fn microphones(&self) -> ToolResult<Vec<Device>> {
        Err(not_yet("listing microphones"))
    }
    async fn open(&self, _config: SessionConfig) -> ToolResult<Box<dyn CaptureSession>> {
        Err(not_yet("recording"))
    }
}

fn not_yet(what: &str) -> ToolError {
    ToolError::backend(format!(
        "macOS {what} is not implemented yet (M0 capture spike). Run the engine with LUMOS_BACKEND=fake to try the pipeline."
    ))
}
