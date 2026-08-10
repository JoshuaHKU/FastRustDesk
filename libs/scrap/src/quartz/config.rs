//! Capture configuration.
//!
//! Plain data now: the ScreenCaptureKit backend reads these fields directly
//! (crate::sck::Capturer::new maps them onto SCStreamConfiguration). The
//! CoreFoundation imports went with `build()`.

//TODO: Color space, YCbCr matrix.
pub struct Config {
    /// Whether the cursor is visible.
    pub cursor: bool,
    /// Whether it should letterbox or stretch.
    pub letterbox: bool,
    /// Minimum seconds per frame.
    pub throttle: f64,
    /// How many frames are allocated.
    /// 3 is the recommended value.
    /// 8 is the maximum value.
    pub queue_length: i8,
}

impl Config {
    // NOTE: this impl used to hold `build()`, which turned these fields into the
    // CFDictionary that CGDisplayStreamCreate took. The ScreenCaptureKit backend
    // reads the fields directly (crate::sck::Capturer::new maps them onto
    // SCStreamConfiguration), so the dictionary is gone.
}

impl Default for Config {
    fn default() -> Config {
        Config {
            cursor: false,
            letterbox: true,
            // Cap capture at 30 fps. RustDesk itself clamps the session to 30
            // (`custom_fps > 30` is reduced in client.rs unless the peer allows
            // more), so anything ScreenCaptureKit delivers above that is copied
            // out of the IOSurface and then thrown away — 59 MB per wasted frame
            // on a 5K display. Leaving this at 0.0 asked for 60.
            throttle: 1.0 / 30.0,
            queue_length: 3,
        }
    }
}
