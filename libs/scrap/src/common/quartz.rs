//! macOS capture adapter.
//!
//! Frames come from the ScreenCaptureKit backend (`crate::sck`); display
//! enumeration stays on CoreGraphics (`crate::quartz::Display`), which is current
//! API and exposes geometry ScreenCaptureKit does not.
use crate::{quartz, sck, Frame, Pixfmt};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, TryLockError};
use std::{io, mem};

pub struct Capturer {
    inner: sck::Capturer,
    frame: Arc<Mutex<Option<sck::Frame>>>,
}

impl Capturer {
    pub fn new(display: Display) -> io::Result<Capturer> {
        let (w, h) = (display.width(), display.height());
        Self::new_with_size(display, w, h)
    }

    /// LOCAL FORK (fast.6): capture at `width` x `height` instead of the
    /// display's native size. ScreenCaptureKit scales on the GPU, so the frames
    /// (and the IOSurfaces the zero-copy encoder takes) simply come out smaller;
    /// nothing downstream converts or copies more. Used for P1-c downscale.
    pub fn new_with_size(display: Display, width: usize, height: usize) -> io::Result<Capturer> {
        let frame = Arc::new(Mutex::new(None));

        let f = frame.clone();
        let inner = sck::Capturer::new(
            display.0,
            width,
            height,
            quartz::PixelFormat::Argb8888,
            Default::default(),
            move |inner| {
                if let Ok(mut f) = f.lock() {
                    *f = Some(inner);
                }
            },
        )
        .map_err(|_| io::Error::from(io::ErrorKind::Other))?;

        Ok(Capturer { inner, frame })
    }

    pub fn width(&self) -> usize {
        self.inner.width()
    }

    pub fn height(&self) -> usize {
        self.inner.height()
    }
}

impl crate::TraitCapturer for Capturer {
    fn frame<'a>(&'a mut self, _timeout_ms: std::time::Duration) -> io::Result<Frame<'a>> {
        match self.frame.try_lock() {
            Ok(mut handle) => {
                let mut frame = None;
                mem::swap(&mut frame, &mut handle);

                match frame {
                    // No `would_block_if_equal` here, unlike the CGDisplayStream
                    // backend this replaced. That helper does a whole-frame memcmp
                    // and then a whole-frame copy into a scratch buffer -- on a 5K
                    // display, 59 MB compared plus 59 MB copied for every frame, to
                    // discover the screen did not change.
                    //
                    // ScreenCaptureKit already answers that question for free:
                    // unchanged desktops arrive as SCFrameStatus::Idle and are
                    // dropped in the stream callback, so nothing identical reaches
                    // this point. The slot is also emptied by the `mem::swap` above,
                    // so each captured frame is handed out exactly once.
                    Some(mut frame) => {
                        frame.surface_to_bgra(self.height());
                        Ok(Frame::PixelBuffer(PixelBuffer {
                            frame,
                            data: PhantomData,
                            width: self.width(),
                            height: self.height(),
                        }))
                    }

                    // No frame this round. Distinguish "nothing yet" from "the stream
                    // is gone": ScreenCaptureKit stops the stream on display
                    // reconfiguration or a revoked permission and then simply never
                    // calls back. Reporting WouldBlock forever would leave the session
                    // frozen on its last frame, so surface a real error -- the caller
                    // treats that as a reason to re-check the display and rebuild the
                    // capturer.
                    None if self.inner.is_stopped() => Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "ScreenCaptureKit stream stopped",
                    )),
                    None => Err(io::ErrorKind::WouldBlock.into()),
                }
            }

            Err(TryLockError::WouldBlock) => Err(io::ErrorKind::WouldBlock.into()),

            Err(TryLockError::Poisoned(..)) => Err(io::ErrorKind::Other.into()),
        }
    }
}

pub struct PixelBuffer<'a> {
    frame: sck::Frame,
    data: PhantomData<&'a [u8]>,
    width: usize,
    height: usize,
}

impl<'a> crate::TraitPixelBuffer for PixelBuffer<'a> {
    fn data(&self) -> &[u8] {
        &*self.frame
    }

    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    /// One stride for BGRA, two for NV12 (Y then CbCr). Reported from the frame
    /// itself rather than assumed, because the stream can deliver either.
    fn stride(&self) -> Vec<usize> {
        self.frame.strides().to_vec()
    }

    fn pixfmt(&self) -> Pixfmt {
        self.frame.pixfmt()
    }
}

impl<'a> PixelBuffer<'a> {
    /// Byte offset of the CbCr plane within `data()`. Meaningful only for NV12.
    #[inline]
    pub fn uv_offset(&self) -> usize {
        self.frame.uv_offset()
    }

    /// The IOSurface the pixels live in, when the capture produced one. This is
    /// what makes the zero-copy encode possible: it goes straight to
    /// VTCompressionSession without the bytes in `data()` being touched.
    #[inline]
    pub fn surface(&self) -> Option<&apple_cf::iosurface::IOSurface> {
        self.frame.surface()
    }
}

pub struct Display(quartz::Display);

impl Display {
    pub fn primary() -> io::Result<Display> {
        Ok(Display(quartz::Display::primary()))
    }

    pub fn all() -> io::Result<Vec<Display>> {
        Ok(quartz::Display::online()
            .map_err(|_| io::Error::from(io::ErrorKind::Other))?
            .into_iter()
            .map(Display)
            .collect())
    }

    pub fn width(&self) -> usize {
        self.0.width()
    }

    pub fn height(&self) -> usize {
        self.0.height()
    }

    pub fn scale(&self) -> f64 {
        self.0.scale()
    }

    pub fn name(&self) -> String {
        self.0.id().to_string()
    }

    pub fn is_online(&self) -> bool {
        self.0.is_online()
    }

    pub fn origin(&self) -> (i32, i32) {
        let o = self.0.bounds().origin;
        (o.x as _, o.y as _)
    }

    pub fn is_primary(&self) -> bool {
        self.0.is_primary()
    }
}
