//! Probe: which hardware encoders/decoders does this machine actually expose?
//! Answers whether H265 (HEVC) VideoToolbox encoding is available on this Mac.
fn main() {
    println!("{}", scrap::hwcodec::check_available_hwcodec());
}
