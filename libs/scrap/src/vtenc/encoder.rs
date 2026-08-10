//! `EncoderApi` implementation that feeds an IOSurface straight to
//! VTCompressionSession.
//!
//! Everything the rest of the pipeline sees is unchanged -- same `VideoFrame`
//! with Annex-B H265 payloads, same bitrate maths as the hwcodec path so image
//! quality at a given "quality" setting is comparable. The only difference is
//! that the pixels never come back to the CPU.

use crate::codec::{base_bitrate, EncoderApi, EncoderCfg};
use crate::vtenc::{annexb, bitstream};
use crate::{EncodeInput, EncodeYuvFormat, Pixfmt};
use apple_cf::iosurface::IOSurface;
use hbb_common::{
    anyhow::{anyhow, bail, Context},
    bytes::Bytes,
    log,
    message_proto::{EncodedVideoFrame, EncodedVideoFrames, VideoFrame},
    ResultType,
};
use videotoolbox::compression::CompressionSession;
use videotoolbox::session::Codec;

/// Timescale for presentation timestamps. Milliseconds -- the caller hands us
/// `ms` and nothing downstream depends on a finer unit.
const TIMESCALE: i32 = 1000;

#[derive(Debug, Clone)]
pub struct VtEncoderConfig {
    pub width: usize,
    pub height: usize,
    pub quality: f32,
    pub keyframe_interval: Option<usize>,
}

pub struct VtEncoder {
    session: CompressionSession,
    config: VtEncoderConfig,
    bitrate: u32,
    /// Reused across frames so the per-frame path does not allocate.
    scratch: Vec<u8>,
}

impl VtEncoder {
    /// Same curve the hwcodec path uses, so a given quality slider position
    /// produces a comparable picture whichever encoder is active. Diverging here
    /// would look like a quality regression when the zero-copy path turns on.
    pub fn calc_bitrate(width: usize, height: usize, ratio: f32) -> u32 {
        let base = base_bitrate(width as _, height as _) as f32 * ratio;
        let threshold = 2000.0;
        let decay_rate = 0.001;
        let factor: f32 = if base > threshold {
            1.0 + 0.5 / (1.0 + (base - threshold) * decay_rate)
        } else {
            1.5
        };
        (base * factor) as u32
    }

    fn build_session(cfg: &VtEncoderConfig, bitrate_kbps: u32) -> ResultType<CompressionSession> {
        CompressionSession::builder(cfg.width as i32, cfg.height as i32, Codec::HEVC)
            .with_real_time(true)
            // No B-frames: reordering adds latency and delivers frames out of
            // presentation order, neither of which suits a live desktop stream.
            .with_allow_frame_reordering(false)
            .with_average_bit_rate((bitrate_kbps as i32).saturating_mul(1000))
            .with_expected_frame_rate(30.0)
            .with_max_keyframe_interval(cfg.keyframe_interval.unwrap_or(300) as i32)
            .build()
            .map_err(|e| anyhow!("create VideoToolbox HEVC session: {e:?}"))
    }

    /// Encode one surface. Returns Annex-B bytes plus whether it was a keyframe.
    fn encode_surface(&mut self, surface: &IOSurface, ms: i64) -> ResultType<(Vec<u8>, bool)> {
        let frame = self
            .session
            .encode(surface, (ms, TIMESCALE))
            .map_err(|e| anyhow!("VideoToolbox encode failed: {e:?}"))?;
        let sample = frame
            .cm_sample_buffer()
            .ok_or_else(|| anyhow!("encoder dropped the frame"))?;

        let key = bitstream::is_keyframe(sample);
        let ps = bitstream::parameter_sets(sample, true);
        let nal_len = ps.as_ref().map(|p| p.nal_length_size).unwrap_or(4);

        self.scratch.clear();
        // Parameter sets ride ahead of every keyframe. VideoToolbox keeps them out
        // of the bitstream, and a client joining mid-session (or recovering from
        // loss) has nothing to configure its decoder with otherwise.
        if key {
            if let Some(ps) = &ps {
                annexb::append_parameter_sets(&ps.sets, &mut self.scratch);
            }
        }
        annexb::append_annexb(&frame.data, nal_len, &mut self.scratch)
            .map_err(|e| anyhow!("Annex-B conversion: {e}"))?;
        Ok((std::mem::take(&mut self.scratch), key))
    }
}

impl EncoderApi for VtEncoder {
    fn new(cfg: EncoderCfg, _i444: bool) -> ResultType<Self>
    where
        Self: Sized,
    {
        match cfg {
            EncoderCfg::VTENC(config) => {
                let bitrate = Self::calc_bitrate(config.width, config.height, config.quality);
                let session = Self::build_session(&config, bitrate)?;
                log::info!(
                    "VideoToolbox zero-copy encoder: {}x{} @ {} kbps",
                    config.width,
                    config.height,
                    bitrate
                );
                Ok(Self {
                    session,
                    config,
                    bitrate,
                    scratch: Vec::new(),
                })
            }
            _ => bail!("VtEncoder requires EncoderCfg::VTENC"),
        }
    }

    fn encode_to_message(&mut self, input: EncodeInput, ms: i64) -> ResultType<VideoFrame> {
        let surface = input
            .surface()
            .with_context(|| "VtEncoder needs a Surface input")?;
        let (data, key) = self.encode_surface(surface, ms)?;
        if data.is_empty() {
            bail!("empty bitstream");
        }
        let mut vf = VideoFrame::new();
        vf.set_h265s(EncodedVideoFrames {
            frames: vec![EncodedVideoFrame {
                data: Bytes::from(data),
                key,
                pts: ms,
                ..Default::default()
            }]
            .into(),
            ..Default::default()
        });
        Ok(vf)
    }

    /// Only meaningful for the RAM path. The zero-copy path never converts, but
    /// the trait requires an answer, so report what the surface actually is.
    fn yuvfmt(&self) -> EncodeYuvFormat {
        EncodeYuvFormat {
            pixfmt: Pixfmt::NV12,
            w: self.config.width,
            h: self.config.height,
            stride: vec![self.config.width, self.config.width],
            u: self.config.width * self.config.height,
            v: 0,
        }
    }

    #[cfg(feature = "vram")]
    fn input_texture(&self) -> bool {
        // Not a D3D11 texture -- that flag drives the Windows VRAM path. The
        // surface input is selected by the encoder type, not by this.
        false
    }

    fn input_surface(&self) -> bool {
        true
    }

    fn set_quality(&mut self, ratio: f32) -> ResultType<()> {
        let bitrate = Self::calc_bitrate(self.config.width, self.config.height, ratio);
        if bitrate == 0 {
            bail!("invalid bitrate");
        }
        // VideoToolbox takes a live bitrate change, but the crate's builder owns
        // that property, so rebuild the session. Rebuilding costs a keyframe --
        // acceptable for a quality change, which is user-initiated and rare.
        self.config.quality = ratio;
        self.session = Self::build_session(&self.config, bitrate)?;
        self.bitrate = bitrate;
        Ok(())
    }

    fn bitrate(&self) -> u32 {
        self.bitrate
    }

    fn support_changing_quality(&self) -> bool {
        true
    }

    fn latency_free(&self) -> bool {
        // Matches how the hwcodec path classifies videotoolbox: the encoder can
        // hold a frame before emitting it, so the caller must not assume
        // one-in-one-out.
        false
    }

    fn is_hardware(&self) -> bool {
        true
    }

    fn disable(&self) {
        // Nothing cached to invalidate: unlike hwcodec there is no probed codec
        // list on disk. Falling back is a matter of the selection logic choosing
        // the RAM encoder next time.
    }
}
