//! # hysteresis_threshold — double-threshold edge segmentation
//!
//! Demonstrates [`hysteresis_threshold`], the final segmentation stage of a
//! Canny edge detector, on a real gradient-magnitude image:
//!
//! 1. Load the Terrace sample and linearise it to `MonoF32` with `SrgbGamma`.
//! 2. Compute Sobel gradients and fuse them into a gradient-magnitude map.
//! 3. Derive `low` / `high` thresholds as fractions of the peak magnitude
//!    (so the demo adapts to the image rather than hard-coding levels).
//! 4. Produce **three** binary masks — all from the same function — to show
//!    what hysteresis buys you:
//!    - **strong-only** (`low == high`): keep only pixels `>= high`. Clean,
//!      but strong edges break into disconnected fragments.
//!    - **low-only** (`low == low`): keep every pixel `>= low`. Edges stay
//!      connected, but weak texture and noise leak in everywhere.
//!    - **hysteresis** (`low`, `high`): keep a weak pixel only if its
//!      8-connected component touches a strong one — connected edges
//!      *without* the noise.
//!
//! The "strong-only" and "low-only" baselines are just `hysteresis_threshold`
//! called with equal thresholds, so the contrast comes from one API.
//!
//! ```text
//! cargo run --bin hysteresis_threshold
//! ```
//!
//! Press any key or close any window to exit.

use std::fs;

use fovea::analyze::threshold::{HysteresisThresholds, hysteresis_threshold};
use fovea::border::Clamp;
use fovea::image::{BinaryImage, Image, ImageView, RasterImage};
use fovea::pixel::{MonoF32, SrgbMono8};
use fovea::transform::{Magnitude, SrgbGamma, combine_images, convert_image, sobel_x, sobel_y};
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

    // ── 1. Linearise: SrgbMono8 → MonoF32 in [0.0, 1.0] linear light ──────────
    let linear: Image<MonoF32> = convert_image(&mono, SrgbGamma);

    // ── 2. Gradient magnitude √(gx² + gy²) — the Canny pre-stage ──────────────
    let gx = sobel_x(&linear, &Clamp);
    let gy = sobel_y(&linear, &Clamp);
    let magnitude = combine_images(&gx, &gy, Magnitude).expect("gx/gy share a size");

    // ── 3. Pick thresholds relative to the peak magnitude ─────────────────────
    // Hysteresis takes its thresholds as explicit, named arguments (it never
    // infers them). Here the *example* derives sensible levels
    // from the data; in a real pipeline they'd be tuned constants.
    let mut peak = 0.0f32;
    for y in 0..magnitude.height() {
        for &p in magnitude.row(y) {
            peak = peak.max(p.0);
        }
    }
    // Derived from the image, so `try_new` rather than `new`: a degenerate
    // frame (peak 0, or a NaN from an empty magnitude map) is a value to
    // report, not a panic. `new` is for literals.
    let low = 0.08 * peak;
    let high = 0.20 * peak;
    let thresholds = HysteresisThresholds::try_new(low, high)
        .expect("0.08·peak <= 0.20·peak for any finite non-negative peak");
    println!("magnitude peak = {peak:.4}  →  low = {low:.4}, high = {high:.4}");

    // ── 4. Three masks from one function ──────────────────────────────────────
    // Equal thresholds collapse the weak band: every kept component must
    // already contain a `>= t` pixel, so the result is a plain threshold at t.
    let strong_pair = HysteresisThresholds::try_new(high, high).unwrap();
    let low_pair = HysteresisThresholds::try_new(low, low).unwrap();
    let strong_only = hysteresis_threshold(&magnitude, strong_pair);
    let low_only = hysteresis_threshold(&magnitude, low_pair);
    let hysteresis = hysteresis_threshold(&magnitude, thresholds);

    println!(
        "kept pixels — strong-only: {}, low-only: {}, hysteresis: {}",
        count_true(&strong_only),
        count_true(&low_only),
        count_true(&hysteresis),
    );

    // ── 5. Promote each BinaryImage to a displayable SrgbMono8 (white edges) ──
    let strong_display = mask_to_display(&strong_only);
    let low_display = mask_to_display(&low_only);
    let hysteresis_display = mask_to_display(&hysteresis);

    // ── 6. Display all stages simultaneously ──────────────────────────────────
    println!("Opening 5 windows — press any key to close all");
    DebugDisplay::run(move |ctx| {
        ctx.show("1 — Terrace luminance (linear)", &linear, LinearToDisplay);
        ctx.show(
            "2 — Gradient magnitude √(gx²+gy²)",
            &magnitude,
            AutoContrast::scan_with(&magnitude, |p| p.0 as f64),
        );
        ctx.show(
            &format!("3 — Strong only (≥ {high:.3}) — broken edges"),
            &strong_display,
            Identity,
        );
        ctx.show(
            &format!("4 — Low only (≥ {low:.3}) — noisy"),
            &low_display,
            Identity,
        );
        ctx.show(
            "5 — Hysteresis (weak kept iff bridged to strong)",
            &hysteresis_display,
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
