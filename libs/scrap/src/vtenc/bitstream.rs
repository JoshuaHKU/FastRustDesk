//! Pulling Annex-B out of what VideoToolbox actually returns.
//!
//! `annexb.rs` holds the pure rewriting logic; this is the part that has to talk
//! to CoreMedia: fetch the parameter sets out of the format description, and
//! decide whether a frame is a keyframe.

use apple_cf::cm::CMSampleBuffer;
use apple_cf::raw;

/// Parameter sets (VPS, SPS, PPS for HEVC; SPS, PPS for H.264) plus the NAL
/// length prefix size the bitstream uses.
pub struct ParameterSets {
    pub sets: Vec<Vec<u8>>,
    pub nal_length_size: usize,
}

/// Read the parameter sets from a sample buffer's format description.
///
/// VideoToolbox keeps these *out* of the bitstream -- they live only here -- so
/// without this step a decoder joining the stream has nothing to configure
/// itself with, and every frame decodes to nothing.
pub fn parameter_sets(sample: &CMSampleBuffer, hevc: bool) -> Option<ParameterSets> {
    let desc = sample.format_description()?;
    let desc_ptr = desc.as_ptr();

    // The first call is only for the count and the length size; the pointer and
    // size it returns describe set 0, which we re-read in the loop anyway.
    let mut count: usize = 0;
    let mut nal_len: std::ffi::c_int = 0;
    let mut ptr: *const u8 = std::ptr::null();
    let mut size: usize = 0;
    let status = unsafe {
        if hevc {
            raw::CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
                desc_ptr.cast(),
                0,
                &mut ptr,
                &mut size,
                &mut count,
                &mut nal_len,
            )
        } else {
            raw::CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                desc_ptr.cast(),
                0,
                &mut ptr,
                &mut size,
                &mut count,
                &mut nal_len,
            )
        }
    };
    if status != 0 || count == 0 {
        return None;
    }

    let mut sets = Vec::with_capacity(count);
    for i in 0..count {
        let mut p: *const u8 = std::ptr::null();
        let mut s: usize = 0;
        let st = unsafe {
            if hevc {
                raw::CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
                    desc_ptr.cast(),
                    i,
                    &mut p,
                    &mut s,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            } else {
                raw::CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                    desc_ptr.cast(),
                    i,
                    &mut p,
                    &mut s,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            }
        };
        if st != 0 || p.is_null() || s == 0 {
            return None;
        }
        // SAFETY: the pointer belongs to the format description, which `desc`
        // keeps alive for this scope; we copy out before returning.
        sets.push(unsafe { std::slice::from_raw_parts(p, s) }.to_vec());
    }
    Some(ParameterSets {
        sets,
        nal_length_size: if nal_len > 0 { nal_len as usize } else { 4 },
    })
}

/// Is this frame a sync sample (keyframe)?
///
/// CoreMedia expresses it the other way round: an attachment `NotSync = true`
/// means "not a keyframe". Absence of the attachment means it *is* one, which is
/// why the default here is `true` rather than `false` -- getting this backwards
/// would attach parameter sets to the wrong frames.
pub fn is_keyframe(sample: &CMSampleBuffer) -> bool {
    unsafe {
        let arr = raw::CMSampleBufferGetSampleAttachmentsArray(sample.as_ptr().cast(), 0);
        if arr.is_null() {
            return true;
        }
        if raw::CFArrayGetCount(arr) < 1 {
            return true;
        }
        let dict = raw::CFArrayGetValueAtIndex(arr, 0);
        if dict.is_null() {
            return true;
        }
        let mut value: *const std::ffi::c_void = std::ptr::null();
        let found = raw::CFDictionaryGetValueIfPresent(
            dict.cast(),
            raw::kCMSampleAttachmentKey_NotSync.cast(),
            &mut value,
        );
        if found == 0 || value.is_null() {
            return true;
        }
        // Present and true => not a sync sample.
        raw::CFBooleanGetValue(value.cast()) == 0
    }
}
