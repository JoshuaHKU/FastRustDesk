//! End-to-end probe of the modified capture+encode engine:
//! ScreenCaptureKit capture -> H265 hardware encode (VideoToolbox).
//!
//! Verifies the two changes this fork exists for, without needing the Flutter UI:
//!   * frames actually come out of the SCK backend at the display's geometry
//!   * they feed the hardware H265 encoder and produce a plausible bitstream
//!
//! Run: cargo run --release --features hwcodec --example pipeline_probe
extern crate librustdesk as _;

use scrap::codec::{EncoderApi, EncoderCfg};
use scrap::hwcodec::{HwRamEncoder, HwRamEncoderConfig};
use scrap::{Capturer, Display, TraitCapturer, TraitPixelBuffer};
use std::time::{Duration, Instant};

fn main() {
    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    let d = Display::primary().expect("primary display");
    let (w, h) = (d.width(), d.height());
    println!(
        "display: {}x{} (name={} scale={:.2})",
        w,
        h,
        d.name(),
        d.scale()
    );

    // try_get() reads a cached codec config that the app normally populates during
    // startup. Probe and store it first, exactly as the real startup path does.
    scrap::hwcodec::HwCodecConfig::set(scrap::hwcodec::check_available_hwcodec());

    // Which hardware encoder did the build actually find?
    let info = HwRamEncoder::try_get(scrap::CodecFormat::H265)
        .expect("no H265 hardware encoder available — was --features hwcodec used?");
    println!("H265 encoder: {}", info.name);
    assert!(
        info.name.contains("videotoolbox"),
        "expected VideoToolbox, got {}",
        info.name
    );

    let mut enc = HwRamEncoder::new(
        EncoderCfg::HWRAM(HwRamEncoderConfig {
            name: info.name.clone(),
            mc_name: None,
            width: w,
            height: h,
            quality: 1.0,
            keyframe_interval: None,
        }),
        false,
    )
    .expect("create H265 encoder");
    println!("encoder bitrate: {} kbps", enc.bitrate());

    let mut cap = Capturer::new(d).expect("capturer");

    let (mut frames, mut encoded, mut bytes, mut keyframes) = (0u64, 0u64, 0u64, 0u64);
    let mut worst_encode = Duration::ZERO;
    let start = Instant::now();
    let deadline = start + Duration::from_secs(secs);

    while Instant::now() < deadline {
        let f = match cap.frame(Duration::from_millis(200)) {
            Ok(f) => f,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(e) => {
                println!("capture error: {e}");
                break;
            }
        };
        let scrap::Frame::PixelBuffer(pb) = f else {
            continue;
        };
        frames += 1;

        let t0 = Instant::now();
        match enc.encode(pb.data(), 0) {
            Ok(units) => {
                let dt = t0.elapsed();
                worst_encode = worst_encode.max(dt);
                for u in units {
                    encoded += 1;
                    bytes += u.data.len() as u64;
                    if u.key == 1 {
                        keyframes += 1;
                    }
                }
            }
            Err(e) => println!("encode error: {e}"),
        }
    }

    let el = start.elapsed().as_secs_f64();
    println!("\n--- {:.1}s ---", el);
    println!("captured frames : {frames}  ({:.1}/s)", frames as f64 / el);
    println!("encoded units   : {encoded} ({keyframes} keyframes)");
    println!(
        "bitstream       : {:.2} MB  ({:.2} Mbps)",
        bytes as f64 / 1e6,
        (bytes as f64 * 8.0) / el / 1e6
    );
    println!(
        "worst encode    : {:.1} ms",
        worst_encode.as_secs_f64() * 1000.0
    );

    assert!(frames > 0, "no frames captured");
    assert!(encoded > 0, "encoder produced nothing");
    assert!(bytes > 0, "empty bitstream");
    println!("\nOK: ScreenCaptureKit -> H265/VideoToolbox pipeline is live");
}
