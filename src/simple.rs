//! # simple — minimal getting-started example
//!
//! Loads the classic cameraman test image (grayscale PNG), converts it to
//! linear RGB, tints it by boosting the red channel, and writes the result
//! back to disk.
//!
//! ```text
//! cargo run --bin simple
//! ```

use std::fs;

use fovea::image::{ContiguousImageMut, ImageView};
use fovea::pixel::{MonoF32, RgbF32, Srgb8};
use fovea::transform::{Broadcast, ConvertPixelExt, SrgbGamma, convert_image};
use fovea_io::png::{self, PngEncodeOptions, PngImage};

fn main() {
    // ── 1. Load the PNG ──────────────────────────────────────────────────
    let bytes = fs::read("data/cameraman.png").expect("failed to read cameraman.png");
    let decoded = png::decode(&bytes).expect("failed to decode PNG");

    // ── 2. Convert to linear RGB ─────────────────────────────────────────
    // The cameraman image is 8-bit grayscale, which decodes as SrgbMono8.
    // SrgbGamma linearises it to f32, then Broadcast spreads the single
    // channel into RgbF32.
    let PngImage::SrgbMono8(mono) = decoded.image else {
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

    fs::write("data/cameraman_tinted.png", &out).expect("failed to write cameraman_tinted.png");

    println!(
        "wrote cameraman_tinted.png ({}×{})",
        mono.width(),
        mono.height()
    );
}
