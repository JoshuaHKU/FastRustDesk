//! Probe: does the ScreenCaptureKit backend actually deliver frames, at the right
//! geometry, for every online display?
// Pulling in the crate's own lib makes cargo link this package's native objects
// (src/platform/macos.mm, which defines BackingScaleFactor used by Display::scale).
extern crate librustdesk as _;

use scrap::{Capturer, Display, TraitCapturer, TraitPixelBuffer};
use std::time::{Duration, Instant};

fn main() {
    let displays = Display::all().expect("enumerate displays");
    println!("online displays: {}", displays.len());

    for (i, d) in displays.into_iter().enumerate() {
        let (w, h) = (d.width(), d.height());
        let (ox, oy) = d.origin();
        println!(
            "\n[{i}] name={} {}x{} origin=({ox},{oy}) scale={:.2} primary={} online={}",
            d.name(), w, h, d.scale(), d.is_primary(), d.is_online()
        );

        let mut cap = match Capturer::new(d) {
            Ok(c) => c,
            Err(e) => { println!("  capturer failed: {e}"); continue }
        };

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut frames = 0;
        let mut first_ms = None;
        let t0 = Instant::now();
        while Instant::now() < deadline && frames < 30 {
            match cap.frame(Duration::from_millis(200)) {
                Ok(f) => {
                    if let scrap::Frame::PixelBuffer(pb) = f {
                        if frames == 0 {
                            first_ms = Some(t0.elapsed().as_millis());
                            println!(
                                "  first frame: {}x{} stride={:?} pixfmt={:?} bytes={}",
                                pb.width(), pb.height(), pb.stride(), pb.pixfmt(), pb.data().len()
                            );
                            let expect = pb.stride()[0] * pb.height();
                            assert_eq!(pb.data().len(), expect, "buffer must match stride*height");
                            assert_eq!(pb.width(), w, "width must match the display");
                            assert_eq!(pb.height(), h, "height must match the display");
                            let nonzero = pb.data().iter().any(|&b| b != 0);
                            println!("  content non-blank: {nonzero}");
                        }
                        frames += 1;
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(15));
                }
                Err(e) => { println!("  frame error: {e}"); break }
            }
        }
        println!("  frames in {:?}: {frames} (first at {:?}ms)", t0.elapsed(), first_ms);
        if frames == 0 { println!("  !! NO FRAMES"); }
    }
}
