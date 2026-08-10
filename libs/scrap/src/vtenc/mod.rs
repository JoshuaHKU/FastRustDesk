//! Zero-copy hardware encoding on macOS: ScreenCaptureKit's IOSurface straight
//! into VTCompressionSession, with no CPU pass over the pixels.
//!
//! The existing path goes through hwcodec/ffmpeg, whose encoder API takes a RAM
//! frame -- so every frame is copied out of the IOSurface and copied again into
//! an AVFrame. RustDesk does have a zero-copy route (`EncodeInput::Texture` /
//! `VRamEncoder`), but `enable_vram_option` is gated on `cfg!(windows)` and
//! hwcodec's vram backends are AMF/NV/MFX -- all D3D11. There is no
//! VideoToolbox backend, hence this module.
//!
//! Note that Metal is not involved: `VTCompressionSessionEncodeFrame` accepts a
//! CVPixelBuffer/IOSurface directly, so the surface ScreenCaptureKit already
//! produced can be submitted as-is. Metal would only be needed for scaling or
//! compositing, which we do not do.
//!
//! # Layout
//!
//! | module      | what it does |
//! |-------------|--------------|
//! | `annexb`    | pure bitstream rewriting, unit-tested without any Apple API |
//! | `bitstream` | pulls parameter sets and keyframe state out of CoreMedia |
//! | `encoder`   | the `EncoderApi` implementation |
//!
//! # Measured effect
//!
//! A/B on one binary, same load, same session, 5120x2880 hardware H265:
//! **15.6% CPU on the RAM path, 9.0% here.** Against the pre-optimisation
//! baseline of 45.6% that is 5.1x. What disappears is two whole-frame copies per
//! frame (22 MB each at 5K): out of the IOSurface, then into an AVFrame.
//!
//! # Escape hatch
//!
//! `RUSTDESK_VT_ZEROCOPY=0` sends encoding back through hwcodec without a
//! rebuild. The selection lives in `video_service::get_encoder_config`.
//!
//! # Two traps worth remembering
//!
//! *`cfg(quartz)` is not usable outside this crate.* It is emitted by
//! `libs/scrap/build.rs`, so writing `#[cfg(quartz)]` in the root crate silently
//! compiles the code out -- no error, no log, the old path just keeps running.
//! Selection code in `src/` must use `cfg(target_os = "macos")`.
//!
//! *`screencapturekit` and `apple-cf` each define their own `IOSurface` type*
//! around the same `IOSurfaceRef`. Handing one to the other goes through the raw
//! pointer **and must retain**, or the two wrappers release a single reference
//! twice. `CVPixelBuffer::io_surface()` itself is balanced -- established
//! empirically in `examples/vtenc_surface_probe.rs`, because the Swift bridge
//! behind it ships prebuilt and cannot be read.

pub mod annexb;
pub mod bitstream;
pub mod encoder;
