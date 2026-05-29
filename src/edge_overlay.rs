//! # edge_overlay — Sobel edge detection overlaid on the cameraman image
//!
//! Demonstrates the convolution and pixel-wise combinator APIs working together:
//!
//! 1. Load the cameraman test image and linearise it to `MonoF32` with `SrgbGamma`
//! 2. Compute Sobel gradients with `sobel_x` and `sobel_y`
//! 3. Combine them into a gradient-magnitude image with `Magnitude`
//! 4. Blend original and magnitude with `LinearCombine`
//! 5. Display all stages simultaneously in separate windows
//!
//! **Naming note:** `sobel_x` computes `dI/dx` (horizontal gradient), which
//! lights up *vertical* edges (|).  `sobel_y` computes `dI/dy` (vertical
//! gradient), which lights up *horizontal* edges (―).  The name refers to
//! the gradient direction, not the orientation of the visible edge.
//!
//! ```text
//! cargo run --bin edge_overlay
//! ```
//!
//! Press any key or close any window to exit.

use std::fs;

use fovea::border::Clamp;
use fovea::image::Image;
use fovea::pixel::MonoF32;
use fovea::transform::{
    LinearCombine, Magnitude, SrgbGamma, combine_images, convert_image, laplacian, sobel_x, sobel_y,
};
use fovea_display::{AutoContrast, DebugDisplay, LinearToDisplay};
use fovea_io::png::{self, PngImage};

fn main() {
    // ── Load ──────────────────────────────────────────────────────────────────
    let bytes = fs::read("data/cameraman.png").expect("failed to read cameraman.png");
    let decoded = png::decode(&bytes).expect("failed to decode PNG");
    let PngImage::SrgbMono8(mono) = decoded.image else {
        panic!("expected SrgbMono8, got a different pixel format");
    };

    // ── 1. Linearise ──────────────────────────────────────────────────────────
    // SrgbMono8 → MonoF32: removes sRGB gamma, values now in [0.0, 1.0] linear light.
    let linear: Image<MonoF32> = convert_image(&mono, SrgbGamma);

    // ── 2. Sobel gradients ────────────────────────────────────────────────────
    // sobel_x = dI/dx (horizontal gradient) → lights up VERTICAL   edges (|)
    // sobel_y = dI/dy (vertical   gradient) → lights up HORIZONTAL edges (―)
    // Both return Image<MonoF32> with signed values; edge polarity depends on
    // whether brightness increases or decreases across the edge.
    let gx = sobel_x(&linear, &Clamp);
    let gy = sobel_y(&linear, &Clamp);

    let laplace = laplacian(&linear, &Clamp);

    // ── 3. Gradient magnitude: sqrt(gx² + gy²) ───────────────────────────────
    // Magnitude fuses both gradient directions into a single unsigned edge map.
    let magnitude = combine_images(&gx, &gy, Magnitude).unwrap();

    // ── 4. Edge overlay: 70 % original + 30 % edges ──────────────────────────
    // LinearCombine delegates to pixel.scale(), precision stays in MonoF32.
    let overlay =
        combine_images(&linear, &magnitude, LinearCombine { wa: 0.85, wb: 0.15 }).unwrap();

    let laplace_overlay =
        combine_images(&linear, &laplace, LinearCombine { wa: 0.70, wb: 0.30 }).unwrap();

    // ── 5. Display all stages simultaneously ──────────────────────────────────
    println!("Opening 5 windows — press any key to close all");

    DebugDisplay::run(move |ctx| {
        // Original: [0, 1] linear light → sRGB gamma for a perceptually correct display.
        ctx.show("1 — Cameraman (linear)", &linear, LinearToDisplay);

        // Signed gradients: AutoContrast maps zero → mid-grey, ± peaks → black/white.
        ctx.show(
            "2 — Sobel X  (vertical edges |)",
            &gx,
            AutoContrast::scan_with(&gx, |p| p.0 as f64),
        );
        ctx.show(
            "3 — Sobel Y  (horizontal edges ―)",
            &gy,
            AutoContrast::scan_with(&gy, |p| p.0 as f64),
        );
        ctx.show(
            "4 — Laplacian  (edge strength)",
            &laplace,
            AutoContrast::scan_with(&laplace, |p| p.0 as f64),
        );

        // Magnitude is always ≥ 0; AutoContrast stretches to full display range.
        ctx.show(
            "5 — Magnitude  √(gx²+gy²)",
            &magnitude,
            AutoContrast::scan_with(&magnitude, |p| p.0 as f64),
        );

        // Blended result: AutoContrast handles values that exceed [0, 1].
        ctx.show(
            "6 — Overlay  (85 % + 15 % edges)",
            &overlay,
            LinearToDisplay,
        );
        ctx.show(
            "7 — Laplacian Overlay  (70 % + 30 % edges)",
            &laplace_overlay,
            LinearToDisplay,
        );

        match ctx.wait_key() {
            Some(key) => println!("Key pressed: {key:?}"),
            None => println!("All windows closed"),
        }
    });
}
