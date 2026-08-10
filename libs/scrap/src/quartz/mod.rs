//! CoreGraphics display enumeration and geometry.
//!
//! The capture *stream* moved to ScreenCaptureKit (see `crate::sck`) because Apple
//! deprecated CGDisplayStream in macOS 14.0. What remains here is current API:
//! CGDisplayPixelsWide / CGGetOnlineDisplayList / CGDisplayBounds / CGDisplayIsMain,
//! which supply the display geometry and identity ScreenCaptureKit does not expose.
pub use self::config::Config;
pub use self::display::Display;
pub use self::ffi::{CGError, PixelFormat};

mod config;
mod display;
pub mod ffi;

use std::sync::{Arc, Mutex};

lazy_static::lazy_static! {
    pub static ref ENABLE_RETINA: Arc<Mutex<bool>> = Arc::new(Mutex::new(true));
}
