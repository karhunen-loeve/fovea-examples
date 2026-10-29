//! # phase_drift: measure the sub-pixel drift of a sequence by phase correlation
//!
//! Demonstrates `fovea::frequency::PhaseReference` on a sequence whose
//! motion is known, as a camera on a slowly warming mount sees it:
//!
//! 1. A 256 × 256 crop of `data/Terrace.jpg`, in linear `MonoF32`, is the
//!    reference frame.
//! 2. Twenty-four frames are resampled from the photograph by `remap` with
//!    a `CatmullRom` kernel, each moved by a slow drift plus a small
//!    vibration, to a fraction of a pixel. Each frame also gets its own gain
//!    and offset, as a flickering light gives them, and a little noise.
//! 3. The radius of the frequency weight is chosen as the documentation of
//!    `phase_correlate` says, and checked once against the safe 0.03.
//! 4. The reference is prepared once with `PhaseReference`, and every frame
//!    is located against it. A table compares the measured shift with the
//!    one each frame was built with, and a summary gives the worst and the
//!    RMS error.
//! 5. A plot of the drift track: the built track as a line, each measured
//!    shift as a cross, and its error drawn 50 times enlarged.
//!
//! ```text
//! cargo run --release --bin phase_drift
//! cargo run --release --bin phase_drift -- --save track.png
//! ```
//!
//! Without `--save`, press any key or close a window to exit. With it, the
//! track plot is written as a PNG and no window opens.

use std::f64::consts::TAU;
use std::fs;
use std::path::PathBuf;

use clap::Parser;

use fovea::border::Clamp;
use fovea::draw::{FONT_6X13, draw_crosshair, draw_line, draw_polyline, draw_text};
use fovea::frequency::{Auto, CorrelationRadius, PhaseReference, phase_correlate};
use fovea::geometry::{SourceToDest, UniformScale, Vector};
use fovea::image::{Image, ImageView, RasterImageMut};
use fovea::pixel::{MonoF32, Srgb8};
use fovea::transform::{CatmullRom, SrgbGamma, convert_image, remap};
use fovea::{Pixels, Size, correlation_radius};
use fovea_display::{AutoContrast, DebugDisplay, Identity, LinearToDisplay};
use fovea_io::jpeg::{self, JpegImage};
use fovea_io::png::{self, PngEncodeOptions};

#[derive(Parser)]
#[command(about = "Measure the sub-pixel drift of a sequence by phase correlation")]
struct Args {
    /// Write the track plot to this PNG file instead of opening windows
    #[arg(long)]
    save: Option<PathBuf>,
}

/// The side of every frame, in pixels.
const SIDE: usize = 256;
/// How many frames follow the reference.
const FRAMES: usize = 24;
/// The standard deviation of the noise, in linear intensity.
const NOISE: f32 = 0.004;

fn main() -> Result<(), fovea::Error> {
    let args = Args::parse();

    // ── 1. The reference ─────────────────────────────────────────────────────
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/data/Terrace.jpg");
    let bytes = fs::read(path).unwrap_or_else(|e| fail(&format!("cannot read {path}: {e}")));
    let decoded = jpeg::decode(&bytes).unwrap_or_else(|e| fail(&format!("JPEG decode: {e}")));
    let JpegImage::SrgbMono8(mono) = decoded.image else {
        fail("expected a greyscale JPEG");
    };
    let photo: Image<MonoF32> = convert_image(&mono, SrgbGamma);
    // The crop's top-left corner, centred in the photograph.
    let corner = Vector::<Pixels>::new(
        ((photo.width() - SIDE) / 2) as f64,
        ((photo.height() - SIDE) / 2) as f64,
    );
    let reference = frame(&photo, corner, Vector::new(0.0, 0.0))?;

    // ── 2. The sequence ──────────────────────────────────────────────────────
    // Frame k moves by a drift of (0.16, −0.05) px per frame, plus a
    // vibration of a few tenths of a pixel at two periods that do not
    // divide each other.
    let mut noise = Noise(0x9e37_79b9_7f4a_7c15);
    let mut built = Vec::new();
    let mut frames = Vec::new();
    for k in 1..=FRAMES {
        let t = k as f64;
        let shift = Vector::<Pixels>::new(
            0.16 * t + 0.30 * (TAU * t / 6.0).sin(),
            -0.05 * t + 0.25 * (TAU * t / 4.3).sin(),
        );
        let gain = 1.0 + 0.25 * (TAU * t / 7.0).sin() as f32;
        let offset = 0.04 * (TAU * t / 5.0).cos() as f32;
        let mut lit = frame(&photo, corner, shift)?;
        for y in 0..SIDE {
            for p in lit.row_mut(y) {
                p.0 = gain * p.0 + offset + NOISE * noise.gaussian();
            }
        }
        built.push((shift, gain));
        frames.push(lit);
    }

    // ── 3. The radius ────────────────────────────────────────────────────────
    // A sharp photograph, slightly softened by the resampling kernel: 0.06
    // lies well inside what it holds. The check from the documentation:
    // locate one frame at the radius chosen and at 0.03, and compare. If the
    // two differ by more than the accuracy needed, 0.06 reaches too far.
    let radius = correlation_radius!(0.06);
    let last = frames.last().expect("the sequence has frames");
    let check = |r: CorrelationRadius| -> Result<Vector<Pixels>, fovea::Error> {
        Ok(phase_correlate(&reference, last, r, Auto)?
            .unwrap_or_else(|| fail("the photograph has no structure"))
            .shift())
    };
    let apart = (check(radius)? - check(correlation_radius!(0.03))?).length();
    println!(
        "radius {} cycles/px; on the last frame it differs from radius 0.03 by {:.4} px",
        radius.get(),
        apart.get(),
    );

    // ── 4. Locate every frame ────────────────────────────────────────────────
    // The reference is transformed once; each frame costs its own two passes.
    let prepared = PhaseReference::new(&reference, radius, Auto)?;
    println!("\nframe   gain   built (dx, dy) px      measured (dx, dy) px   error px   peak");
    let mut measured = Vec::new();
    let (mut worst, mut squares) = (0.0_f64, 0.0_f64);
    for (k, (lit, &(shift, gain))) in frames.iter().zip(&built).enumerate() {
        let found = prepared
            .locate(lit)?
            .unwrap_or_else(|| fail("a frame has no correlation peak"));
        let error = (found.shift() - shift).length().get();
        worst = worst.max(error);
        squares += error * error;
        println!(
            "{:>5}   {gain:.2}   ({:>7.3}, {:>7.3})     ({:>7.3}, {:>7.3})     {error:.4}   {:.3}",
            k + 1,
            shift.x,
            shift.y,
            found.shift().x,
            found.shift().y,
            found.peak(),
        );
        measured.push(found.shift());
    }
    println!(
        "\nover {FRAMES} frames: max error {worst:.4} px, RMS error {:.4} px",
        (squares / FRAMES as f64).sqrt(),
    );

    // ── 5. The track ─────────────────────────────────────────────────────────
    let truth: Vec<Vector<Pixels>> = built.iter().map(|&(s, _)| s).collect();
    let plot = track_plot(&truth, &measured);

    // ── Save or display ──────────────────────────────────────────────────────
    if let Some(path) = args.save {
        let encoded = png::encode(&plot, &PngEncodeOptions::default())
            .unwrap_or_else(|e| fail(&format!("PNG encode failed: {e}")));
        fs::write(&path, &encoded)
            .unwrap_or_else(|e| fail(&format!("cannot write {}: {e}", path.display())));
        println!("\nwrote {}", path.display());
        return Ok(());
    }
    let last = last.clone();
    println!("\nOpening 3 windows, press any key to close all");
    DebugDisplay::run(move |ctx| {
        ctx.show("reference", &reference, LinearToDisplay);
        // Gain, offset and noise take the last frame outside [0, 1].
        ctx.show(
            &format!("frame {FRAMES}: moved, relit and noisy"),
            &last,
            AutoContrast::scan_with(&last, |p| p.0 as f64),
        );
        ctx.show(
            "drift track: built (green), measured (cyan), error x50 (red)",
            &plot,
            Identity,
        );
        match ctx.wait_key() {
            Some(key) => println!("Key pressed: {key:?}"),
            None => println!("Window closed"),
        }
    });
    Ok(())
}

/// The frame of `SIDE` × `SIDE` pixels whose content is the photograph's at
/// `corner`, moved by `shift`: a point `p` of the reference lies at
/// `p + shift` in it.
fn frame(
    photo: &Image<MonoF32>,
    corner: Vector<Pixels>,
    shift: Vector<Pixels>,
) -> Result<Image<MonoF32>, fovea::Error> {
    // Where each pixel of the photograph lands in the frame; the remap
    // inverts it once and reads the photograph through `CatmullRom`.
    let placement = UniformScale::<Pixels, Pixels>::try_new(1.0, shift - corner)?;
    remap(
        photo,
        &SourceToDest::new(placement),
        CatmullRom,
        &Clamp,
        Size::new(SIDE, SIDE),
    )
}

/// The drift track on a plot of 80 plot pixels per image pixel.
fn track_plot(truth: &[Vector<Pixels>], measured: &[Vector<Pixels>]) -> Image<Srgb8> {
    const SCALE: f64 = 80.0;
    // The image pixel shown at the plot's top-left corner.
    const ORIGIN: (f64, f64) = (-0.6, -1.7);
    let mut plot = Image::fill(440, 220, Srgb8::new(24, 24, 32));
    let at = |v: Vector<Pixels>| {
        (
            ((v.x - ORIGIN.0) * SCALE).round() as isize,
            ((v.y - ORIGIN.1) * SCALE).round() as isize,
        )
    };
    let (grid, green) = (Srgb8::new(60, 60, 76), Srgb8::new(120, 255, 120));
    let (cyan, red) = (Srgb8::new(80, 220, 255), Srgb8::new(255, 80, 80));

    // Grid lines at whole pixels of shift.
    for i in 0..=5 {
        let x = at(Vector::new(i as f64, 0.0)).0;
        draw_line(&mut plot, (x, 0), (x, 219), grid);
    }
    for j in -1..=0 {
        let y = at(Vector::new(0.0, j as f64)).1;
        draw_line(&mut plot, (0, y), (439, y), grid);
    }

    // The built track, from the reference at zero shift.
    let line: Vec<(isize, isize)> = std::iter::once(Vector::new(0.0, 0.0))
        .chain(truth.iter().copied())
        .map(at)
        .collect();
    draw_polyline(&mut plot, &line, green, false);
    // Each measured shift, and its error 50 times enlarged.
    for (&t, &m) in truth.iter().zip(measured) {
        draw_line(&mut plot, at(t), at(t + (m - t) * 50.0), red);
        draw_crosshair(&mut plot, at(m), 3, cyan);
    }
    let label = "grid: 1 px of shift, error (red) x50";
    draw_text(&mut plot, label, (6, 204), green, None, 1, &FONT_6X13);
    plot
}

/// Gaussian noise of unit variance from a xorshift generator, by the
/// Box-Muller transform: enough for an example, with no dependency.
struct Noise(u64);

impl Noise {
    /// A uniform value in (0, 1].
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) + 1) as f64 / (1u64 << 53) as f64
    }

    fn gaussian(&mut self) -> f32 {
        let (u, v) = (self.uniform(), self.uniform());
        ((-2.0 * u.ln()).sqrt() * (TAU * v).cos()) as f32
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("phase_drift: {msg}");
    std::process::exit(1);
}
