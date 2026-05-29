//! # simple — minimal getting-started example
//!
//! Loads the Terrace sample image (grayscale JPEG), converts it to linear RGB,
//! tints it by boosting the red channel, and writes the result back to disk.
//!
//! ```text
//! cargo run --bin simple
//! ```

use std::fs;

use fovea::image::{ContiguousImageMut, ImageView};
use fovea::pixel::{MonoF32, RgbF32, Srgb8};
use fovea::transform::{Broadcast, ConvertPixelExt, SrgbGamma, convert_image};
use fovea_io::jpeg::{self, JpegImage};
use fovea_io::png::{self, PngEncodeOptions};

fn main() {
    // ── 1. Load the JPEG ─────────────────────────────────────────────────
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/data/Terrace.jpg");
    let bytes = fs::read(input).expect("failed to read Terrace.jpg");
    let decoded = jpeg::decode(&bytes).expect("failed to decode JPEG");

    // ── 2. Convert to linear RGB ─────────────────────────────────────────
    // Terrace is an 8-bit grayscale JPEG, which decodes as SrgbMono8.
    // SrgbGamma linearises it to f32, then Broadcast spreads the single
    // channel into RgbF32.
    let JpegImage::SrgbMono8(mono) = decoded.image else {
        panic!("expected SrgbMono8, got a different pixel format");
    };

    let mut linear: fovea::image::Image<RgbF32> =
        convert_image(&mono, SrgbGamma.then::<MonoF32, _>(Broadcast));

    // ── 3. Tint: boost the red channel ───────────────────────────────────
    for px in linear.as_mut_slice() {
        px.r = (px.r * 1.5).min(1.0);
    }

    // ── 4. Convert back to sRGB and save ─────────────────────────────────
    let srgb: fovea::image::Image<Srgb8> = convert_image(&linear, SrgbGamma);
    let out = png::encode(&srgb, &PngEncodeOptions::default()).expect("failed to encode PNG");

    let output = concat!(env!("CARGO_MANIFEST_DIR"), "/data/terrace_tinted.png");
    fs::write(output, &out).expect("failed to write terrace_tinted.png");

    println!(
        "wrote terrace_tinted.png ({}×{})",
        mono.width(),
        mono.height()
    );
}
