//! Does `CVPixelBuffer::io_surface()` hand back an owned reference or a borrowed one?
//!
//! This decides whether the zero-copy path is safe. `IOSurface::from_raw` is
//! documented as wrapping a **+1 retained** ref and `Drop` releases, but
//! `CVPixelBufferGetIOSurface` follows CoreFoundation's *Get* rule and returns
//! +0. If apple-cf's Swift bridge does not retain on the way out, every surface
//! we take from a captured frame would be over-released -- a use-after-free that
//! shows up as a random crash under load, long after the code that caused it.
//!
//! The crate's bridge is prebuilt Swift, so the answer cannot be read out of the
//! source. Acquire and drop many times and see whether the surface survives: an
//! over-release corrupts the object, and reading its geometry afterwards either
//! returns garbage or traps.
//!
//! Run: cargo run --release --features hwcodec --example vtenc_surface_probe
extern crate librustdesk as _;

use apple_cf::cv::CVPixelBuffer;
use apple_cf::iosurface::{IOSurface, PlaneProperties};

const W: usize = 640;
const H: usize = 480;
const PIXFMT_420V: u32 = 0x34323076;
const ROUNDS: usize = 2000;

fn main() {
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
    let backing =
        IOSurface::create_with_properties(W, H, PIXFMT_420V, 1, W, y_size + uv_size, Some(&planes))
            .expect("create IOSurface");
    let id = backing.id();
    println!(
        "backing surface id={id} {}x{}",
        backing.width(),
        backing.height()
    );

    let pb = CVPixelBuffer::create_with_io_surface(&backing).expect("wrap in CVPixelBuffer");
    println!("wrapped in CVPixelBuffer\n");

    println!("acquiring and dropping io_surface() {ROUNDS} times...");
    for i in 0..ROUNDS {
        let s = pb.io_surface().expect("io_surface() returned None");
        // Touch it so a corrupted object would be caught here, not silently.
        let (w, h, sid) = (s.width(), s.height(), s.id());
        if w != W || h != H || sid != id {
            println!("CORRUPT at round {i}: {w}x{h} id={sid} (expected {W}x{H} id={id})");
            std::process::exit(1);
        }
        drop(s);
    }

    // The original must still be intact. If io_surface() had been over-releasing,
    // the backing object would have been freed out from under us by now.
    let (w, h, sid) = (backing.width(), backing.height(), backing.id());
    println!("after {ROUNDS} rounds, backing surface: {w}x{h} id={sid}");
    if w != W || h != H || sid != id {
        println!("\nOVER-RELEASE: io_surface() returns a borrowed ref that Drop then frees.");
        println!("The zero-copy path must std::mem::forget it, or retain explicitly.");
        std::process::exit(1);
    }

    println!("\nOK: io_surface() is balanced -- the returned IOSurface owns a reference");
    println!("and can be held across the encode call safely.");
}
