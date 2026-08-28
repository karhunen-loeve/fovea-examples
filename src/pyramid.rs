//! # pyramid — multi-scale images, and getting a coarse detection back
//!
//! Demonstrates `fovea::image::pyramid` and `fovea::transform::pyramid`, and
//! the one thing a pyramid is actually for: finding something on a small,
//! cheap level and saying where it is in the **full-resolution** frame.
//!
//! 1. `Gaussian.build` → a `GaussianPyramid<MonoF32>`. `max_depth` is an
//!    upper bound, not a promise; the build stops when a level cannot shrink.
//! 2. `pyr_down` and `pyr_up` by hand, to show the pyramid is a container
//!    over two ordinary operations rather than a special kind of image.
//! 3. The **reconstruction residual**: `pyr_up(pyr_down(img))` is not `img`,
//!    and the size of the gap is what a Laplacian pyramid would store.
//! 4. Detect corners on level 2 and lift them into base coordinates with
//!    `Decimated::to_base`. This is the step that needs the level to carry
//!    its sampling geometry, which a bare `Image` does not.
//! 5. Go the other way with `to_local`, and show the round trip is exact.
//! 6. What the lift costs in accuracy: a coarse detection is quantised to
//!    coarse pixels, so lifting multiplies its error by the sampling
//!    distance. The pyramid buys speed, and this is the price.
//!
//! ```text
//! cargo run --bin pyramid
//! ```
//!
//! Press any key or close any window to exit.

use std::fs;

use fovea::features::detect::{
    CornerParams, NmsRadius, ShiTomasi, corner_response_map, detect_corners,
};
use fovea::features::{HasPosition, retain_top_n};
use fovea::image::{Decimated, GaussianPyramid, Image, ImageView, OriginOffset, ScaledImage};
use fovea::pixel::{MonoF32, Srgb8, SrgbMono8};
use fovea::transform::{Gaussian, PyramidMethod, SrgbGamma, convert_image, pyr_down, pyr_up};
use fovea::{CoordinateF64, PixelDistance, Sigma, sigma};
use fovea_display::{DebugDisplay, Identity, LinearToDisplay};
use fovea_io::jpeg::{self, JpegImage};

fn main() {
    // ── Load ─────────────────────────────────────────────────────────────────
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
    let base: Image<MonoF32> = convert_image(&mono, SrgbGamma);
    println!(
        "Terrace {}×{} → MonoF32 linear",
        base.width(),
        base.height()
    );

    // ── 1. Build ─────────────────────────────────────────────────────────────
    // Ask for six levels. Whether six arrive is the image's business: the
    // build clamps rather than erroring, and `depth()` reports what happened.
    let requested = 6;
    let pyramid: GaussianPyramid<MonoF32> = Gaussian.build(&base, requested);
    println!(
        "\nrequested depth {requested}, resolved depth {}",
        pyramid.depth()
    );
    for (i, level) in pyramid.iter().enumerate() {
        let size = level.size();
        println!(
            "  level {i}: {:>4}×{:<4}  {:>9} px  sampling distance {} base px",
            size.width,
            size.height,
            size.width * size.height,
            1u32 << i,
        );
    }
    let total: usize = pyramid
        .iter()
        .map(|l| l.size().width * l.size().height)
        .sum();
    println!(
        "  all levels together: {total} px, {:.2}× the base image",
        total as f64 / (base.width() * base.height()) as f64,
    );

    // ── 2-3. The two operations, and what the round trip loses ───────────────
    // `pyr_down` is a fixed binomial smoothing followed by even-sample
    // decimation, with the border policy pinned to reflect-101. `pyr_up`
    // takes the target size explicitly, because halving is ceiling division
    // and 15 → 8 → 16 would otherwise silently grow the image.
    let half: Image<MonoF32> = pyr_down(&base);
    let restored: Image<MonoF32> =
        pyr_up(&half, base.size()).expect("the target is the size we came from");
    println!(
        "\npyr_down {}×{} → {}×{}, pyr_up back → {}×{}",
        base.width(),
        base.height(),
        half.width(),
        half.height(),
        restored.width(),
        restored.height(),
    );

    let (mean_abs, max_abs) = residual(&base, &restored);
    println!(
        "reconstruction residual: mean |Δ| {mean_abs:.5}, max |Δ| {max_abs:.5} (linear [0,1])"
    );
    println!(
        "  The round trip is lossy by construction: decimation throws away every\n\
         other sample and no interpolation puts it back. That residual is exactly\n\
         what a Laplacian pyramid stores per level, which is why a Laplacian\n\
         pyramid is not smaller than the image it came from."
    );

    // ── 4. Detect coarse, report fine ────────────────────────────────────────
    // `Gaussian.build` produces plain `Image` levels, which carry no scale
    // metadata and pay nothing for it. Lifting a position needs that
    // metadata, so wrap the level in a `ScaledImage` and *state* the
    // convention: `pyr_down` maps coarse pixel k to fine pixel 2k, so the
    // origin offset is (0, 0) and the sampling distance is 2^level.
    let level_index = 2;
    let coarse = pyramid.level(level_index).clone();
    let distance = PixelDistance::try_new(f64::from(1u32 << level_index))
        .expect("a power of two is finite and positive");
    let level = ScaledImage::new(
        coarse,
        distance,
        OriginOffset::ZERO,
        effective_sigma(level_index),
    );

    let window = sigma!(1.4);
    let map: Image<MonoF32> = corner_response_map(level.image(), ShiTomasi, window);
    let peak = max_response(&map);
    let params = CornerParams::try_new(window, 0.05 * peak, NmsRadius::new(4).unwrap())
        .expect("a calibrated threshold is finite and the radius is non-zero");
    let mut corners = detect_corners(level.image(), ShiTomasi, params);
    retain_top_n(&mut corners, 60);
    println!(
        "\ndetected on level {level_index} ({}×{}): {} corners kept",
        level.size().width,
        level.size().height,
        corners.len(),
    );

    let lifted: Vec<CoordinateF64> = corners
        .iter()
        .map(|c| level.to_base(c.position()))
        .collect();
    if let (Some(first), Some(base_pos)) = (corners.first(), lifted.first()) {
        let p = first.position();
        println!(
            "  strongest: level-local ({:.1}, {:.1}) → base ({:.1}, {:.1})",
            p.x, p.y, base_pos.x, base_pos.y,
        );
    }

    // ── 5. The inverse, and that it is exact ─────────────────────────────────
    let round_trip_exact = corners.iter().zip(&lifted).all(|(c, base_pos)| {
        let back = level.to_local(*base_pos);
        let p = c.position();
        (back.x - p.x).abs() < 1e-9 && (back.y - p.y).abs() < 1e-9
    });
    println!(
        "  to_base → to_local round trip exact for all {} corners: {round_trip_exact}",
        corners.len()
    );
    println!(
        "  Both directions are needed and they are not the same job: detection\n\
         lifts *out* of a level, while a descriptor reading a patch around a\n\
         base-frame keypoint comes back *in*."
    );

    // ── 6. What the lift costs ───────────────────────────────────────────────
    report_lift_cost();

    // ── Display ──────────────────────────────────────────────────────────────
    let overlay = overlay_positions(&mono, &lifted);
    let residual_image: Image<MonoF32> = Image::generate(base.width(), base.height(), |x, y| {
        MonoF32::new((base.pixel_at(x, y).0 - restored.pixel_at(x, y).0).abs())
    });
    let levels: Vec<Image<MonoF32>> = pyramid.iter().cloned().collect();

    println!(
        "\nOpening {} windows — press any key to close all",
        levels.len() + 2
    );
    DebugDisplay::run(move |ctx| {
        for (i, level) in levels.iter().enumerate() {
            ctx.show(
                &format!("level {i} — {}×{}", level.width(), level.height()),
                level,
                LinearToDisplay,
            );
        }
        ctx.show(
            "residual — |base − pyr_up(pyr_down(base))|",
            &residual_image,
            fovea_display::AutoContrast::scan_with(&residual_image, |p| p.0 as f64),
        );
        ctx.show(
            &format!("corners found on level {level_index}, drawn in base coordinates"),
            &overlay,
            Identity,
        );
        match ctx.wait_key() {
            Some(key) => println!("Key pressed: {key:?}"),
            None => println!("All windows closed"),
        }
    });
}

/// The σ a level has accumulated, in base-image pixels.
///
/// Each `pyr_down` applies the pinned binomial kernel, whose σ is about
/// 1.0 in *its own* level's pixels; expressed in base pixels that doubles
/// with every octave. This is an approximation and the demo says so: the
/// exact accumulated σ of repeated binomial smoothing plus decimation is
/// not a closed form, and `ScaleLevel` exists precisely so the number is
/// stated by whoever built the level rather than guessed by whoever reads
/// it.
fn effective_sigma(level_index: usize) -> Sigma {
    let base_sigma = 1.0_f32;
    Sigma::try_new(base_sigma * (1u32 << level_index) as f32)
        .expect("a positive scale of a positive sigma is positive")
}

/// Mean and maximum absolute difference between two same-size images.
fn residual(a: &Image<MonoF32>, b: &Image<MonoF32>) -> (f64, f64) {
    let mut sum = 0.0_f64;
    let mut max = 0.0_f64;
    for y in 0..a.height() {
        for x in 0..a.width() {
            let d = (a.pixel_at(x, y).0 - b.pixel_at(x, y).0).abs() as f64;
            sum += d;
            max = max.max(d);
        }
    }
    (sum / (a.width() * a.height()) as f64, max)
}

/// The largest response in a map, for calibrating a threshold.
fn max_response(map: &Image<MonoF32>) -> f32 {
    (0..map.height())
        .flat_map(|y| (0..map.width()).map(move |x| (x, y)))
        .map(|(x, y)| map.pixel_at(x, y).0)
        .fold(f32::NEG_INFINITY, f32::max)
}

/// Detect the same synthetic corner on several levels and print how far the
/// lifted answer lands from the truth.
///
/// The bias is not subtle and it is not noise: a detection on level `n` is
/// an integer coordinate in that level's grid, so lifting it can only ever
/// name a base position that is a multiple of `2^n`. Half that spacing is
/// the floor on the error before anything else goes wrong. Refinement runs
/// on the level you detected on, not on the lifted number.
fn report_lift_cost() {
    let square: Image<MonoF32> = Image::generate(128, 128, |x, y| {
        let inside = (40..88).contains(&x) && (40..88).contains(&y);
        MonoF32::new(if inside { 1.0 } else { 0.0 })
    });
    let pyramid: GaussianPyramid<MonoF32> = Gaussian.build(&square, 4);
    let window = sigma!(1.2);

    println!("\ncost of detecting coarse (true top-left corner at base 39.5, 39.5):");
    for index in 0..pyramid.depth() {
        let image = pyramid.level(index);
        if image.width() < 16 || image.height() < 16 {
            println!("  level {index}: too small to detect on, skipped");
            continue;
        }
        let map: Image<MonoF32> = corner_response_map(image, ShiTomasi, window);
        let peak = max_response(&map);
        let params = CornerParams::try_new(window, 0.25 * peak, NmsRadius::new(2).unwrap())
            .expect("a calibrated threshold is finite and the radius is non-zero");
        let corners = detect_corners(image, ShiTomasi, params);
        let Some(top_left) = corners.iter().min_by(|a, b| {
            let (pa, pb) = (a.position(), b.position());
            (pa.x + pa.y).total_cmp(&(pb.x + pb.y))
        }) else {
            println!("  level {index}: no corner above threshold");
            continue;
        };

        let distance = PixelDistance::try_new(f64::from(1u32 << index)).expect("positive");
        let level = ScaledImage::new(
            image.clone(),
            distance,
            OriginOffset::ZERO,
            effective_sigma(index),
        );
        let lifted = level.to_base(top_left.position());
        let error = ((lifted.x - 39.5).powi(2) + (lifted.y - 39.5).powi(2)).sqrt();
        println!(
            "  level {index} ({:>3}×{:<3}): local ({:.0}, {:.0}) → base ({:>5.1}, {:>5.1}), {error:.2} px from truth, quantisation floor {:.2} px",
            image.width(),
            image.height(),
            top_left.position().x,
            top_left.position().y,
            lifted.x,
            lifted.y,
            0.5 * distance.get() * std::f64::consts::SQRT_2,
        );
    }

    println!(
        "  The error is larger than the quantisation floor at every level, and the\n\
         gap is not the pyramid's fault: the structure-tensor response peak drifts\n\
         inward as its window grows, which the `harris` example measures on this\n\
         same square. Two independent biases stack here, and only one of them\n\
         shrinks if you detect on a finer level."
    );
}

/// Draw a cross at every lifted position on a colour copy of the source.
///
/// Uses `fovea::draw::Crosshair`, so this is the crate's drawing API rather
/// than example scaffolding.
fn overlay_positions(base: &Image<SrgbMono8>, positions: &[CoordinateF64]) -> Image<Srgb8> {
    use fovea::draw::{Crosshair, Drawable};

    let mut out: Image<Srgb8> = Image::generate(base.width(), base.height(), |x, y| {
        let v = base.pixel_at(x, y).0.0;
        Srgb8::new(v, v, v)
    });
    for p in positions {
        Crosshair {
            center: (p.x.round() as i32, p.y.round() as i32).into(),
            arm_length: 6,
            color: Srgb8::new(255, 60, 60),
        }
        .draw_into(&mut out);
    }
    out
}
