//! # measure_label — measure a bar and a disc, and write the numbers beside them
//!
//! Demonstrates the measurement tools of `fovea::measure` and the text of
//! `fovea::draw` on a synthetic scene whose answers are known:
//!
//! 1. A light bar 25 px wide and a light disc of 30 px radius on a dark
//!    ground. Their edges are drawn by area coverage, so they sit between
//!    pixels, as edges in a camera image do.
//! 2. A caliper across the bar: two edges, one pair, its width.
//! 3. Sixteen radial calipers across the border of the disc: one edge each,
//!    and a circle fitted to the sixteen points by Taubin's method.
//! 4. The inspection record: the scene in colour, with the caliper, the edge
//!    points and the fitted circle drawn on it, and every number written next
//!    to the geometry it describes, in pixels and in millimetres through a
//!    pixel of 12.5 µm. Each label is measured with `text_size` before it is
//!    placed, so it sits beside its shape rather than over it.
//!
//! ```text
//! cargo run --bin measure_label
//! cargo run --bin measure_label -- --save record.png
//! ```
//!
//! Without `--save`, press any key or close the window to exit. With it, the
//! record is written as a PNG, the form an inspection record is archived in.

use std::f64::consts::TAU;
use std::path::PathBuf;

use clap::Parser;

use fovea::border::Skip;
use fovea::draw::{FONT_6X13, FONT_10X20, draw_circle, draw_crosshair, draw_line, draw_text};
use fovea::geometry::{ConformalMap, UniformScale};
use fovea::image::{Image, ImageView};
use fovea::measure::{AllPoints, Caliper, Neighbors, Polarity, Taubin, profile, try_fit};
use fovea::pixel::{MonoF32, Srgb8};
use fovea::transform::CatmullRom;
use fovea::{Length, Millimeter, Pixels, Point, min_contrast, sigma, uniform_scale};
use fovea_display::{DebugDisplay, Identity};
use fovea_io::png::{self, PngEncodeOptions};

#[derive(Parser)]
#[command(about = "Measure a bar and a disc, and write the numbers beside them")]
struct Args {
    /// Write the record to this PNG file instead of opening a window
    #[arg(long)]
    save: Option<PathBuf>,
}

/// The bar's left and right edges, 25 px apart.
const BAR: (f64, f64) = (40.3, 65.3);
/// The disc's centre and radius.
const DISC: (f64, f64, f64) = (160.4, 70.2, 30.0);

fn main() -> Result<(), fovea::Error> {
    let args = Args::parse();

    // ── 1. The scene ─────────────────────────────────────────────────────────
    let scene = scene(320, 140);
    // 12.5 µm per pixel, the same along both axes.
    let scale: UniformScale<Pixels, Millimeter> = uniform_scale!(0.0125);

    // ── 2. The bar: one caliper, one pair ────────────────────────────────────
    let bar_caliper = Caliper::try_segment(
        Point::new(20.0, 70.0),
        Point::new(90.0, 70.0),
        Length::new(9.0),
        Length::new(0.25),
    )?;
    let bar = profile(&scene, &bar_caliper, CatmullRom, &Skip)?
        .edges(Polarity::Either, sigma!(1.0), min_contrast!(50.0))
        .pairs(Polarity::DarkToLight, Neighbors)[0];
    let width = bar.width();
    println!(
        "bar width: {:.4} px = {:.5} mm (built as {:.1} px)",
        width.get(),
        scale.map_length(width).get(),
        BAR.1 - BAR.0,
    );

    // ── 3. The disc: sixteen radial calipers, one circle ─────────────────────
    // Started from a rough centre, as a blob detector would hand it over;
    // the fit does not need it to be exact.
    let rough = Point::<Pixels>::new(160.0, 70.0);
    let mut points = Vec::new();
    for k in 0..16 {
        let angle = TAU * k as f64 / 16.0;
        let (dx, dy) = (angle.cos(), angle.sin());
        let caliper = Caliper::try_segment(
            Point::new(rough.x + 15.0 * dx, rough.y + 15.0 * dy),
            Point::new(rough.x + 45.0 * dx, rough.y + 45.0 * dy),
            Length::new(3.0),
            Length::new(0.25),
        )?;
        let edge = profile(&scene, &caliper, CatmullRom, &Skip)?
            .edges(Polarity::LightToDark, sigma!(1.0), min_contrast!(50.0))
            .strongest()
            .expect("every radial caliper crosses the border of the disc once");
        points.push(edge.position);
    }
    let fit = try_fit(&points, Taubin, AllPoints)?;
    let circle = fit.element();
    let in_mm = fit.map(&scale);
    println!(
        "disc: centre ({:.4}, {:.4}), diameter {:.4} px = {:.5} mm, RMS residual {:.4} px \
         (built at ({:.1}, {:.1}), diameter {:.1} px)",
        circle.center().x,
        circle.center().y,
        2.0 * circle.radius().get(),
        2.0 * in_mm.element().radius().get(),
        fit.rms_residual().get(),
        DISC.0,
        DISC.1,
        2.0 * DISC.2,
    );

    // ── 4. The record ────────────────────────────────────────────────────────
    let mut record: Image<Srgb8> = Image::generate(scene.width(), scene.height(), |x, y| {
        let v = scene.pixel_at(x, y).0.round().clamp(0.0, 255.0) as u8;
        Srgb8::new(v, v, v)
    });
    let (yellow, cyan, green) = (
        Srgb8::new(255, 220, 60),
        Srgb8::new(80, 220, 255),
        Srgb8::new(120, 255, 120),
    );
    let (white, shade) = (Srgb8::new(255, 255, 255), Srgb8::new(30, 30, 60));

    // The caliper across the bar, and the two edges it found.
    draw_line(&mut record, (20, 70), (90, 70), yellow);
    for edge in [bar.first, bar.second] {
        draw_crosshair(&mut record, rounded(edge.position), 4, cyan);
    }
    // The width, centred above the bar.
    let label = format!(
        "w {:.2} px\n= {:.4} mm",
        width.get(),
        scale.map_length(width).get()
    );
    let size = FONT_6X13.text_size(&label, 1);
    let middle = (BAR.0 + BAR.1) / 2.0;
    let corner = (middle.round() as isize - size.width as isize / 2, 8);
    draw_text(
        &mut record,
        &label,
        corner,
        white,
        Some(shade),
        1,
        &FONT_6X13,
    );

    // The sixteen edge points and the circle fitted to them.
    for &p in &points {
        draw_crosshair(&mut record, rounded(p), 2, cyan);
    }
    draw_circle(
        &mut record,
        rounded(circle.center()),
        circle.radius().get().round() as u32,
        green,
        false,
    );
    // Diameter, in millimetres, and how well the circle fits, to its right
    // and centred on its height.
    let label = format!(
        "Ø {:.2} px\n= {:.4} mm\nRMS {:.3} px",
        2.0 * circle.radius().get(),
        2.0 * in_mm.element().radius().get(),
        fit.rms_residual().get(),
    );
    let size = FONT_6X13.text_size(&label, 1);
    let right = circle.center().x + circle.radius().get() + 8.0;
    let corner = (
        right.round() as isize,
        circle.center().y.round() as isize - size.height as isize / 2,
    );
    draw_text(
        &mut record,
        &label,
        corner,
        white,
        Some(shade),
        1,
        &FONT_6X13,
    );

    // The calibration, in the larger font, at the bottom left.
    let label = "12.5 µm / px";
    let size = FONT_10X20.text_size(label, 1);
    let corner = (8, (record.height() - size.height - 6) as isize);
    draw_text(&mut record, label, corner, yellow, None, 1, &FONT_10X20);

    // ── Archive or display ───────────────────────────────────────────────────
    if let Some(path) = args.save {
        let encoded = png::encode(&record, &PngEncodeOptions::default())
            .unwrap_or_else(|e| fail(&format!("PNG encode failed: {e}")));
        std::fs::write(&path, &encoded)
            .unwrap_or_else(|e| fail(&format!("cannot write {}: {e}", path.display())));
        println!("\nwrote {}", path.display());
        return Ok(());
    }
    println!("\nOpening 1 window — press any key to close");
    DebugDisplay::run(move |ctx| {
        ctx.show(
            "measure_label — caliper (yellow), edges (cyan), fitted circle (green)",
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

/// The scene, each pixel the light fraction of its area: a pixel covers
/// `x - 0.5 ..= x + 0.5`, and its value is 20 dark, 220 light, sampled 8 × 8
/// times across its area.
fn scene(width: usize, height: usize) -> Image<MonoF32> {
    const SUB: usize = 8;
    Image::generate(width, height, |x, y| {
        let mut light = 0;
        for j in 0..SUB {
            for i in 0..SUB {
                let sx = x as f64 - 0.5 + (i as f64 + 0.5) / SUB as f64;
                let sy = y as f64 - 0.5 + (j as f64 + 0.5) / SUB as f64;
                let in_bar = (BAR.0..BAR.1).contains(&sx) && (30.0..110.0).contains(&sy);
                let (cx, cy, r) = DISC;
                let in_disc = (sx - cx).powi(2) + (sy - cy).powi(2) < r * r;
                if in_bar || in_disc {
                    light += 1;
                }
            }
        }
        let coverage = light as f32 / (SUB * SUB) as f32;
        MonoF32::new(20.0 + 200.0 * coverage)
    })
}

/// The nearest pixel to `p`, as a drawing position.
fn rounded(p: Point<Pixels>) -> (isize, isize) {
    (p.x.round() as isize, p.y.round() as isize)
}

fn fail(msg: &str) -> ! {
    eprintln!("measure_label: {msg}");
    std::process::exit(1);
}
