//! HVCC/AVCC -> Annex-B conversion.
//!
//! VideoToolbox hands back NAL units in the "length-prefixed" layout: each unit
//! is preceded by a big-endian length field (normally 4 bytes), and the
//! parameter sets (VPS/SPS/PPS) are *not* in the bitstream at all -- they live in
//! the `CMFormatDescription`.
//!
//! The rest of RustDesk expects what the ffmpeg VideoToolbox encoder produced:
//! Annex-B, with the parameter sets inlined ahead of every keyframe. hwcodec
//! sets neither `AV_CODEC_FLAG_GLOBAL_HEADER` nor a bitstream filter, which is
//! ffmpeg's default in-band behaviour.
//!
//! Getting this wrong does not fail loudly -- the decoder just produces garbage
//! or nothing -- so the logic lives here as a pure function with unit tests
//! rather than inline in the encoder.

/// Annex-B start code. Four bytes everywhere here: the three-byte form is legal
/// but only worth the saving inside a slice, and mixing them buys nothing.
const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// Rewrite a length-prefixed bitstream in place into Annex-B, appending to `out`.
///
/// `nal_length_size` comes from the format description (`NALUnitHeaderLengthOut`)
/// and is 1, 2 or 4 in practice. Returns an error rather than panicking on a
/// truncated buffer: a malformed frame should drop, not take the session down.
pub fn append_annexb(
    src: &[u8],
    nal_length_size: usize,
    out: &mut Vec<u8>,
) -> Result<(), &'static str> {
    if !matches!(nal_length_size, 1 | 2 | 4) {
        return Err("unsupported NAL length size");
    }
    let mut pos = 0usize;
    while pos < src.len() {
        if pos + nal_length_size > src.len() {
            return Err("truncated NAL length field");
        }
        let mut len = 0usize;
        for i in 0..nal_length_size {
            len = (len << 8) | src[pos + i] as usize;
        }
        pos += nal_length_size;
        if len == 0 {
            // A zero-length unit carries nothing; skipping keeps the walk in step
            // instead of emitting a bare start code the decoder would have to
            // resynchronise past.
            continue;
        }
        let end = pos.checked_add(len).ok_or("NAL length overflow")?;
        if end > src.len() {
            return Err("NAL length exceeds buffer");
        }
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(&src[pos..end]);
        pos = end;
    }
    Ok(())
}

/// Prepend parameter sets, each as its own Annex-B unit.
///
/// Only emitted on keyframes: a decoder joining mid-stream (or after packet
/// loss) needs them to make sense of anything that follows, and RustDesk clients
/// do exactly that when a session starts.
pub fn append_parameter_sets(sets: &[Vec<u8>], out: &mut Vec<u8>) {
    for s in sets {
        if s.is_empty() {
            continue;
        }
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_single_nal() {
        // one 3-byte NAL, 4-byte length prefix
        let src = [0, 0, 0, 3, 0xAA, 0xBB, 0xCC];
        let mut out = Vec::new();
        append_annexb(&src, 4, &mut out).unwrap();
        assert_eq!(out, vec![0, 0, 0, 1, 0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn converts_multiple_nals() {
        let src = [
            0, 0, 0, 2, 0x11, 0x22, // first
            0, 0, 0, 3, 0x33, 0x44, 0x55, // second
        ];
        let mut out = Vec::new();
        append_annexb(&src, 4, &mut out).unwrap();
        assert_eq!(
            out,
            vec![0, 0, 0, 1, 0x11, 0x22, 0, 0, 0, 1, 0x33, 0x44, 0x55]
        );
    }

    #[test]
    fn honours_two_byte_length() {
        let src = [0, 2, 0x77, 0x88];
        let mut out = Vec::new();
        append_annexb(&src, 2, &mut out).unwrap();
        assert_eq!(out, vec![0, 0, 0, 1, 0x77, 0x88]);
    }

    #[test]
    fn skips_zero_length_unit() {
        // zero-length unit between two real ones must not desynchronise the walk
        let src = [0, 0, 0, 0, 0, 0, 0, 1, 0x99];
        let mut out = Vec::new();
        append_annexb(&src, 4, &mut out).unwrap();
        assert_eq!(out, vec![0, 0, 0, 1, 0x99]);
    }

    #[test]
    fn rejects_length_past_end() {
        // claims 9 bytes but only 2 follow -- must be an error, not a panic and
        // not a silent truncation that would emit a corrupt NAL
        let src = [0, 0, 0, 9, 0x01, 0x02];
        let mut out = Vec::new();
        assert!(append_annexb(&src, 4, &mut out).is_err());
    }

    #[test]
    fn rejects_truncated_length_field() {
        let src = [0, 0];
        let mut out = Vec::new();
        assert!(append_annexb(&src, 4, &mut out).is_err());
    }

    #[test]
    fn rejects_bad_length_size() {
        let mut out = Vec::new();
        assert!(append_annexb(&[0, 1, 2], 3, &mut out).is_err());
    }

    #[test]
    fn parameter_sets_each_get_a_start_code() {
        let sets = vec![vec![0x40, 0x01], vec![0x42, 0x01], vec![0x44, 0x01]];
        let mut out = Vec::new();
        append_parameter_sets(&sets, &mut out);
        assert_eq!(
            out,
            vec![0, 0, 0, 1, 0x40, 0x01, 0, 0, 0, 1, 0x42, 0x01, 0, 0, 0, 1, 0x44, 0x01]
        );
    }

    #[test]
    fn empty_parameter_set_is_skipped() {
        let sets = vec![vec![0x40], vec![], vec![0x42]];
        let mut out = Vec::new();
        append_parameter_sets(&sets, &mut out);
        assert_eq!(out, vec![0, 0, 0, 1, 0x40, 0, 0, 0, 1, 0x42]);
    }

    /// A keyframe is parameter sets followed by the frame's own NALs, in that
    /// order -- the shape the decoder has to see.
    #[test]
    fn keyframe_layout() {
        let sets = vec![vec![0x40, 0x01]];
        let src = [0, 0, 0, 2, 0x26, 0x01];
        let mut out = Vec::new();
        append_parameter_sets(&sets, &mut out);
        append_annexb(&src, 4, &mut out).unwrap();
        assert_eq!(out, vec![0, 0, 0, 1, 0x40, 0x01, 0, 0, 0, 1, 0x26, 0x01]);
    }
}
