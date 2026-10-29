//! # lens_measure: measure a corner through a lens that bends straight edges
//!
//! Demonstrates measuring on the image of a distorting lens with
//! `fovea::geometry::BrownConrady`, and the pixel spaces that keep the camera
//! image and the ideal, undistorted image apart. The scene is synthetic, so
//! every answer is known:
//!
//! 1. A light part on a dark ground: two straight edges meeting at 70° in a
//!    corner, and a chip in the top edge. The part is built in millimetres
//!    and seen through a lens with barrel distortion, handed over as a
//!    calibration tool writes it (camera matrix and OpenCV coefficients).
//!    Each camera pixel is the light fraction of its area, traced back
//!    through the lens, so the straight edges come out curved.
//! 2. Calipers along both edges of the camera image and a line fitted to
//!    each: the edges bow away from their lines by pixels, and the angle
//!    between the lines is off.
//! 3. The same edge points corrected for the lens with `undistort_point`;
//!    and for comparison, the image remapped into the ideal image and
//!    measured again. Both give the same straight edges.
//! 4. Lines fitted to the corrected points, with Tukey's biweight leaving
//!    out the point on the chip; their angle, axis and intersection.
//! 5. The fits converted to millimetres through the calibration of the ideal
//!    image, and compared with the part as built.
//! 6. The inspection record: the camera image beside the ideal one, with the
//!    edge points, the fitted lines, the corner and the numbers.
//!
//! ```text
//! cargo run --bin lens_measure
//! cargo run --bin lens_measure -- --save record.png
//! ```
//!
//! Without `--save`, press any key or close the window to exit. With it, the
//! record is written as a PNG.

use std::path::PathBuf;

use clap::Parser;

use fovea::border::{Clamp, Skip};
use fovea::draw::{FONT_6X13, draw_crosshair, draw_line, draw_text};
use fovea::geometry::{
    AffineMap, BrownConrady, CameraMatrix, DestToSource, FocalLength, LengthUnit, PlaneMap,
    Similarity, Vector,
};
use fovea::image::{Image, ImageView};
use fovea::measure::{AllPoints, Caliper, Polarity, TotalLeastSquares, fit, profile};
use fovea::pixel::{MonoF32, Srgb8};
use fovea::transform::{CatmullRom, remap};
use fovea::{Angle, IdealPixels, Length, Line, Millimeter, Point, min_contrast, sigma, tukey};
use fovea_display::{DebugDisplay, Identity};
use fovea_io::png::{self, PngEncodeOptions};

#[derive(Parser)]
#[command(about = "Measure a corner through a lens that bends straight edges")]
struct Args {
    /// Write the record to this PNG file instead of opening a window
    #[arg(long)]
    save: Option<PathBuf>,
}

/// The part as built, in millimetres and degrees: its corner, the direction
/// of its top edge, and the angle from the top edge to the side edge.
const CORNER: (f64, f64) = (10.6, 7.3);
const TOP: f64 = 2.0;
const ANGLE: f64 = 70.0;
/// A chip in the top edge, 11.6 mm from the corner: a disc of 0.1 mm radius
/// cut out of the part.
const CHIP: (f64, f64) = (11.6, 0.1);
/// The camera image, in pixels.
const SIZE: (usize, usize) = (640, 480);

fn main() -> Result<(), fovea::Error> {
    let args = Args::parse();

    // ── 1. The lens, the calibration and the camera image ───────────────────
    // As a calibration hands them over: the camera matrix, and the
    // coefficients in OpenCV's order (k1, k2, p1, p2, k3). A negative k1 is a
    // barrel: the further from the centre, the more the lens pulls inwards.
    let camera =
        CameraMatrix::try_new(FocalLength { x: 600.0, y: 600.0 }, Point::new(318.6, 243.2))?;
    let lens = BrownConrady::from_opencv(camera, &[-0.28, 0.09, 0.0004, -0.0003, 0.0])?;
    // The ideal image to the part: 50 µm per pixel, the camera turned 1.5°
    // against the machine axes. It starts in `IdealPixels`, the space it was
    // calibrated in, so it takes no point of the camera image.
    let cal: Similarity<IdealPixels, Millimeter> =
        Similarity::try_new(0.05, Angle::from_degrees(1.5)?, Vector::new(5.0, 3.0))?;
    let part = Part::new()?;
    let seen = scene(&lens, &cal, &part);

    // The inspection plan, drawn on the part and carried into the ideal
    // image: a caliper every 24 px along each edge, from 40 px past the
    // corner, each crossing the edge into the part. The ninth on the top
    // edge falls on the chip.
    let to_ideal = cal.inverse();
    let corner = to_ideal.map_point(part.corner);
    let plan = |edge: Vector<Millimeter>, count: usize, turn: f64| {
        let d = to_ideal
            .map_vector(edge)
            .normalized()
            .expect("an edge of the part has a direction");
        let into = Vector::new(-turn * d.y, turn * d.x);
        let at = (0..count).map(|k| corner + d * (40.0 + 24.0 * k as f64));
        (at.collect::<Vec<_>>(), into)
    };
    let (plan_a, plan_b) = (plan(part.top, 20, 1.0), plan(part.side, 15, -1.0));

    // ── 2. Measured on the camera image ─────────────────────────────────────
    // The lens puts each planned caliper where the edge appears in the
    // camera image: `Point<IdealPixels>` in, `Point<Pixels>` out.
    let through_lens = |p| {
        lens.try_map_point(p)
            .unwrap_or_else(|| fail("a planned point maps nowhere"))
    };
    let raw_a = edge_points(&seen, &plan_a, through_lens)?;
    let raw_b = edge_points(&seen, &plan_b, through_lens)?;
    let raw_fit_a = fit(&raw_a, TotalLeastSquares, AllPoints)?;
    let raw_fit_b = fit(&raw_b, TotalLeastSquares, AllPoints)?;
    println!(
        "camera image:     edges {:.3} and {:.3} px RMS from their lines, angle {:.3}°",
        raw_fit_a.rms_residual().get(),
        raw_fit_b.rms_residual().get(),
        raw_fit_a.element().angle_to(raw_fit_b.element()).degrees(),
    );

    // ── 3. Corrected for the lens ────────────────────────────────────────────
    // `Point<Pixels>` in, `Point<IdealPixels>` out. From here on the types
    // say which image a point belongs to: `cal.map_point(raw_a[0])`,
    // `raw_fit_a.map(&cal)` and correcting a point twice do not compile.
    let correct = |points: &[Point<_>]| -> Result<Vec<_>, fovea::Error> {
        points.iter().map(|&p| lens.undistort_point(p)).collect()
    };
    let (ideal_a, ideal_b) = (correct(&raw_a)?, correct(&raw_b)?);

    // For comparison, the other way: remap the image into the ideal image
    // and measure there, along the plan itself. The lens maps an ideal pixel
    // to the camera pixel it reads, the direction a remap goes, and the
    // result is an `Image<MonoF32, IdealPixels>`. Correcting the points is
    // the better way, since it adds no interpolation.
    let ideal_image: Image<MonoF32, IdealPixels> =
        remap(&seen, &DestToSource(lens), CatmullRom, &Clamp, seen.size())?;
    let remapped_a = edge_points(&ideal_image, &plan_a, |p| p)?;
    let remapped_b = edge_points(&ideal_image, &plan_b, |p| p)?;

    // ── 4. The corner in the ideal image ─────────────────────────────────────
    // Tukey's threshold sits well above the noise of the edge points and
    // below the depth of the chip, so the point on the chip is left out.
    // Bound once: the record below reuses it to mark the rejected point.
    let rule = tukey!(0.5);
    let fit_a = fit(&ideal_a, TotalLeastSquares, rule)?;
    let fit_b = fit(&ideal_b, TotalLeastSquares, rule)?;
    let (line_a, line_b) = (fit_a.element(), fit_b.element());
    let corner_ideal = meet(line_a, line_b);
    println!(
        "corrected points: edges {:.3} and {:.3} px RMS from their lines, angle {:.3}°, \
         {} point left out",
        fit_a.rms_residual().get(),
        fit_b.rms_residual().get(),
        line_a.angle_to(line_b).degrees(),
        fit_a.outlier_count() + fit_b.outlier_count(),
    );
    let check_a = fit(&remapped_a, TotalLeastSquares, rule)?;
    let check_b = fit(&remapped_b, TotalLeastSquares, rule)?;
    println!(
        "remapped image:   angle {:.3}°, corner {:.4} px from that of the corrected points",
        check_a.element().angle_to(check_b.element()).degrees(),
        meet(check_a.element(), check_b.element())
            .distance(corner_ideal)
            .get(),
    );

    // ── 5. In millimetres ────────────────────────────────────────────────────
    // The calibration is a similarity, so a fitted line converts as a whole,
    // residuals included, and the angle between two lines is kept.
    let (mm_a, mm_b) = (fit_a.map(&cal), fit_b.map(&cal));
    let angle = mm_a.element().angle_to(mm_b.element());
    let corner_mm = meet(mm_a.element(), mm_b.element());
    println!(
        "\nangle:    {:.3}° (built {ANGLE:.1}°)\n\
         top edge: {:.3}° to the machine x axis (built {TOP:.1}°)\n\
         corner:   ({:.4}, {:.4}) mm (built ({:.1}, {:.1}) mm)\n\
         chip:     {:.4} mm deep (built {:.1} mm)",
        angle.degrees(),
        mm_a.element().axis().degrees(),
        corner_mm.x,
        corner_mm.y,
        CORNER.0,
        CORNER.1,
        mm_a.max_residual().get(),
        CHIP.1,
    );

    // ── 6. The record ────────────────────────────────────────────────────────
    // The two images side by side, the ideal one 640 px to the right. The
    // record is a picture, drawn on and never measured.
    let (w, h) = SIZE;
    let mut record: Image<Srgb8> = Image::generate(2 * w, h, |x, y| {
        let v = if x < w {
            seen.pixel_at(x, y).0
        } else {
            ideal_image.pixel_at(x - w, y).0
        };
        let v = v.round().clamp(0.0, 255.0) as u8;
        Srgb8::new(v, v, v)
    });
    let (red, cyan, green, yellow) = (
        Srgb8::new(255, 90, 90),
        Srgb8::new(80, 220, 255),
        Srgb8::new(120, 255, 120),
        Srgb8::new(255, 220, 60),
    );
    let (white, shade) = (Srgb8::new(255, 255, 255), Srgb8::new(30, 30, 60));
    let (cx, cy) = rounded(corner_ideal, w);

    // Left: the edge points of the camera image, and the straight lines the
    // curved edges do not follow.
    for &p in raw_a.iter().chain(&raw_b) {
        draw_crosshair(&mut record, rounded(p, 0), 2, cyan);
    }
    for f in [&raw_fit_a, &raw_fit_b] {
        let s = f.extent();
        draw_line(&mut record, rounded(s.start, 0), rounded(s.end, 0), red);
    }
    // Right: the corrected points, the one on the chip in red, and the
    // fitted lines up to the corner.
    for (points, f) in [(&ideal_a, &fit_a), (&ideal_b, &fit_b)] {
        for &p in points {
            let off = f.element().distance(p) >= rule.threshold();
            draw_crosshair(&mut record, rounded(p, w), 2, if off { red } else { cyan });
        }
        draw_line(&mut record, (cx, cy), rounded(f.extent().end, w), green);
    }
    draw_crosshair(&mut record, (cx, cy), 6, yellow);

    // The numbers, each beside what it describes.
    let mut label = |text: &str, at: (isize, isize)| {
        draw_text(&mut record, text, at, white, Some(shade), 1, &FONT_6X13);
    };
    let fits_off = format!(
        "camera image\nline fits off by up to\n{:.2} px (top), {:.2} px (side)",
        raw_fit_a.max_residual().get(),
        raw_fit_b.max_residual().get(),
    );
    label(&fits_off, (330, 300));
    label("ideal image", ((w + 330) as isize, 300));
    let at_corner = format!(
        "{:.2}°\n({:.3}, {:.3}) mm",
        angle.degrees(),
        corner_mm.x,
        corner_mm.y
    );
    label(&at_corner, (cx + 40, cy + 30));
    let chip = rounded(ideal_a[8], w);
    let depth = format!("chip {:.3} mm", mm_a.max_residual().get());
    label(&depth, (chip.0 - 40, chip.1 - 30));

    // ── Archive or display ───────────────────────────────────────────────────
    if let Some(path) = args.save {
        let encoded = png::encode(&record, &PngEncodeOptions::default())
            .unwrap_or_else(|e| fail(&format!("PNG encode failed: {e}")));
        std::fs::write(&path, &encoded)
            .unwrap_or_else(|e| fail(&format!("cannot write {}: {e}", path.display())));
        println!("\nwrote {}", path.display());
        return Ok(());
    }
    println!("\nOpening 1 window, press any key to close");
    DebugDisplay::run(move |ctx| {
        ctx.show(
            "lens_measure: camera image (left), ideal image (right)",
            &record,
            Identity,
        );
        match ctx.wait_key() {
            Some(key) => println!("Key pressed: {key:?}"),
            None => println!("Window closed"),
        }
    });
    Ok(())
}

/// The part in millimetres: its corner, the unit directions of its edges,
/// and the centre of the chip.
struct Part {
    corner: Point<Millimeter>,
    top: Vector<Millimeter>,
    side: Vector<Millimeter>,
    chip: Point<Millimeter>,
}

impl Part {
    fn new() -> Result<Self, fovea::Error> {
        let direction = |degrees: f64| -> Result<Vector<Millimeter>, fovea::Error> {
            Ok(Vector::from_polar(
                Length::new(1.0),
                Angle::from_degrees(degrees)?,
            ))
        };
        let (corner, top) = (Point::new(CORNER.0, CORNER.1), direction(TOP)?);
        Ok(Self {
            corner,
            top,
            side: direction(TOP + ANGLE)?,
            chip: corner + top * CHIP.0,
        })
    }

    /// How deep `p` lies inside the part, negative outside: every point
    /// nearer to `p` than that lies on the same side of the outline.
    fn depth(&self, p: Point<Millimeter>) -> f64 {
        let v = p - self.corner;
        let below_top = self.top.x * v.y - self.top.y * v.x;
        let right_of_side = v.x * self.side.y - v.y * self.side.x;
        let outside_chip = p.distance(self.chip).get() - CHIP.1;
        below_top.min(right_of_side).min(outside_chip)
    }
}

/// The camera image, each pixel the light fraction of its area: 20 dark,
/// 220 light. A camera position sees the point of the part that the lens
/// correction and the calibration trace it back to. A pixel more than
/// 0.15 mm (three pixels) inside or outside takes one value; one nearer to
/// the outline is sampled 8 × 8 times across its area.
fn scene(
    lens: &BrownConrady,
    cal: &Similarity<IdealPixels, Millimeter>,
    part: &Part,
) -> Image<MonoF32> {
    const SUB: usize = 8;
    let depth = |x: f64, y: f64| {
        let ideal = lens
            .undistort_point(Point::new(x, y))
            .unwrap_or_else(|e| fail(&format!("no correction at ({x}, {y}): {e}")));
        part.depth(cal.map_point(ideal))
    };
    Image::generate(SIZE.0, SIZE.1, |x, y| {
        let d = depth(x as f64, y as f64);
        let coverage = if d.abs() > 0.15 {
            if d > 0.0 { 1.0 } else { 0.0 }
        } else {
            let sub = |k: usize, at: usize| at as f64 - 0.5 + (k as f64 + 0.5) / SUB as f64;
            let light = (0..SUB * SUB)
                .filter(|k| depth(sub(k % SUB, x), sub(k / SUB, y)) > 0.0)
                .count();
            light as f32 / (SUB * SUB) as f32
        };
        MonoF32::new(20.0 + 200.0 * coverage)
    })
}

/// The edge of the part on each caliper of `plan`, measured on `image`.
/// A caliper runs 15 px either side of its planned point, into the part,
/// with both ends placed in the image's space by `place`.
fn edge_points<I>(
    image: &I,
    plan: &(Vec<Point<IdealPixels>>, Vector<IdealPixels>),
    place: impl Fn(Point<IdealPixels>) -> Point<I::Space>,
) -> Result<Vec<Point<I::Space>>, fovea::Error>
where
    I: ImageView<Pixel = MonoF32>,
{
    let (at, into) = plan;
    at.iter()
        .map(|&q| {
            let (from, to) = (place(q - *into * 15.0), place(q + *into * 15.0));
            let caliper = Caliper::try_segment(from, to, Length::new(2.0), Length::new(0.25))?;
            let edge = profile(image, &caliper, CatmullRom, &Skip)?
                .edges(Polarity::DarkToLight, sigma!(1.0), min_contrast!(50.0))
                .strongest()
                .unwrap_or_else(|| fail("a caliper missed the edge"));
            Ok(edge.position())
        })
        .collect()
}

/// Where two fitted edges meet.
fn meet<U: LengthUnit>(a: &Line<U>, b: &Line<U>) -> Point<U> {
    a.intersection(b)
        .unwrap_or_else(|| fail("the edges are parallel"))
}

/// The nearest pixel to `p`, `dx` px further right, as a drawing position.
fn rounded<U>(p: Point<U>, dx: usize) -> (isize, isize) {
    (p.x.round() as isize + dx as isize, p.y.round() as isize)
}

fn fail(msg: &str) -> ! {
    eprintln!("lens_measure: {msg}");
    std::process::exit(1);
}
