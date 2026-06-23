//! # adaptive_threshold — local-mean thresholding vs a global cut
//!
//! Demonstrates [`adaptive_threshold`] against a global
//! [`otsu_binary_mask`] on a real photograph with uneven illumination
//! (the Terrace sample — bright sky grading into shadowed foreground):
//!
//! 1. Load the Terrace sample and read its 8-bit luminance as `Mono8`.
//! 2. Threshold it two ways and compare:
//!    - **global Otsu** — one cut for the whole frame. Picks a single
//!      brightness; the illumination gradient then swamps local detail —
//!      whole regions wash out to all-foreground or all-background.
//!    - **adaptive** — each pixel is compared to the mean of its own
//!      `window × window` neighbourhood (`pixel > local_mean − bias`),
//!      so local structure survives regardless of the global gradient.
//! 3. Display the original alongside both masks.
//!
//! The accumulator is named explicitly (`Mono32`), exactly as for
//! `integral_image`. If the image were large enough to overflow a 32-bit
//! accumulator (`255 · W · H > u32::MAX`, i.e. above ~16 Mpx for 8-bit),
//! the call returns [`Error::AccumulatorOverflow`] — an *actionable* error —
//! and we fall back to the wider `Mono64`. That live fallback is the whole
//! point of the explicit accumulator: the failure is loud and fixable, not
//! a silent wrong answer.
//!
//! ```text
//! cargo run --bin adaptive_threshold
//! ```
//!
//! Press any key or close any window to exit.

use std::fs;

use fovea::Error;
use fovea::analyze::histogram::otsu_binary_mask;
use fovea::analyze::threshold::{Bias, adaptive_threshold};
use fovea::image::{BinaryImage, Image, ImageView, RasterImage};
use fovea::pixel::{Mono8, Mono32, Mono64, SrgbMono8};
use fovea_display::{DebugDisplay, Identity, LinearToDisplay};
use fovea_io::jpeg::{self, JpegImage};

/// Local window side length (must be odd). Roughly the scale of the
/// structure to separate from its background.
const WINDOW: usize = 31;
/// Bias on the local mean. Positive lifts the threshold's *acceptance*
/// (foreground = `pixel > local_mean − bias`), trimming faint noise just
/// above the local average.
const BIAS: i64 = 7;

fn main() {
    // ── Load ──────────────────────────────────────────────────────────────────
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/data/Terrace.jpg");
    let bytes = fs::read(path).unwrap_or_else(|e| {
        eprintln!("Failed to read {path}: {e}");
        eprintln!("Run this example from the fovea-examples repository root.");
        std::process::exit(1);
    });
    let decoded = jpeg::decode(&bytes).expect("failed to decode JPEG");
    let JpegImage::SrgbMono8(srgb) = decoded.image else {
        panic!("expected SrgbMono8, got a different pixel format");
    };
    let (w, h) = (srgb.width(), srgb.height());
    println!("Terrace {w}×{h} (SrgbMono8)");

    // ── 1. Read the gamma-encoded luminance byte as a plain Mono8 intensity ───
    // Thresholding operates on displayed brightness here, so the sRGB byte is
    // used directly (no linearisation) — what a global threshold would see.
    let mono: Image<Mono8> = Image::generate(w, h, |x, y| Mono8::new(srgb.pixel_at(x, y).0.0));

    // ── 2a. Global Otsu — one cut for the whole frame ─────────────────────────
    let (otsu_t, global) = otsu_binary_mask(&mono).expect("otsu over a single channel");
    println!("global Otsu threshold = {otsu_t}");

    // ── 2b. Adaptive — per-pixel local mean over a WINDOW×WINDOW neighbourhood ─
    // Accumulator named explicitly (Mono32). If the image is too large for a
    // 32-bit accumulator, the error is actionable — widen to Mono64.
    println!("adaptive: window = {WINDOW}, bias = {BIAS}");
    let adaptive = match adaptive_threshold::<_, Mono32>(&mono, WINDOW, Bias::new(BIAS)) {
        Ok(mask) => mask,
        Err(Error::AccumulatorOverflow {
            required_capacity,
            accumulator_capacity,
        }) => {
            println!(
                "  Mono32 too narrow (needs {required_capacity}, holds {accumulator_capacity}); \
                 retrying with Mono64"
            );
            adaptive_threshold::<_, Mono64>(&mono, WINDOW, Bias::new(BIAS))
                .expect("Mono64 holds any realistic 8-bit image sum")
        }
        Err(e) => panic!("unexpected error: {e}"),
    };

    println!(
        "foreground pixels — global Otsu: {}, adaptive: {}",
        count_true(&global),
        count_true(&adaptive),
    );

    // ── 3. Promote each mask to a displayable image and show all stages ───────
    let global_display = mask_to_display(&global);
    let adaptive_display = mask_to_display(&adaptive);

    println!("Opening 3 windows — press any key to close all");
    DebugDisplay::run(move |ctx| {
        ctx.show("1 — Terrace luminance", &mono, LinearToDisplay);
        ctx.show(
            &format!("2 — Global Otsu (t = {otsu_t}) — gradient swamps detail"),
            &global_display,
            Identity,
        );
        ctx.show(
            &format!("3 — Adaptive (window {WINDOW}, bias {BIAS}) — local detail kept"),
            &adaptive_display,
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

/// Render a binary mask as `SrgbMono8`: foreground white, background black.
fn mask_to_display(mask: &BinaryImage) -> Image<SrgbMono8> {
    Image::generate(mask.width(), mask.height(), |x, y| {
        SrgbMono8::new(if mask.pixel_at(x, y) { 255 } else { 0 })
    })
}
