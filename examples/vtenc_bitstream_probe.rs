//! Milestone 1 of the zero-copy encoder: prove the bitstream conversion.
//!
//! VideoToolbox returns length-prefixed NAL units with the parameter sets held
//! separately in the format description, while the rest of RustDesk expects
//! Annex-B with the parameter sets inlined ahead of keyframes. That conversion
//! is the single highest-risk piece of the whole change: get it wrong and the
//! decoder shows garbage or nothing, with no error anywhere to point at it.
//!
//! So it is validated here **in isolation from screen capture** -- synthetic
//! frames into an IOSurface, real hardware encode, convert, write `.h265`. The
//! verdict comes from ffprobe/ffmpeg, not from this program agreeing with itself.
//!
//! Run: cargo run --release --features hwcodec --example vtenc_bitstream_probe
//! Then: ffprobe -v error -show_streams /tmp/vtenc_probe.h265
extern crate librustdesk as _;

use apple_cf::iosurface::{IOSurface, IOSurfaceLockOptions, PlaneProperties};
use scrap::vtenc::{annexb, bitstream};
use videotoolbox::compression::CompressionSession;
use videotoolbox::session::Codec;

const W: usize = 1280;
const H: usize = 720;
const FRAMES: i64 = 60;
const OUT: &str = "/tmp/vtenc_probe.h265";

// '420v' -- biplanar YCbCr 4:2:0, video range. Same format the SCK capture path
// uses, and what libyuv's output is calibrated for.
const PIXFMT_420V: u32 = 0x34323076;

fn main() {
    // Biplanar surface: Y at offset 0, interleaved CbCr right after it. Strides
    // are the plain widths -- no alignment padding -- which keeps `paint` simple
    // and is what the encoder reads back through the plane descriptors anyway.
    let y_size = W * H;
    let uv_size = W * (H / 2);
    let planes = [
        PlaneProperties {
            width: W,
            height: H,
            bytes_per_row: W,
            bytes_per_element: 1,
            offset: 0,
            size: y_size,
        },
        PlaneProperties {
            width: W / 2,
            height: H / 2,
            bytes_per_row: W,
            bytes_per_element: 2,
            offset: y_size,
            size: uv_size,
        },
    ];
    let surface =
        IOSurface::create_with_properties(W, H, PIXFMT_420V, 1, W, y_size + uv_size, Some(&planes))
            .expect("create IOSurface");
    println!(
        "surface: {}x{} planes={} fmt=420v",
        surface.width(),
        surface.height(),
        surface.plane_count()
    );

    let session = CompressionSession::builder(W as i32, H as i32, Codec::HEVC)
        .with_real_time(true)
        // No B-frames. Reordering would add latency and put the output out of
        // presentation order, neither of which a remote-desktop stream wants.
        .with_allow_frame_reordering(false)
        .with_average_bit_rate(4_000_000)
        .with_expected_frame_rate(30.0)
        .with_max_keyframe_interval(30)
        .build()
        .expect("create HEVC compression session");
    println!("encoder: hardware HEVC (VideoToolbox)\n");

    let mut out = Vec::new();
    let (mut encoded, mut keyframes, mut param_emissions) = (0u32, 0u32, 0u32);
    let mut nal_len_seen = 0usize;

    for i in 0..FRAMES {
        paint(&surface, i);
        let frame = match session.encode(&surface, (i, 30)) {
            Ok(f) => f,
            Err(e) => {
                println!("encode failed at frame {i}: {e:?}");
                std::process::exit(1);
            }
        };
        let Some(sample) = frame.cm_sample_buffer() else {
            continue; // dropped frame
        };
        encoded += 1;

        let key = bitstream::is_keyframe(sample);
        let ps = bitstream::parameter_sets(sample, true);
        let nal_len = ps.as_ref().map(|p| p.nal_length_size).unwrap_or(4);
        if nal_len_seen == 0 {
            nal_len_seen = nal_len;
        }
        if key {
            keyframes += 1;
            if let Some(ps) = &ps {
                annexb::append_parameter_sets(&ps.sets, &mut out);
                param_emissions += 1;
            }
        }
        if let Err(e) = annexb::append_annexb(&frame.data, nal_len, &mut out) {
            println!("conversion failed at frame {i}: {e}");
            std::process::exit(1);
        }
    }

    std::fs::write(OUT, &out).expect("write bitstream");

    println!("encoded frames   : {encoded}/{FRAMES}");
    println!("keyframes        : {keyframes}");
    println!("parameter sets   : emitted {param_emissions} time(s)");
    println!("NAL length size  : {nal_len_seen}");
    println!("bitstream        : {} bytes -> {OUT}", out.len());
    println!("starts with start code: {}", out.starts_with(&[0, 0, 0, 1]));

    assert!(encoded > 0, "encoder produced nothing");
    assert!(keyframes > 0, "no keyframe seen");
    assert!(param_emissions > 0, "parameter sets never emitted");
    assert!(out.starts_with(&[0, 0, 0, 1]), "not Annex-B");

    println!("\nNow verify with a real decoder:");
    println!(
        "  ffprobe -v error -show_entries stream=codec_name,width,height -of default=nk=1 {OUT}"
    );
    println!("  ffmpeg -v error -i {OUT} -f null -   # must decode every frame without complaint");
}

/// A moving pattern, so successive frames genuinely differ -- a static image
/// would compress to almost nothing and prove little about the NAL walk.
fn paint(surface: &IOSurface, i: i64) {
    let Ok(mut guard) = surface.lock(IOSurfaceLockOptions::NONE) else {
        return;
    };
    let bar = ((i * 17) as usize) % W;
    if let Some(y) = guard.base_address_of_plane_mut(0) {
        let stride = surface.bytes_per_row_of_plane(0);
        for row in 0..H {
            // SAFETY: writing within the plane's own stride*height allocation.
            unsafe {
                let line = y.add(row * stride);
                std::ptr::write_bytes(line, 16, stride.min(W));
                // bright vertical bar that sweeps across
                let x0 = (bar + row / 8) % W;
                std::ptr::write_bytes(line.add(x0), 235, (64).min(W - x0));
            }
        }
    }
    if let Some(uv) = guard.base_address_of_plane_mut(1) {
        let stride = surface.bytes_per_row_of_plane(1);
        for row in 0..H / 2 {
            unsafe {
                std::ptr::write_bytes(uv.add(row * stride), 128, stride.min(W));
            }
        }
    }
}
