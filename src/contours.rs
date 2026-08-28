//! # contours — border tracing, hierarchy, and the shape descriptors it unlocks
//!
//! Demonstrates `fovea::analyze::contours` on a synthetic scene whose answers
//! are known exactly, then on a real image where they are not:
//!
//! 1. Build a binary scene with four shapes chosen so that every descriptor
//!    has a checkable value: a filled disc, an annulus, a square with two
//!    square holes, and a concave C.
//! 2. `extract_contours` → one `ComponentContour` per component, each with an
//!    outer border and zero or more hole borders. The hierarchy is *derived
//!    from the labeling*, not from a second tracing pass.
//! 3. Read the descriptors: area, perimeter, circularity, solidity and the
//!    Euler number. The hole count and the Euler number are the two that
//!    prove the hierarchy is real rather than a flat list of chains.
//! 4. Show the **staircase bias** the traced chain carries, and what
//!    `approximate_polygon` (Douglas–Peucker) does about it. The disc is the
//!    honest case: a perfect circle has circularity 1.0 and the raw chain
//!    does not reach it.
//! 5. Convex hull and solidity, which is where the concave C separates from
//!    the disc.
//! 6. The chain code, which is the same border in a different alphabet.
//! 7. The same pipeline on the Terrace photograph, thresholded — no known
//!    answer, just evidence that step 2 scales.
//!
//! Everything is drawn with `fovea::draw`, so the overlays are an API
//! demonstration rather than example scaffolding.
//!
//! ```text
//! cargo run --bin contours
//! ```
//!
//! Press any key or close any window to exit.

use std::fs;

use fovea::analyze::components::Connectivity8;
use fovea::analyze::contours::{
    ComponentContour, Contour, approximate_polygon, extract_contours, polygon_area,
    polygon_perimeter,
};
use fovea::draw::{Drawable, Polyline};
use fovea::image::{Image, ImageView};
use fovea::pixel::{Label32, Srgb8};
use fovea::{Coordinate, Tolerance, tolerance};
use fovea_display::{DebugDisplay, Identity};
use fovea_io::jpeg::{self, JpegImage};

// Outline colours: outer borders, hole borders, the convex hull, and the
// Douglas-Peucker simplification. Functions rather than consts because
// `Srgb8::new` is not a `const fn`.
fn outer_color() -> Srgb8 {
    Srgb8::new(255, 80, 80)
}
fn hole_color() -> Srgb8 {
    Srgb8::new(80, 200, 255)
}
fn hull_color() -> Srgb8 {
    Srgb8::new(255, 220, 60)
}
fn simplified_color() -> Srgb8 {
    Srgb8::new(120, 255, 120)
}

fn main() {
    // ── 1. A scene with known answers ────────────────────────────────────────
    // Each shape isolates one descriptor. The disc pins circularity, the
    // annulus pins a single hole, the square pins two holes and an Euler
    // number of −1, and the C pins solidity well below 1.
    let scene = synthetic_scene();
    println!(
        "synthetic scene {}×{} (bool)",
        scene.width(),
        scene.height()
    );

    // ── 2. Extract ───────────────────────────────────────────────────────────
    // One call returns both the labeling and the hierarchy, because the
    // hierarchy *is* derived from the labeling: a hole belongs to the
    // component that encloses it, and enclosure is a labeling question.
    // 8-connectivity for the foreground implies 4-connectivity for the
    // background, which is what makes the two consistent.
    let (labeling, hierarchy) = extract_contours::<Label32, Connectivity8>(&scene)
        .expect("the scene is a valid bool image");
    println!(
        "components: {} (labeling reports {})",
        hierarchy.components().len(),
        labeling.label_count,
    );

    // ── 3. Descriptors ───────────────────────────────────────────────────────
    println!(
        "\n{:<10} {:>8} {:>10} {:>7} {:>7} {:>6} {:>6}",
        "shape", "area", "perimeter", "circ", "solid", "holes", "euler"
    );
    for (name, component) in NAMES.iter().zip(hierarchy.components()) {
        let outer = component.outer();
        println!(
            "{name:<10} {:>8.0} {:>10.2} {:>7} {:>7} {:>6} {:>6}",
            outer.area(),
            outer.perimeter(),
            fmt_opt(outer.circularity()),
            fmt_opt(outer.solidity()),
            component.holes().len(),
            component.euler_number(),
        );
    }
    println!(
        "\nThe disc and the annulus report the *same* area, perimeter, circularity\n\
         and solidity, and that is correct rather than a bug: an outer border is a\n\
         border, and these two shapes have the same one. Only the hole count and\n\
         the Euler number tell them apart, which is the whole argument for\n\
         deriving a hierarchy instead of returning a flat list of chains.\n\
         Euler number is (components − holes) per component, so the square with\n\
         two holes reports −1. It is the descriptor that cannot be computed from\n\
         an outer border alone, which is why the hierarchy has to exist."
    );

    // ── 4. The staircase bias, and Douglas–Peucker ───────────────────────────
    report_staircase_bias(&hierarchy.components()[0]);

    // ── 5. Solidity: where a concave shape separates from a convex one ───────
    report_solidity(&hierarchy.components()[0], &hierarchy.components()[3]);

    // ── 6. The chain code ────────────────────────────────────────────────────
    let disc_chain = hierarchy.components()[0].outer().chain_code();
    let first: Vec<String> = disc_chain
        .moves()
        .iter()
        .take(12)
        .map(|m| format!("{m:?}"))
        .collect();
    println!(
        "\nchain code of the disc border: {} moves, first 12 = {}",
        disc_chain.moves().len(),
        first.join(", "),
    );
    println!(
        "  round-trip to_points() reproduces the border: {}",
        disc_chain.to_points() == hierarchy.components()[0].outer().points(),
    );

    // ── 7. The same pipeline on a photograph ─────────────────────────────────
    let (photo_rgb, photo_count) = terrace_contours();

    // ── Display ──────────────────────────────────────────────────────────────
    let scene_rgb = render_scene(&scene, &hierarchy);
    println!("\nOpening 2 windows — press any key to close all");
    DebugDisplay::run(move |ctx| {
        ctx.show(
            "1 — synthetic scene: outer (red), holes (blue), hull (yellow), Douglas-Peucker (green)",
            &scene_rgb,
            Identity,
        );
        ctx.show(
            &format!("2 — Terrace thresholded: {photo_count} components"),
            &photo_rgb,
            Identity,
        );
        match ctx.wait_key() {
            Some(key) => println!("Key pressed: {key:?}"),
            None => println!("All windows closed"),
        }
    });
}

/// Names in the raster order `extract_contours` reports components in.
const NAMES: [&str; 4] = ["disc", "annulus", "square", "concave C"];

/// Four shapes, laid out left to right so raster order matches [`NAMES`].
///
/// The disc is centred at (40, 60) with radius 30, which is large enough
/// that the staircase bias in step 4 is a property of the tracing rather
/// than of the rasterisation being coarse.
fn synthetic_scene() -> Image<bool> {
    Image::generate(320, 120, |x, y| {
        let (x, y) = (x as i64, y as i64);

        // Disc: filled, convex, the circularity reference.
        let disc = sq(x - 40) + sq(y - 60) <= 30 * 30;

        // Annulus: one hole, so Euler number 0.
        let r = sq(x - 120) + sq(y - 60);
        let annulus = r <= 30 * 30 && r > 15 * 15;

        // Square with two square holes: Euler number −1.
        let square = (180..=240).contains(&x) && (30..=90).contains(&y);
        let hole_a = (192..=208).contains(&x) && (42..=58).contains(&y);
        let hole_b = (212..=228).contains(&x) && (62..=78).contains(&y);
        let square = square && !hole_a && !hole_b;

        // Concave C: a disc with a bite taken out of the right side, which
        // drops solidity without adding a hole.
        let c_body = sq(x - 285) + sq(y - 60) <= 30 * 30;
        let c_bite = x > 285 && (45..=75).contains(&y);
        let concave = c_body && !c_bite;

        disc || annulus || square || concave
    })
}

fn sq(v: i64) -> i64 {
    v * v
}

/// How far the traced chain's circularity sits from the analytic 1.0, and
/// how much of the gap Douglas–Peucker closes.
///
/// The traced border is a chain of *pixel centres*, so it follows the
/// rasterisation staircase: every diagonal step is counted at its true
/// length but the boundary itself is longer than the underlying circle.
/// Perimeter is therefore overestimated while area is not, and circularity
/// (`4π·area / perimeter²`) comes out low. Simplifying the chain removes
/// most of the staircase and most of the gap — but not all of it, and the
/// remainder is why the crate documents the measured number instead of
/// claiming an unbiased perimeter.
fn report_staircase_bias(disc: &ComponentContour) {
    let outer = disc.outer();
    let raw = outer
        .circularity()
        .expect("a disc border has more than two points");

    println!("\nstaircase bias on the disc (a perfect circle would be 1.0):");
    println!("  raw traced chain:      circularity {raw:.3}");

    for t in [tolerance!(0.5), tolerance!(1.0), tolerance!(2.0)] {
        let simplified = approximate_polygon(outer.points(), t);
        let area = polygon_area(&simplified);
        let perimeter = polygon_perimeter(&simplified);
        let circularity = if perimeter > 0.0 {
            4.0 * std::f64::consts::PI * area / (perimeter * perimeter)
        } else {
            f64::NAN
        };
        println!(
            "  Douglas-Peucker ε={:.1}: circularity {:.3} ({} vertices, was {})",
            t.get(),
            circularity,
            simplified.len(),
            outer.points().len(),
        );
    }
    println!(
        "  Note the gap does not close monotonically: a coarser tolerance can\n\
         overshoot 1.0 and come back down, because Douglas-Peucker shortens the\n\
         perimeter faster than it shrinks the area. Circularity is a ratio, so a\n\
         simplification tuned to maximise it is fitting the metric, not the shape.\n\
         The approximation is a caller-named step, not something tracing does\n\
         for you: it trades vertices for a shorter, straighter boundary, and the\n\
         tolerance is the caller's call about how much shape to give up."
    );
}

/// Solidity separates a convex shape from a concave one; circularity does
/// not do it reliably, because a bite that shortens the boundary can leave
/// circularity almost unchanged.
fn report_solidity(disc: &ComponentContour, concave: &ComponentContour) {
    println!("\nsolidity = area / convex-hull area:");
    for (name, component) in [("disc", disc), ("concave C", concave)] {
        let outer = component.outer();
        let hull = outer.convex_hull();
        println!(
            "  {name:<10} solidity {}, hull has {} vertices for a border of {}",
            fmt_opt(outer.solidity()),
            hull.len(),
            outer.points().len(),
        );
    }
}

/// Threshold the Terrace photograph and count what comes out.
///
/// No known answer here, and no claim that the components are meaningful —
/// a fixed threshold on a natural image is not segmentation. The point is
/// that the same two calls run on 1000×1000-ish real data.
fn terrace_contours() -> (Image<Srgb8>, usize) {
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

    let binary: Image<bool> = Image::generate(mono.width(), mono.height(), |x, y| {
        mono.pixel_at(x, y).0.0 > 128
    });
    let (_, hierarchy) = extract_contours::<Label32, Connectivity8>(&binary)
        .expect("the thresholded image is a valid bool image");

    // Keep only components big enough to see, so the overlay stays readable.
    let mut shown = 0;
    let mut out: Image<Srgb8> = Image::generate(mono.width(), mono.height(), |x, y| {
        let v = mono.pixel_at(x, y).0.0 / 3;
        Srgb8::new(v, v, v)
    });
    for component in hierarchy.components() {
        if component.outer().area() < 400.0 {
            continue;
        }
        shown += 1;
        draw_contour(&mut out, component.outer(), outer_color());
        for hole in component.holes() {
            draw_contour(&mut out, hole, hole_color());
        }
    }
    println!(
        "\nTerrace {}×{} thresholded at 128: {} components, {shown} with area ≥ 400",
        mono.width(),
        mono.height(),
        hierarchy.components().len(),
    );
    (out, shown)
}

/// Paint the synthetic scene with every layer the demo talks about.
fn render_scene(
    scene: &Image<bool>,
    hierarchy: &fovea::analyze::contours::ContourHierarchy,
) -> Image<Srgb8> {
    let mut out: Image<Srgb8> = Image::generate(scene.width(), scene.height(), |x, y| {
        if scene.pixel_at(x, y) {
            Srgb8::new(50, 50, 50)
        } else {
            Srgb8::new(12, 12, 12)
        }
    });

    for component in hierarchy.components() {
        let outer = component.outer();

        // Convex hull first, so the tighter outlines draw on top of it.
        Polyline {
            points: to_points(&outer.convex_hull()),
            color: hull_color(),
            closed: true,
        }
        .draw_into(&mut out);

        // The Douglas–Peucker simplification of the same border.
        Polyline {
            points: to_points(&approximate_polygon(outer.points(), tolerance!(2.0))),
            color: simplified_color(),
            closed: true,
        }
        .draw_into(&mut out);

        draw_contour(&mut out, outer, outer_color());
        for hole in component.holes() {
            draw_contour(&mut out, hole, hole_color());
        }
    }
    out
}

/// Draw one traced border as a closed polyline.
fn draw_contour(image: &mut Image<Srgb8>, contour: &Contour, color: Srgb8) {
    Polyline {
        points: to_points(contour.points()),
        color,
        closed: true,
    }
    .draw_into(image);
}

/// `Coordinate` is an unsigned grid position; `Polyline` takes signed
/// vertices because a shape may legitimately extend off-image.
fn to_points(points: &[Coordinate]) -> Vec<(i32, i32)> {
    points.iter().map(|p| (p.x as i32, p.y as i32)).collect()
}

/// Descriptors that would divide by zero return `None` rather than `NaN`;
/// print that distinction instead of hiding it behind a default.
fn fmt_opt(value: Option<f64>) -> String {
    match value {
        Some(v) => format!("{v:.3}"),
        None => "n/a".to_string(),
    }
}

/// Silences an unused-import warning when the `Tolerance` type is only
/// reached through the `tolerance!` macro.
#[allow(dead_code)]
fn _tolerance_type_is_named(_: Tolerance) {}
