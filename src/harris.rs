//! # harris — Harris and Shi-Tomasi corner detection
//!
//! Demonstrates `fovea::features::detect` end to end, and — because every
//! stage is a public function — rebuilds the same pipeline by hand with a
//! *different gradient operator* to show that the operator, the window and
//! the response are three separate choices:
//!
//! 1. Load the Terrace sample and linearise it to `MonoF32` with `SrgbGamma`.
//! 2. Compute the **response map** for `Harris` and for `ShiTomasi`.
//! 3. **Calibrate** the threshold against each map's own maximum — the
//!    response is a fourth power (Harris) or a square (Shi-Tomasi) of
//!    contrast, so an absolute threshold cannot be guessed.
//! 4. `detect_corners` → threshold + local-maximum selection → `Vec<Corner>`,
//!    then `retain_top_n` for the strongest few.
//! 5. Rebuild step 4 by hand over a `StructureTensor` built from **Scharr**
//!    gradients instead of the pinned Sobel.
//! 6. Print the localization drift: on a synthetic square, the response peak
//!    sits on the corner pixel for a small window and moves *inward* as the
//!    window grows.
//!
//! ```text
//! cargo run --bin harris
//! ```
//!
//! Press any key or close any window to exit.

use std::fs;

use fovea::Sigma;
use fovea::border::Clamp;
use fovea::features::detect::{
    CornerParams, NmsRadius, ShiTomasi, StructureTensor, corner_peaks, corner_response_map,
    detect_corners,
};
use fovea::features::{Corner, HasPosition, HasResponse, retain_top_n};
use fovea::image::{Image, ImageView};
use fovea::pixel::{MonoF32, Srgb8, SrgbMono8};
use fovea::transform::{SrgbGamma, convert_image, scharr_x, scharr_y};
use fovea::{harris, sigma};
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
    // `window` is the σ of the Gaussian that integrates the gradient products:
    // larger means more noise-robust and less precisely localized (see the
    // drift section at the end). `nms_radius` is the minimum separation
    // between two reported corners. `k` is Harris' sensitivity — its type
    // rejects the two silent failure modes, k ≤ 0 (edges become rewards) and
    // k ≥ 0.25 (nothing can ever fire).
    let window = sigma!(1.4);
    let nms_radius = NmsRadius::new(6).unwrap();
    let keep = 200;
    let harris = harris!(0.04);
    println!(
        "window σ = {}, nms radius = {}, k = {}",
        window.get(),
        nms_radius.get(),
        harris.k(),
    );

    // ── 1. Linearise: SrgbMono8 → MonoF32 in [0.0, 1.0] linear light ──────────
    let linear: Image<MonoF32> = convert_image(&mono, SrgbGamma);

    // ── 2-3. Response maps, and a threshold calibrated against each ──────────
    // The two responses are not comparable with each other: Harris is a
    // fourth power of contrast and Shi-Tomasi a square, so 3 % of one map's
    // peak is not 3 % of the other's. Calibrating per map is the only recipe
    // that survives a change of measure, operator, pixel type or σ.
    let harris_map: Image<MonoF32> = corner_response_map(&linear, harris, window);
    let shi_map: Image<MonoF32> = corner_response_map(&linear, ShiTomasi, window);
    let harris_peak = max_response(&harris_map);
    let shi_peak = max_response(&shi_map);
    println!("peak response: Harris {harris_peak:.4}, Shi-Tomasi {shi_peak:.4}");

    let harris_params = CornerParams::try_new(window, 0.02 * harris_peak, nms_radius)
        .expect("calibrated threshold is finite and the radius is non-zero");
    let shi_params = CornerParams::try_new(window, 0.05 * shi_peak, nms_radius)
        .expect("calibrated threshold is finite and the radius is non-zero");

    // ── 4. Detect, then rank ──────────────────────────────────────────────────
    // `detect_corners` returns every peak above the threshold, in raster
    // order. Ranking is a separate, named step, so "the strongest 200" is
    // reproducible: `retain_top_n` breaks response ties on (y, x).
    let mut harris_corners = detect_corners(&linear, harris, harris_params);
    let mut shi_corners = detect_corners(&linear, ShiTomasi, shi_params);
    println!(
        "above threshold: Harris {}, Shi-Tomasi {}",
        harris_corners.len(),
        shi_corners.len(),
    );

    retain_top_n(&mut harris_corners, keep);
    retain_top_n(&mut shi_corners, keep);
    if let Some(strongest) = harris_corners.first() {
        let p = strongest.position();
        println!(
            "strongest Harris corner: ({:.1}, {:.1}) response {:.4}",
            p.x,
            p.y,
            strongest.response(),
        );
    }

    // ── 5. The same detection, hand-built over a Scharr gradient ─────────────
    // `corner_response_map` pins Sobel because that is what other libraries'
    // `cornerHarris` uses. Scharr has better rotational symmetry; swapping it
    // in is composition, not a fork — and the small difference in the corner
    // count is the honest evidence that the operator is a real choice.
    let gx = scharr_x(&linear, &Clamp);
    let gy = scharr_y(&linear, &Clamp);
    let tensor = StructureTensor::from_gradients(&gx, &gy, window).expect("gx and gy share a size");
    let scharr_map = tensor.response(harris);
    let scharr_corners = corner_peaks(&scharr_map, 0.02 * max_response(&scharr_map), nms_radius.get());
    println!(
        "same pipeline with a Scharr gradient: {} corners above threshold",
        scharr_corners.len(),
    );

    // ── 6. Where the reported corner actually is ──────────────────────────────
    report_localization_drift();

    // ── Display ───────────────────────────────────────────────────────────────
    // Corners are marked with `fovea::draw::Crosshair`, which shipped after
    // this example was first written; the markers are an API demonstration.
    let gamma: Image<SrgbMono8> = convert_image(&linear, SrgbGamma);
    let harris_overlay = overlay_corners(&gamma, &harris_corners, Srgb8::new(255, 40, 40));
    let shi_overlay = overlay_corners(&gamma, &shi_corners, Srgb8::new(40, 200, 255));

    println!("Opening 5 windows — press any key to close all");
    DebugDisplay::run(move |ctx| {
        ctx.show("1 — Terrace luminance (linear)", &linear, LinearToDisplay);
        ctx.show(
            &format!("2 — Harris response (k = {})", harris.k()),
            &harris_map,
            AutoContrast::scan_with(&harris_map, |p| p.0 as f64),
        );
        ctx.show(
            "3 — Shi-Tomasi response (λ_min)",
            &shi_map,
            AutoContrast::scan_with(&shi_map, |p| p.0 as f64),
        );
        ctx.show(
            &format!("4 — strongest {keep} Harris corners"),
            &harris_overlay,
            Identity,
        );
        ctx.show(
            &format!("5 — strongest {keep} Shi-Tomasi corners"),
            &shi_overlay,
            Identity,
        );

        match ctx.wait_key() {
            Some(key) => println!("Key pressed: {key:?}"),
            None => println!("All windows closed"),
        }
    });
}

/// The largest response in a map — the calibration reference the module docs
/// recommend over guessing an absolute threshold.
fn max_response(map: &Image<MonoF32>) -> f32 {
    (0..map.height())
        .flat_map(|y| (0..map.width()).map(move |x| (x, y)))
        .map(|(x, y)| map.pixel_at(x, y).0)
        .fold(f32::NEG_INFINITY, f32::max)
}

/// Draw a 9-pixel cross at every corner, on a colour copy of the image.
///
/// Uses `fovea::draw::Crosshair`, so this is the crate's drawing API rather
/// than example scaffolding. It was hand-rolled until `fovea::draw` shipped.
fn overlay_corners(base: &Image<SrgbMono8>, corners: &[Corner], colour: Srgb8) -> Image<Srgb8> {
    use fovea::draw::{Crosshair, Drawable};

    let mut out: Image<Srgb8> = Image::generate(base.width(), base.height(), |x, y| {
        let v = base.pixel_at(x, y).0.0;
        Srgb8::new(v, v, v)
    });

    for corner in corners {
        // Positions are `f64` in the base-image frame; these detections are
        // pixel-centred, so rounding is exact here.
        let p = corner.position();
        Crosshair {
            center: (p.x.round() as i32, p.y.round() as i32),
            arm_length: 4,
            color: colour,
        }
        .draw_into(&mut out);
    }
    out
}

/// Print how far the reported corner sits from the true one as the
/// structure-tensor window grows.
///
/// A synthetic square gives an exactly known answer: the step between the
/// black field and the white square sits *between* pixels 7 and 8, so the
/// geometric corner is at (7.5, 7.5) and the corner *pixel* is (8, 8). For a
/// small window the response peaks on that pixel; as the window grows the
/// peak moves inward along the diagonal, because a window centred on the
/// corner has half its mass on flat field while one centred slightly inside
/// covers more of both edges. The bias is systematic — it does not average
/// away over frames — which is why refining a detection is a separate step
/// from detecting it, and why fitting a curve to *this* response surface
/// would refine the biased peak rather than remove the bias.
fn report_localization_drift() {
    let square: Image<MonoF32> = Image::generate(24, 24, |x, y| {
        let inside = (8..16).contains(&x) && (8..16).contains(&y);
        MonoF32::new(if inside { 1.0 } else { 0.0 })
    });

    println!("\nlocalization on a synthetic square (true corner at 7.5, 7.5):");
    for sigma in [0.8_f32, 1.0, 1.4, 2.0] {
        let window = Sigma::new(sigma).unwrap();
        let map: Image<MonoF32> = corner_response_map(&square, ShiTomasi, window);
        let params = CornerParams::try_new(window, 0.3 * max_response(&map), NmsRadius::new(3).unwrap())
            .expect("calibrated threshold is finite and the radius is non-zero");
        let corners = detect_corners(&square, ShiTomasi, params);
        let top_left = corners
            .iter()
            .min_by(|a, b| {
                let (pa, pb) = (a.position(), b.position());
                (pa.x + pa.y).total_cmp(&(pb.x + pb.y))
            })
            .expect("a square has corners");
        let p = top_left.position();
        let drift = ((p.x - 7.5).powi(2) + (p.y - 7.5).powi(2)).sqrt();
        println!(
            "  σ = {sigma:.1}: {} corners, top-left at ({:.0}, {:.0}), {drift:.2} px from truth",
            corners.len(),
            p.x,
            p.y,
        );
    }
}
