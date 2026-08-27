//! # fast — the FAST segment-test corner detector
//!
//! Demonstrates `fovea::features::detect::fast` end to end, and puts it
//! side by side with the structure-tensor detector from the `harris` example
//! so the two families can be compared on one frame:
//!
//! 1. Load the Terrace sample and linearise it to `MonoF32` with `SrgbGamma`.
//! 2. Detect with `fast` at a threshold expressed in **contrast**, not in
//!    response units — the practical difference from Harris, which needs a
//!    map-relative calibration before a threshold means anything.
//! 3. Sweep the arc length `n` (FAST-9 through FAST-12) to show what "more
//!    selective" costs and buys.
//! 4. Show the border policy as a real choice: `Skip` refuses the 3-pixel
//!    margin where the ring does not fit, `Clamp` extrapolates into it.
//! 5. Run Shi-Tomasi on the same frame and report how many detections the two
//!    families agree on, and how long each took.
//! 6. Print the segment test's own localization limit on a synthetic square:
//!    the score saturates, several pixels tie, and the tie-break reports the
//!    cluster's raster-first member.
//!
//! ```text
//! cargo run --bin fast
//! ```
//!
//! Press any key or close any window to exit.

use std::fs;
use std::time::Instant;

use fovea::border::{Clamp, Skip};
use fovea::features::detect::{
    CornerParams, FastParams, SegmentTest, ShiTomasi, corner_response_map, detect_corners, fast,
    fast_score_at, fast_score_map,
};
use fovea::features::{Corner, HasPosition, HasResponse, retain_top_n};
use fovea::image::{Image, ImageView};
use fovea::pixel::{MonoF32, Srgb8, SrgbMono8};
use fovea::sigma;
use fovea::transform::{SrgbGamma, convert_image};
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

    // This example times two detectors against each other, and an unoptimised
    // build makes that comparison meaningless — roughly 18× slower here, and
    // not by the same factor for both. Say so rather than printing numbers
    // that invite the wrong conclusion.
    if cfg!(debug_assertions) {
        println!(
            "\n  ⚠ debug build: the timings below are not meaningful.\n    \
             Re-run with `cargo run --release --bin fast` to compare them.\n"
        );
    }

    // ── 1. Linearise: SrgbMono8 → MonoF32 in [0.0, 1.0] linear light ──────────
    let linear: Image<MonoF32> = convert_image(&mono, SrgbGamma);

    // ── 2. Detect ─────────────────────────────────────────────────────────────
    // The threshold is an intensity difference on the image's own scale: this
    // is "8 % contrast", and it would be `20.0` for the same picture as
    // `Mono8`. Nothing has to be calibrated against a response map first,
    // which is the ergonomic difference from the structure-tensor family.
    let threshold = 0.08;
    let arc_length = 9;
    let nms_radius = 6;
    let keep = 200;

    let test = SegmentTest::new(threshold, arc_length).unwrap();
    let params = FastParams::new(test, nms_radius).unwrap();
    println!(
        "FAST-{arc_length}, t = {threshold} ({:.0} % contrast), nms radius = {nms_radius}",
        threshold * 100.0,
    );

    let started = Instant::now();
    let mut corners = fast(&linear, params, &Skip);
    let fast_elapsed = started.elapsed();
    println!(
        "found {} corners in {:.1?} (before ranking)",
        corners.len(),
        fast_elapsed,
    );

    // Ranking is a separate, named step, shared with every other detector.
    retain_top_n(&mut corners, keep);
    if let Some(strongest) = corners.first() {
        let p = strongest.position();
        println!(
            "strongest corner: ({:.0}, {:.0}), score {:.3} — a contrast, not a response",
            p.x,
            p.y,
            strongest.response(),
        );
    }

    // ── 3. What the arc length does ───────────────────────────────────────────
    // On synthetic geometry the arc length is decisive: FAST-9 accepts a right
    // angle and FAST-12 rejects every one of them, because a 90° corner leaves
    // only 11 contiguous ring pixels on the outside. On a photograph it barely
    // moves the count — natural corners are blobs, texture and junctions
    // rather than clean wedges, and those clear long arcs too. Worth printing
    // precisely because the synthetic intuition does not transfer.
    println!("\narc length sweep (t = {threshold}):");
    for n in 9..=12 {
        let sweep = FastParams::new(SegmentTest::new(threshold, n).unwrap(), nms_radius).unwrap();
        println!("  FAST-{n}: {} corners", fast(&linear, sweep, &Skip).len());
    }

    // ── 4. The border is the crate's ordinary vocabulary ──────────────────────
    // `Skip` declines the margin where the radius-3 ring does not fit — the
    // detections there would be made from invented samples. `Clamp` extends
    // the image instead and reports them. `fast_score_at` says which is which
    // in its return type: `None` is "not scored", `Some(0.0)` is "scored, no
    // corner".
    let skipped = fast(&linear, params, &Skip).len();
    let clamped = fast(&linear, params, &Clamp).len();
    println!("\nborder policy: Skip {skipped} corners, Clamp {clamped} corners");
    println!(
        "  score at (1, 1): Skip {:?}, Clamp {:?}",
        fast_score_at(&linear, 1, 1, test, &Skip),
        fast_score_at(&linear, 1, 1, test, &Clamp),
    );

    // ── 5. Against the structure tensor, on the same frame ────────────────────
    let window = sigma!(1.4);
    let shi_map: Image<MonoF32> = corner_response_map(&linear, &ShiTomasi, window);
    let shi_params = CornerParams::try_new(window, 0.05 * max_response(&shi_map), nms_radius)
        .expect("calibrated threshold is finite and the radius is non-zero");

    let started = Instant::now();
    let mut shi_corners = detect_corners(&linear, &ShiTomasi, shi_params);
    let shi_elapsed = started.elapsed();
    retain_top_n(&mut shi_corners, keep);

    println!(
        "\nShi-Tomasi on the same frame: {} corners (top {keep} kept) in {shi_elapsed:.1?}",
        shi_corners.len(),
    );
    println!(
        "  FAST took {fast_elapsed:.1?} — reading 17 samples per pixel does not make it the \
         cheaper detector here; see the module docs on where its time goes"
    );
    println!(
        "  the two agree (within 2 px) on {} of the strongest {keep}",
        agreement(&corners, &shi_corners, 2.0),
    );

    // ── 6. Where the reported corner actually is ──────────────────────────────
    report_saturation();

    // ── Display ───────────────────────────────────────────────────────────────
    let scores = fast_score_map(&linear, test, &Skip);
    let gamma: Image<SrgbMono8> = convert_image(&linear, SrgbGamma);
    let fast_overlay = overlay_corners(&gamma, &corners, Srgb8::new(255, 200, 40));
    let shi_overlay = overlay_corners(&gamma, &shi_corners, Srgb8::new(40, 200, 255));

    println!("\nOpening 4 windows — press any key to close all");
    DebugDisplay::run(move |ctx| {
        ctx.show("1 — Terrace luminance (linear)", &linear, LinearToDisplay);
        ctx.show(
            &format!("2 — FAST-{arc_length} score map (t = {threshold})"),
            &scores,
            AutoContrast::scan_with(&scores, |p| p.0 as f64),
        );
        ctx.show(
            &format!("3 — strongest {keep} FAST corners"),
            &fast_overlay,
            Identity,
        );
        ctx.show(
            &format!("4 — strongest {keep} Shi-Tomasi corners"),
            &shi_overlay,
            Identity,
        );

        match ctx.wait_key() {
            Some(key) => println!("Key pressed: {key:?}"),
            None => println!("All windows closed"),
        }
    });
}

/// The largest response in a map — the calibration reference the
/// structure-tensor docs recommend, and the step FAST does not need.
fn max_response(map: &Image<MonoF32>) -> f32 {
    (0..map.height())
        .flat_map(|y| (0..map.width()).map(move |x| (x, y)))
        .map(|(x, y)| map.pixel_at(x, y).0)
        .fold(f32::NEG_INFINITY, f32::max)
}

/// How many of `a` have a partner in `b` within `tolerance` pixels.
fn agreement(a: &[Corner], b: &[Corner], tolerance: f64) -> usize {
    a.iter()
        .filter(|corner| {
            let p = corner.position();
            b.iter().any(|other| {
                let q = other.position();
                ((p.x - q.x).powi(2) + (p.y - q.y).powi(2)).sqrt() <= tolerance
            })
        })
        .count()
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

/// Print the segment test's localization limit on a fixture with an exactly
/// known answer.
///
/// The square's true corner pixels are (10, 10), (21, 10), (10, 21) and
/// (21, 21). The score **saturates**: once an arc clears the threshold
/// everywhere, six pixels around each corner reach the identical full
/// contrast, and the peak stage's tie-break then reports each tied cluster's
/// raster-first member. For the top-left corner that *is* the corner; for the
/// other three it is up to two pixels along an edge. Unlike the
/// structure-tensor drift, no parameter moves it — which is the point: it is
/// a property of the measure, and the answer to it is a refinement step.
fn report_saturation() {
    let square: Image<MonoF32> = Image::generate(32, 32, |x, y| {
        let inside = (10..22).contains(&x) && (10..22).contains(&y);
        MonoF32::new(if inside { 1.0 } else { 0.0 })
    });

    println!("\nlocalization on a synthetic square (true corners at 10/21):");
    for threshold in [0.05_f32, 0.2, 0.5, 0.9] {
        let params = FastParams::new(SegmentTest::new(threshold, 9).unwrap(), 3).unwrap();
        let corners = fast(&square, params, &Skip);
        let reported: Vec<(i64, i64)> = corners
            .iter()
            .map(|c| (c.position().x as i64, c.position().y as i64))
            .collect();
        println!("  t = {threshold:.2}: {} corners at {reported:?}", corners.len());
    }

    // The tied cluster behind those numbers, for the top-left corner.
    let scores = fast_score_map(&square, SegmentTest::new(0.1, 9).unwrap(), &Skip);
    let tied: Vec<(usize, usize)> = (9..14)
        .flat_map(|y| (9..14).map(move |x| (x, y)))
        .filter(|&(x, y)| scores.pixel_at(x, y).0 > 0.0)
        .collect();
    println!("  pixels tied at the full contrast around (10, 10): {tied:?}");
}
