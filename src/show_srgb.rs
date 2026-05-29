//! # show_srgb — display an sRGB image with the Identity strategy
//!
//! Loads the cameraman test image (grayscale PNG), converts it to `Srgb8`,
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
use fovea_io::png::{self, PngImage};

use fovea_display::{Identity, show};

fn main() {
    let path = "data/cameraman.png";
    let bytes = fs::read(path).unwrap_or_else(|e| {
        eprintln!("Failed to read {path}: {e}");
        eprintln!("Run this example from the fovea-examples repository root.");
        std::process::exit(1);
    });

    let decoded = png::decode(&bytes).expect("failed to decode PNG");

    // The cameraman image is 8-bit grayscale (SrgbMono8).
    // Convert: SrgbMono8 → f32 (linearise) → Srgb8 (broadcast + re-encode).
    let PngImage::SrgbMono8(mono) = decoded.image else {
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
