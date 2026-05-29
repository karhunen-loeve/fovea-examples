//! # show_srgb — display an sRGB image with the Identity strategy
//!
//! Loads the Terrace sample image (grayscale JPEG), converts it to `Srgb8`,
//! and displays it in a debug window using `Identity`.
//!
//! ```text
//! cargo run --bin show_srgb
//! ```
//!
//! Press any key or close the window to exit.

use std::fs;

use fovea::image::Image;
use fovea::pixel::Srgb8;
use fovea::transform::{Broadcast, ConvertPixelExt, SrgbGamma, convert_image};
use fovea_io::jpeg::{self, JpegImage};

use fovea_display::{Identity, show};

fn main() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/data/Terrace.jpg");
    let bytes = fs::read(path).unwrap_or_else(|e| {
        eprintln!("Failed to read {path}: {e}");
        eprintln!("Run this example from the fovea-examples repository root.");
        std::process::exit(1);
    });

    let decoded = jpeg::decode(&bytes).expect("failed to decode JPEG");

    // Terrace is an 8-bit grayscale JPEG (SrgbMono8).
    // Convert: SrgbMono8 → f32 (linearise) → Srgb8 (broadcast + re-encode).
    let JpegImage::SrgbMono8(mono) = decoded.image else {
        panic!("expected SrgbMono8, got a different pixel format");
    };

    let srgb: Image<Srgb8> = convert_image(
        &mono,
        SrgbGamma
            .then::<fovea::pixel::MonoF32, _>(Broadcast)
            .then::<fovea::pixel::RgbF32, _>(SrgbGamma),
    );

    println!(
        "Displaying {}×{} sRGB image — press any key to close",
        fovea::image::ImageView::width(&srgb),
        fovea::image::ImageView::height(&srgb),
    );

    show("show_srgb", &srgb, Identity);
}
