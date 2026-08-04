//! # canny — the composed Canny edge detector
//!
//! Demonstrates [`canny`] end to end, and — because every stage is a public
//! function — rebuilds the same pipeline by hand so each intermediate can be
//! displayed:
//!
//! 1. Load the Terrace sample and linearise it to `MonoF32` with `SrgbGamma`.
//! 2. Gaussian-blur with a true `sigma`, then take Scharr `Gx` / `Gy`.
//! 3. Fuse into gradient **magnitude** and **direction**.
//! 4. **Non-maximum suppression** thins the magnitude ridge to one pixel.
//! 5. **Hysteresis threshold** links weak edges to strong ones → `BinaryImage`.
//!
//! The final hand-built mask is identical to the one-call
//! `canny(&linear, low, high, sigma)`, shown side by side.
//!
//! ```text
//! cargo run --bin canny
//! ```
//!
//! Press any key or close any window to exit.

use std::fs;

use fovea::analyze::edge::canny;
use fovea::analyze::threshold::hysteresis_threshold;
use fovea::border::Clamp;
use fovea::image::{BinaryImage, Image, ImageView, RasterImage};
use fovea::Sigma;
use fovea::pixel::{MonoF32, SrgbMono8};
use fovea::transform::{
    SrgbGamma, convert_image, gaussian_blur, gradient_direction, gradient_magnitude,
    non_maximum_suppression, scharr_x, scharr_y,
};
use fovea_display::{AutoContrast, DebugDisplay, Identity, LinearToDisplay};
use fovea_io::jpeg::{self, JpegImage};

fn main() {
    // ── Load ──────────────────────────────────────────────────────────────────
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/data/Terrace.jpg");
    let bytes = fs::read(path).unwrap_or_else(|e| {
        eprintln!("Failed to read {path}: {e}");
        eprintln!("Run this example from the fovea-examples repository root.");
        std::process::exit(1);
    });
    let decoded = jpeg::decode(&bytes).expect("failed to decode JPEG");
    let JpegImage::SrgbMono8(mono) = decoded.image else {
        panic!("expected SrgbMono8, got a different pixel format");
    };
    let (w, h) = (mono.width(), mono.height());
    println!("Terrace {w}×{h} (SrgbMono8)");

    // ── Parameters ────────────────────────────────────────────────────────────
    // `sigma` is a true Gaussian standard deviation; `low`/`high` are absolute
    // gradient-magnitude thresholds (stable across sigma because the blur
    // preserves brightness).
    let sigma = Sigma::new(2.4);
    let low = 0.06_f32;
    let high = 0.26_f32;
    println!("sigma = {}, low = {low}, high = {high}", sigma.get());

    // ── 1. Linearise: SrgbMono8 → MonoF32 in [0.0, 1.0] linear light ──────────
    let linear: Image<MonoF32> = convert_image(&mono, SrgbGamma);

    // ── 2-5. Rebuild the pipeline by hand to expose every intermediate ───────
    let blurred: Image<MonoF32> = gaussian_blur(&linear, sigma, &Clamp);
    let gx = scharr_x(&blurred, &Clamp);
    let gy = scharr_y(&blurred, &Clamp);
    let magnitude = gradient_magnitude(&gx, &gy).expect("gx/gy share a size");
    let direction = gradient_direction(&gx, &gy).expect("gx/gy share a size");
    let thinned = non_maximum_suppression(&magnitude, &direction)
        .expect("magnitude and direction share a size");
    let edges_manual = hysteresis_threshold(&thinned, low, high);

    // ── The one-call form — identical result ──────────────────────────────────
    let edges = canny(&linear, low, high, sigma);
    assert_eq!(
        count_true(&edges),
        count_true(&edges_manual),
        "the orchestrator and the hand-built pipeline must agree",
    );
    println!("edge pixels: {} of {}", count_true(&edges), w * h);

    // ── Display every stage ────────────────────────────────────────────────────
    let edge_display = mask_to_display(&edges);

    println!("Opening 6 windows — press any key to close all");
    DebugDisplay::run(move |ctx| {
        ctx.show("1 — Terrace luminance (linear)", &linear, LinearToDisplay);
        ctx.show(
            &format!("2 — Gaussian blur (σ = {})", sigma.get()),
            &blurred,
            LinearToDisplay,
        );
        ctx.show(
            "3 — Gradient magnitude √(gx²+gy²)",
            &magnitude,
            AutoContrast::scan_with(&magnitude, |p| p.0 as f64),
        );
        ctx.show(
            "4 — Gradient direction atan2(gy,gx)",
            &direction,
            AutoContrast::scan_with(&direction, |p| p.0 as f64),
        );
        ctx.show(
            "5 — Non-maximum suppression (thinned)",
            &thinned,
            AutoContrast::scan_with(&thinned, |p| p.0 as f64),
        );
        ctx.show(
            &format!("6 — Canny edges (low={low}, high={high})"),
            &edge_display,
            Identity,
        );

        match ctx.wait_key() {
            Some(key) => println!("Key pressed: {key:?}"),
            None => println!("All windows closed"),
        }
    });
}

/// Number of `true` pixels in a binary mask.
fn count_true(mask: &BinaryImage) -> usize {
    (0..mask.height())
        .map(|y| mask.row(y).iter().filter(|&&b| b).count())
        .sum()
}

/// Render a binary mask as `SrgbMono8`: edge pixels white, background black.
fn mask_to_display(mask: &BinaryImage) -> Image<SrgbMono8> {
    Image::generate(mask.width(), mask.height(), |x, y| {
        SrgbMono8::new(if mask.pixel_at(x, y) { 255 } else { 0 })
    })
}
