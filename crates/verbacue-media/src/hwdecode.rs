//! Hardware video decoding: VideoToolbox on macOS, Direct3D 11 (D3D11VA) on Windows.
//! This module exists only on these two systems.
//!
//! FFmpeg has no software AV1 decoder in this build. Its own AV1 decoder works only
//! with a hardware device. ffmpeg-next has no API for hardware devices, so this module
//! calls the FFmpeg C functions directly. It is the only module with unsafe code.
#![allow(unsafe_code)]

use ffmpeg_next::{codec, ffi, frame};

use verbacue_core::error::{Result, bail};

/// Codecs that FFmpeg can decode only with a hardware device.
const HARDWARE_ONLY: &[codec::Id] = &[codec::Id::AV1];

#[cfg(target_os = "macos")]
const DEVICE: (ffi::AVHWDeviceType, &str) = (
    ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
    "VideoToolbox",
);
#[cfg(windows)]
const DEVICE: (ffi::AVHWDeviceType, &str) =
    (ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA, "Direct3D 11");

/// Gives the decoder context a hardware device if `codec` needs one. Call it before
/// the decoder opens. It returns `false` if the codec needs no device or if the system
/// has none.
pub fn attach(ctx: &mut codec::context::Context, codec: codec::Id) -> bool {
    if !HARDWARE_ONLY.contains(&codec) {
        return false;
    }
    // SAFETY: `ctx` is a valid codec context that is not open yet. FFmpeg writes a new
    // device reference into `hw_device_ctx`, and frees it with the context.
    let ret = unsafe {
        ffi::av_hwdevice_ctx_create(
            &raw mut (*ctx.as_mut_ptr()).hw_device_ctx,
            DEVICE.0,
            std::ptr::null(),
            std::ptr::null_mut(),
            0,
        )
    };
    if ret < 0 {
        tracing::debug!("no {} device (error {ret})", DEVICE.1);
        return false;
    }
    tracing::debug!("decoding {} video with {}", codec.name(), DEVICE.1);
    true
}

/// Copies a frame from GPU memory into normal memory. It returns `None` if the frame
/// is already in normal memory.
///
/// # Errors
/// Returns an error if FFmpeg cannot copy the frame.
pub fn download(frame: &frame::Video) -> Result<Option<frame::Video>> {
    // SAFETY: `frame` is a valid decoded frame.
    if unsafe { (*frame.as_ptr()).hw_frames_ctx.is_null() } {
        return Ok(None);
    }
    let mut out = frame::Video::empty();
    // SAFETY: both frames are valid. `out` is empty, so FFmpeg allocates its buffers.
    let ret = unsafe {
        let ret = ffi::av_hwframe_transfer_data(out.as_mut_ptr(), frame.as_ptr(), 0);
        if ret < 0 {
            ret
        } else {
            ffi::av_frame_copy_props(out.as_mut_ptr(), frame.as_ptr())
        }
    };
    if ret < 0 {
        bail!("cannot copy a hardware frame (error {ret})");
    }
    Ok(Some(out))
}
