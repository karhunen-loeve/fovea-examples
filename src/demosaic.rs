//! # demosaic — the CFA pixel family, two demosaic strategies, and white balance
//!
//! Demonstrates `fovea::pixel::bayer` and `fovea::transform::demosaic` on a
//! mosaic built from a real colour image, so every reconstruction has a
//! ground truth to be scored against:
//!
//! 1. Mosaic `Srgb8` down to `BayerRggb8`, which is what a sensor hands over:
//!    one colour sample per pixel, arranged in a 2×2 tile.
//! 2. `BayerPattern::color_at` reads that tile off the *type*, so the demo
//!    never open-codes which pixel is red.
//! 3. `demosaic(&raw, BayerBilinear)` and `demosaic(&raw, MalvarHeCutler)`.
//!    The engine-plus-strategy shape means the algorithm is a value.
//! 4. Score both against the original: mean absolute error per channel.
//! 5. The **grey step**, where the quality claim gets complicated. Malvar–
//!    He–Cutler cuts the peak colour fringe but *widens* the fringed band,
//!    because its window is 5×5 and bilinear's is 3×3. "The better algorithm
//!    has fewer artifacts" is false in one dimension, and this prints both.
//! 6. `white_balance` on the **mosaic**, before demosaicing, which is where a
//!    camera does it and the only place a per-CFA-colour gain is meaningful.
//! 7. Why `demosaic` takes no border argument: reflect-101 is the only
//!    policy in the crate that preserves CFA parity, so it is pinned.
//!
//! ```text
//! cargo run --bin demosaic
//! ```
//!
//! Press any key or close any window to exit.

use std::fs;

use fovea::image::{Image, ImageView};
use fovea::pixel::bayer::{BayerPattern, BayerPixel, BayerRggb8, CfaColor};
use fovea::pixel::{Rgb8, Srgb8};
use fovea::transform::{BayerBilinear, BayerGains, MalvarHeCutler, demosaic, white_balance};
use fovea_display::{DebugDisplay, Identity};
use fovea_io::jpeg::{self, JpegImage};

fn main() {
    // ── Load a colour image to serve as ground truth ─────────────────────────
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/data/Mandrill.jpg");
    let bytes = fs::read(path).unwrap_or_else(|e| {
        eprintln!("Failed to read {path}: {e}");
        eprintln!("Run this example from the fovea-examples repository root.");
        std::process::exit(1);
    });
    let decoded = jpeg::decode(&bytes).expect("failed to decode JPEG");
    let JpegImage::Srgb8(truth) = decoded.image else {
        panic!("expected Srgb8 truecolour, got a different pixel format");
    };
    println!("Mandrill {}×{} (Srgb8)", truth.width(), truth.height());

    // ── 1-2. Mosaic it ───────────────────────────────────────────────────────
    // A sensor keeps one channel per pixel and discards the other two. Which
    // one it keeps is the pattern's business, and `color_at` answers that in
    // a `const fn` off `BayerRggb8::PATTERN` — the demo never writes down the
    // tile itself. That is the difference between item 7 being a testable
    // type family and being an inert tag.
    let pattern = BayerRggb8::PATTERN;
    println!("\npattern {pattern:?}, 2×2 tile = {:?}", pattern.tile());
    let raw = mosaic(&truth, pattern);
    println!(
        "mosaicked to {} — one u8 per pixel, {:.0}% of the data thrown away",
        std::any::type_name::<BayerRggb8>()
            .rsplit("::")
            .next()
            .unwrap_or("BayerRggb8"),
        100.0 * 2.0 / 3.0,
    );

    // ── 3. Two strategies over the same engine ───────────────────────────────
    // `demosaic` is one function; the algorithm is an argument. Depth is
    // preserved by the type: `BayerRggb8::RgbOutput` is `Rgb8`, and asking
    // for anything else does not compile.
    let bilinear: Image<Rgb8> = demosaic(&raw, BayerBilinear);
    let malvar: Image<Rgb8> = demosaic(&raw, MalvarHeCutler);

    // ── 4. Score both against the truth ──────────────────────────────────────
    // The comparison is in sRGB code values, which is what a viewer sees, not
    // in linear light. That is the honest frame for an artifact you are
    // looking at rather than integrating.
    println!("\nmean absolute error against the original, in code values 0-255:");
    println!("  {:<20} {:>7} {:>7} {:>7} {:>9}", "", "red", "green", "blue", "all");
    for (name, image) in [("bilinear", &bilinear), ("Malvar-He-Cutler", &malvar)] {
        let (r, g, b) = channel_mae(&truth, image);
        println!(
            "  {name:<20} {r:>7.4} {g:>7.4} {b:>7.4} {:>9.4}",
            (r + g + b) / 3.0
        );
    }
    println!(
        "  Green wins in both, and that is structural rather than lucky: the\n\
         RGGB tile samples green twice per 2x2 and the other two once, so green\n\
         is interpolated from twice the evidence."
    );

    // ── 5. The grey step, where the ranking stops being one-dimensional ──────
    report_grey_step();

    // ── 6. White balance, on the mosaic ──────────────────────────────────────
    // Gains apply per CFA colour, so they belong on the raw data, before any
    // interpolation has mixed neighbours together. Doing it after demosaicing
    // is not the same operation: it scales values that are already averages
    // of differently-lit samples.
    let gains = BayerGains::try_new(1.45, 1.0, 1.85).expect("finite, non-negative gains");
    let balanced_raw = white_balance(&raw, gains);
    let balanced: Image<Rgb8> = demosaic(&balanced_raw, MalvarHeCutler);
    let (br, bg, bb) = channel_mean(&balanced);
    let (mr, mg, mb) = channel_mean(&malvar);
    println!("\nwhite balance with gains r=1.45 g=1.00 b=1.85, applied to the mosaic:");
    println!("  channel means before: r {mr:.1}, g {mg:.1}, b {mb:.1}");
    println!("  channel means after:  r {br:.1}, g {bg:.1}, b {bb:.1}");
    println!(
        "  The gains are an invariant-carrying type: a negative gain would invert\n\
         a channel and a NaN would erase it, so `try_new` rejects both where the\n\
         number enters the API rather than where the pixel goes wrong."
    );

    // ── 7. The pinned border ─────────────────────────────────────────────────
    println!(
        "\n`demosaic` takes no border policy, deliberately. Reflect-101 is the only\n\
         policy in the crate that preserves CFA parity: mirroring about the edge\n\
         pixel keeps a red site red. `Clamp`, the one a caller would most likely\n\
         reach for, repeats the edge pixel and so reads red where the kernel\n\
         expects green — a wrong colour, not a slightly wrong value. The choice\n\
         is not the caller's to get wrong, so it is not offered."
    );

    // ── Display ──────────────────────────────────────────────────────────────
    let raw_visual = raw_as_grey(&raw);
    let diff = amplified_difference(&bilinear, &malvar, 8);
    // `Identity` displays an `Srgb8`, and demosaicing produced `Rgb8`: the
    // output type is `BayerRggb8::RgbOutput`, which is a *linear* 8-bit RGB.
    // The values in it are sRGB code values, because that is what the mosaic
    // was built from, so the right move for display is to re-label them, not
    // to run a gamma conversion that would encode them a second time. The
    // type system is correct to make this explicit rather than silent.
    let bilinear_srgb = relabel_as_srgb(&bilinear);
    let malvar_srgb = relabel_as_srgb(&malvar);
    println!("\nOpening 5 windows — press any key to close all");
    DebugDisplay::run(move |ctx| {
        ctx.show("1 — original (ground truth)", &truth, Identity);
        ctx.show("2 — the mosaic, shown as grey", &raw_visual, Identity);
        ctx.show("3 — demosaic(BayerBilinear)", &bilinear_srgb, Identity);
        ctx.show("4 — demosaic(MalvarHeCutler)", &malvar_srgb, Identity);
        ctx.show("5 — |bilinear − Malvar| × 8", &diff, Identity);
        match ctx.wait_key() {
            Some(key) => println!("Key pressed: {key:?}"),
            None => println!("All windows closed"),
        }
    });
}

/// Keep one channel per pixel, chosen by the pattern's 2×2 tile.
///
/// This is the inverse of demosaicing and the only way to get ground truth:
/// a real raw file has no reference image to score against.
fn mosaic(image: &Image<Srgb8>, pattern: BayerPattern) -> Image<BayerRggb8> {
    Image::generate(image.width(), image.height(), |x, y| {
        let p = image.pixel_at(x, y);
        let value = match pattern.color_at(x, y) {
            CfaColor::Red => p.r.0,
            CfaColor::Green => p.g.0,
            CfaColor::Blue => p.b.0,
        };
        BayerRggb8::new(value)
    })
}

/// Mean absolute error per channel between the truth and a reconstruction.
///
/// Border pixels are included. Excluding them would flatter both algorithms
/// and flatter the 5×5 one more, since it has a wider border to get wrong.
fn channel_mae(truth: &Image<Srgb8>, recon: &Image<Rgb8>) -> (f64, f64, f64) {
    let (mut sr, mut sg, mut sb) = (0.0_f64, 0.0_f64, 0.0_f64);
    for y in 0..truth.height() {
        for x in 0..truth.width() {
            let t = truth.pixel_at(x, y);
            let r = recon.pixel_at(x, y);
            sr += (i32::from(t.r.0) - i32::from(r.r.0)).abs() as f64;
            sg += (i32::from(t.g.0) - i32::from(r.g.0)).abs() as f64;
            sb += (i32::from(t.b.0) - i32::from(r.b.0)).abs() as f64;
        }
    }
    let n = (truth.width() * truth.height()) as f64;
    (sr / n, sg / n, sb / n)
}

/// Per-channel mean of a reconstruction, for showing what gains did.
fn channel_mean(image: &Image<Rgb8>) -> (f64, f64, f64) {
    let (mut sr, mut sg, mut sb) = (0.0_f64, 0.0_f64, 0.0_f64);
    for y in 0..image.height() {
        for x in 0..image.width() {
            let p = image.pixel_at(x, y);
            sr += f64::from(p.r.0);
            sg += f64::from(p.g.0);
            sb += f64::from(p.b.0);
        }
    }
    let n = (image.width() * image.height()) as f64;
    (sr / n, sg / n, sb / n)
}

/// The measurement that refutes a one-line quality claim.
///
/// A vertical grey step is the hardest case for a demosaic: there is no
/// colour anywhere in the scene, so every non-zero chroma in the output is
/// pure artifact and needs no threshold to identify. Two numbers describe
/// it and they do not move together. The **peak** fringe is how wrong the
/// worst pixel gets; the **width** is how many columns are wrong at all.
/// Malvar–He–Cutler improves the first and worsens the second, because a
/// 5×5 window reaches further from the edge than a 3×3 one. Mean error
/// still falls, so the trade is worth making — but a caller who cares about
/// a thin bright line rather than an average may disagree, and this is the
/// evidence they need to decide.
fn report_grey_step() {
    let step: Image<Srgb8> = Image::generate(64, 32, |x, _| {
        let v = if x < 32 { 40 } else { 210 };
        Srgb8::new(v, v, v)
    });
    let raw = mosaic(&step, BayerRggb8::PATTERN);
    let bilinear: Image<Rgb8> = demosaic(&raw, BayerBilinear);
    let malvar: Image<Rgb8> = demosaic(&raw, MalvarHeCutler);

    println!("\ngrey step (no colour in the scene, so all chroma is artifact):");
    for (name, image) in [("bilinear", &bilinear), ("Malvar-He-Cutler", &malvar)] {
        let (peak, width) = fringe(image);
        println!("  {name:<20} peak fringe {peak:>3} levels, fringed band {width} columns wide");
    }
    println!(
        "  The two numbers move in opposite directions. Ranking the algorithms\n\
         needs the caller to say which of the two costs them more, which is why\n\
         both ship and neither is the default hidden behind the other."
    );
}

/// Peak chroma excursion and how many columns carry any at all.
fn fringe(image: &Image<Rgb8>) -> (i32, usize) {
    let mut peak = 0;
    let mut columns = 0;
    for x in 0..image.width() {
        let mut column_peak = 0;
        for y in 0..image.height() {
            let p = image.pixel_at(x, y);
            let (r, g, b) = (i32::from(p.r.0), i32::from(p.g.0), i32::from(p.b.0));
            let spread = r.max(g).max(b) - r.min(g).min(b);
            column_peak = column_peak.max(spread);
        }
        peak = peak.max(column_peak);
        if column_peak > 2 {
            columns += 1;
        }
    }
    (peak, columns)
}

/// The mosaic drawn as a grey image, which is what it honestly is: a plane
/// of scalars whose meaning depends on position.
fn raw_as_grey(raw: &Image<BayerRggb8>) -> Image<Srgb8> {
    Image::generate(raw.width(), raw.height(), |x, y| {
        let v = raw.pixel_at(x, y).value();
        Srgb8::new(v, v, v)
    })
}

/// Where the two reconstructions disagree, amplified so it is visible.
fn amplified_difference(a: &Image<Rgb8>, b: &Image<Rgb8>, gain: i32) -> Image<Srgb8> {
    Image::generate(a.width(), a.height(), |x, y| {
        let (pa, pb) = (a.pixel_at(x, y), b.pixel_at(x, y));
        let d = |u: u8, v: u8| {
            ((i32::from(u) - i32::from(v)).abs() * gain).min(255) as u8
        };
        Srgb8::new(d(pa.r.0, pb.r.0), d(pa.g.0, pb.g.0), d(pa.b.0, pb.b.0))
    })
}

/// Re-label linear `Rgb8` channel values as `Srgb8` ones, without touching
/// them.
///
/// Not a colour conversion and not pretending to be: this demo fed sRGB code
/// values into the mosaic, so the numbers coming out are already
/// sRGB-encoded and applying a transfer function would encode them twice.
/// The crate makes the step visible because in the general case, with a real
/// sensor, the raw values are linear and this function would be wrong.
fn relabel_as_srgb(image: &Image<Rgb8>) -> Image<Srgb8> {
    Image::generate(image.width(), image.height(), |x, y| {
        let p = image.pixel_at(x, y);
        Srgb8::new(p.r.0, p.g.0, p.b.0)
    })
}
