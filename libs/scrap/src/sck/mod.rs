//! ScreenCaptureKit capture backend for macOS.
//!
//! Replaces the `quartz` backend's `CGDisplayStream`, which Apple deprecated in
//! macOS 14.0. Only the *stream* is replaced: `quartz::Display` stays as-is because
//! it is built on `CGDisplayPixelsWide`/`CGGetOnlineDisplayList`/`CGDisplayBounds`,
//! which are current API and give the display geometry ScreenCaptureKit does not.
//!
//! The public shape deliberately mirrors `quartz::Capturer` and `quartz::Frame` so
//! `common/macos.rs` reads almost identically to the old `common/quartz.rs`.
//!
//! Frames are copied to BGRA inside the stream callback rather than handed out as a
//! locked IOSurface. The old backend copied in `surface_to_bgra` anyway, so this
//! costs nothing and avoids holding a `CVPixelBuffer` lock across threads.

use std::sync::{Arc, Mutex};
use std::{ops, time::Duration};

use screencapturekit::cm::CMTime;
use screencapturekit::cv::CVPixelBufferLockFlags;
use screencapturekit::prelude::*;
use screencapturekit::stream::configuration::{
    PixelFormat as SckPixelFormat, SCStreamConfiguration,
};

// `PixelFormat` here is the quartz backend's, kept because it appears in the
// public `Capturer` signature. The ScreenCaptureKit one is aliased above.
use super::quartz::{CGError, Config, Display, PixelFormat};

/// How many times to re-query shareable content when the wanted display is absent.
///
/// Around a display reconfiguration (sleep/wake, resolution change, monitor attach)
/// ScreenCaptureKit briefly reports an empty or stale display list to an already
/// running process, even though CoreGraphics still sees the screen. Taking the first
/// answer as truth turns a transient blip into a permanent capture failure.
const SHAREABLE_CONTENT_RETRIES: u32 = 5;
const SHAREABLE_CONTENT_RETRY_DELAY: Duration = Duration::from_millis(400);

/// A captured frame.
///
/// Either packed BGRA (one plane) or biplanar NV12 (Y plane followed by an
/// interleaved CbCr plane), depending on what the stream was configured for.
/// NV12 is the cheaper path by a wide margin: ScreenCaptureKit does the colour
/// conversion on the GPU, and the buffer we copy out is 12 bits per pixel instead
/// of 32.
pub struct Frame {
    data: Vec<u8>,
    /// One entry for BGRA, two for NV12 (Y stride, then CbCr stride).
    strides: Vec<usize>,
    /// Byte offset of the CbCr plane inside `data`. Zero when not planar.
    uv_offset: usize,
    pixfmt: crate::Pixfmt,
    /// The surface the pixels actually live in, retained for the lifetime of this
    /// frame. Present only on the NV12 path, where it can go straight to
    /// VTCompressionSession with no CPU copy at all. `io_surface()` returns an
    /// owned reference (verified in examples/vtenc_surface_probe.rs), so holding
    /// it here is balanced.
    surface: Option<apple_cf::iosurface::IOSurface>,
}

impl Frame {
    /// Raw bytes, used by the caller to detect an unchanged screen before doing any
    /// further work. Same buffer as `Deref` — the old backend compared the whole
    /// IOSurface allocation instead, but comparing the pixels is equivalent here.
    #[inline]
    pub fn inner(&self) -> &[u8] {
        &self.data
    }

    /// Stride of the first (or only) plane.
    #[inline]
    pub fn stride(&self) -> usize {
        self.strides.first().copied().unwrap_or(0)
    }

    #[inline]
    pub fn strides(&self) -> &[usize] {
        &self.strides
    }

    #[inline]
    pub fn uv_offset(&self) -> usize {
        self.uv_offset
    }

    #[inline]
    pub fn pixfmt(&self) -> crate::Pixfmt {
        self.pixfmt
    }

    #[inline]
    pub fn surface(&self) -> Option<&apple_cf::iosurface::IOSurface> {
        self.surface.as_ref()
    }

    /// Kept for API compatibility with the quartz backend, where this performed the
    /// IOSurface -> BGRA copy. The SCK path has already done it.
    #[inline]
    pub fn surface_to_bgra(&mut self, _h: usize) {}
}

impl ops::Deref for Frame {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.data
    }
}

/// Bridge the surface from `screencapturekit`'s IOSurface type to `apple-cf`'s.
///
/// Both crates wrap the same `IOSurfaceRef` but in distinct Rust types, so the
/// hand-off goes through the raw pointer. Each wrapper releases on drop, so the
/// pointer is retained once on the way across -- otherwise the two Drops would
/// release a single reference twice.
///
/// (`CVPixelBuffer::io_surface()` itself is balanced; verified empirically in
/// examples/vtenc_surface_probe.rs.)
fn bridge_surface(
    pb: &screencapturekit::cv::CVPixelBuffer,
) -> Option<apple_cf::iosurface::IOSurface> {
    let sck_surface = pb.io_surface()?;
    let ptr = sck_surface.as_ptr();
    if ptr.is_null() {
        return None;
    }
    // SAFETY: `ptr` is a live IOSurfaceRef owned by `sck_surface`; retaining gives
    // the apple-cf wrapper its own reference to release.
    let retained = unsafe { apple_cf::ffi::io_surface_retain(ptr) };
    apple_cf::iosurface::IOSurface::from_raw(retained)
}

struct Output<F: Fn(Frame) + Send + Sync + 'static> {
    handler: F,
    /// Set when the stream reports it has stopped, so `Drop` need not block forever.
    stopped: Arc<Mutex<bool>>,
}

impl<F: Fn(Frame) + Send + Sync + 'static> SCStreamOutputTrait for Output<F> {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
        use screencapturekit::cm::SCFrameStatus;

        if of_type != SCStreamOutputType::Screen {
            return;
        }

        match sample.frame_status() {
            // Idle: the desktop is unchanged and carries no pixels. Blank/Suspended
            // are equally undisplayable. Dropping them here is what the old backend
            // achieved with its whole-frame memcmp, only cheaper.
            Some(SCFrameStatus::Idle)
            | Some(SCFrameStatus::Blank)
            | Some(SCFrameStatus::Suspended) => return,
            Some(SCFrameStatus::Stopped) => {
                if let Ok(mut s) = self.stopped.lock() {
                    *s = true;
                }
                return;
            }
            _ => {}
        }

        let Some(pixel_buffer) = sample.image_buffer() else {
            return;
        };
        let Ok(guard) = pixel_buffer.lock(CVPixelBufferLockFlags::READ_ONLY) else {
            return;
        };

        // Biplanar NV12: copy the Y plane then the CbCr plane into one buffer. The
        // planes are separate IOSurface allocations, so they have to be concatenated
        // for the rest of the pipeline, which expects a single slice plus offsets.
        if guard.plane_count() >= 2 {
            let y_h = guard.height_of_plane(0);
            let uv_h = guard.height_of_plane(1);
            let y_stride = pixel_buffer.bytes_per_row_of_plane(0);
            let uv_stride = pixel_buffer.bytes_per_row_of_plane(1);
            let (Some(y_ptr), Some(uv_ptr)) = (
                guard.base_address_of_plane(0),
                guard.base_address_of_plane(1),
            ) else {
                return;
            };
            if y_h == 0 || uv_h == 0 || y_stride == 0 || uv_stride == 0 {
                return;
            }
            let y_len = y_stride * y_h;
            let uv_len = uv_stride * uv_h;
            let mut data = Vec::with_capacity(y_len + uv_len);
            // SAFETY: the buffer stays locked for the lifetime of `guard`, and the
            // lengths come from that same locked buffer's plane geometry.
            unsafe {
                data.extend_from_slice(std::slice::from_raw_parts(y_ptr, y_len));
                data.extend_from_slice(std::slice::from_raw_parts(uv_ptr, uv_len));
            }
            (self.handler)(Frame {
                data,
                strides: vec![y_stride, uv_stride],
                uv_offset: y_len,
                pixfmt: crate::Pixfmt::NV12,
                surface: bridge_surface(&pixel_buffer),
            });
            return;
        }

        let height = guard.height() as usize;
        let stride = guard.bytes_per_row();
        let pixels = guard.as_slice();
        if height == 0 || stride == 0 || pixels.is_empty() {
            return;
        }

        // Copy only the rows we will actually read. `as_slice` can cover a larger
        // allocation than stride*height.
        //
        // Do NOT use `vec![0u8; wanted]` here: it memsets the whole buffer before we
        // immediately overwrite it. On a 5K display that is a wasted 59 MB write per
        // frame — at 23 fps, 1.4 GB/s of pure overhead. `with_capacity` +
        // `extend_from_slice` writes each byte once; the `resize` only does work in
        // the short-read case, where it zero-pads the tail we could not fill.
        let wanted = stride.saturating_mul(height);
        let take = wanted.min(pixels.len());
        let mut data = Vec::with_capacity(wanted);
        data.extend_from_slice(&pixels[..take]);
        data.resize(wanted, 0);

        (self.handler)(Frame {
            data,
            strides: vec![stride],
            uv_offset: 0,
            pixfmt: crate::Pixfmt::BGRA,
            // BGRA is the fallback path; the zero-copy encoder wants YUV.
            surface: None,
        });
    }
}

pub struct Capturer {
    stream: SCStream,
    width: usize,
    height: usize,
    format: PixelFormat,
    display: Display,
    stopped: Arc<Mutex<bool>>,
}

impl Capturer {
    pub fn new<F: Fn(Frame) + Send + Sync + 'static>(
        display: Display,
        width: usize,
        height: usize,
        format: PixelFormat,
        config: Config,
        handler: F,
    ) -> Result<Capturer, CGError> {
        let sc_display = find_display(display.id()).ok_or(CGError::Failure)?;

        let stopped = Arc::new(Mutex::new(false));

        // `Config` maps onto SCStreamConfiguration one for one:
        //   cursor       -> shows_cursor
        //   letterbox    -> scales_to_fit (preserve aspect ratio, pad the remainder)
        //   throttle     -> minimum_frame_interval
        //   queue_length -> queue_depth
        let fps = if config.throttle > 0.0 {
            (1.0 / config.throttle).round().max(1.0) as i32
        } else {
            60
        };
        // Ask ScreenCaptureKit for NV12 rather than BGRA.
        //
        // The encoders downstream want YUV: the hardware HEVC path takes NV12
        // (hwcodec's DEFAULT_PIXFMT) and the software VP9 fallback takes I420. With a
        // BGRA capture, every single frame goes through libyuv's ARGBToNV12 on the
        // CPU — on a 5120x2880 display that is 14.7 million pixels of colour-space
        // maths per frame, and it dominated the capture cost. ScreenCaptureKit can
        // hand us YUV directly, converted on the GPU for free, and the buffer we copy
        // out shrinks from 32 bpp to 12 bpp (59 MB -> 22 MB per frame).
        //
        // 420v, not 420f: libyuv's ARGBToNV12 produces BT.601 *video* range, so that
        // is what the rest of the pipeline and the encoders are calibrated for.
        // Picking the full-range variant would shift every colour.
        //
        // Escape hatch: RUSTDESK_SCK_BGRA=1 forces the old packed-BGRA capture, in
        // case a display or codec combination misbehaves. The frame carries its own
        // pixfmt, so everything downstream adapts without a rebuild.
        let want_bgra = std::env::var("RUSTDESK_SCK_BGRA")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let sck_fmt = if want_bgra {
            SckPixelFormat::BGRA
        } else {
            SckPixelFormat::YCbCr_420v
        };
        let stream_config = SCStreamConfiguration::new()
            .with_width(width as u32)
            .with_height(height as u32)
            .with_shows_cursor(config.cursor)
            .with_scales_to_fit(config.letterbox)
            .with_minimum_frame_interval(&CMTime::new(1, fps))
            .with_queue_depth(config.queue_length.max(3) as u32)
            .with_pixel_format(sck_fmt);

        let filter = SCContentFilter::create()
            .with_display(&sc_display)
            .with_excluding_windows(&[])
            .build();

        let mut stream = SCStream::new(&filter, &stream_config);
        stream.add_output_handler(
            Output {
                handler,
                stopped: stopped.clone(),
            },
            SCStreamOutputType::Screen,
        );

        stream.start_capture().map_err(|_| CGError::Failure)?;

        Ok(Capturer {
            stream,
            width,
            height,
            format,
            display,
            stopped,
        })
    }

    pub fn width(&self) -> usize {
        self.width
    }
    pub fn height(&self) -> usize {
        self.height
    }
    pub fn format(&self) -> PixelFormat {
        self.format
    }
    pub fn display(&self) -> Display {
        self.display
    }
    /// Whether the stream reported that it stopped on its own.
    pub fn is_stopped(&self) -> bool {
        self.stopped.lock().map(|s| *s).unwrap_or(false)
    }
}

impl Drop for Capturer {
    fn drop(&mut self) {
        let _ = self.stream.stop_capture();
    }
}

/// Find the `SCDisplay` matching a CoreGraphics display id.
///
/// Insists on that exact id rather than settling for whatever else is listed: a
/// stale entry has been observed that ScreenCaptureKit then refuses to capture
/// ("No capture source provided"), which is a permanent black screen. Failing here
/// instead lets the caller retry or fall back.
fn find_display(id: u32) -> Option<SCDisplay> {
    for attempt in 0..SHAREABLE_CONTENT_RETRIES {
        match SCShareableContent::get() {
            Ok(content) => {
                if let Some(d) = content
                    .displays()
                    .into_iter()
                    .find(|d| d.display_id() == id)
                {
                    return Some(d);
                }
            }
            Err(_) => {}
        }
        if attempt + 1 < SHAREABLE_CONTENT_RETRIES {
            std::thread::sleep(SHAREABLE_CONTENT_RETRY_DELAY);
        }
    }
    None
}
