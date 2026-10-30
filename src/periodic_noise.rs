//! # periodic_noise: find periodic interference in the spectrum and notch it out
//!
//! Demonstrates `fovea::frequency` on a photograph spoiled by two stripe
//! patterns of known frequency, the kind of interference a camera picks up
//! from a mains-powered light or a badly shielded cable:
//!
//! 1. A 512 × 512 crop of the Terrace sample, linearised to `MonoF32`, and
//!    two oblique sinusoids added to it.
//! 2. Its spectrum, by `Radix2`, since the crop's sides are powers of two.
//!    A sinusoid is one bright point and its mirror; the photograph's own
//!    content falls off smoothly from the centre.
//! 3. The peaks, found from the spectrum alone: bins that stand far above
//!    their surroundings, refined to a fraction of a bin. Each is printed
//!    beside the frequency that was injected.
//! 4. A `Notch` on every peak, and the inverse transform. The RMS error
//!    against the clean crop, before and after, says how much came back.
//!
//! ```text
//! cargo run --bin periodic_noise
//! cargo run --bin periodic_noise -- --save periodic_noise.png
//! ```
//!
//! Without `--save`, four windows open; press any key or close one to exit.
//! With it, the four panels are written side by side as a 2 × 2 PNG.

use std::f64::consts::TAU;
use std::fs;
use std::path::PathBuf;

use clap::Parser;

use fovea::SpectrumPixels;
use fovea::frequency::{
    Bins, Cutoff, CyclesPerPixel, Frequency, FrequencyIndex, Gaussian, Radix2, Spectrum, dft,
};
use fovea::geometry::PixelSpace;
use fovea::image::{Image, ImageView};
use fovea::pixel::{MonoF32, Srgba8};
use fovea::transform::{SrgbGamma, convert_image};
use fovea_display::{DebugDisplay, DisplayStrategy, LinearToDisplay, LogMagnitude};
use fovea_io::jpeg::{self, JpegImage};
use fovea_io::png::{self, PngEncodeOptions};

#[derive(Parser)]
#[command(about = "Find periodic interference in the spectrum and notch it out")]
struct Args {
    /// Write the four panels to this PNG file instead of opening windows
    #[arg(long)]
    save: Option<PathBuf>,
}

/// The side of the square crop: a power of two, so that `Radix2` takes it.
const SIDE: usize = 512;
/// The injected stripes: period in pixels, axis of the frequency in
/// degrees (the stripes themselves run at right angles to it), amplitude
/// in linear intensity.
const STRIPES: [(f64, f64, f64); 2] = [(7.3, 25.0, 0.06), (12.6, -58.0, 0.05)];
/// How far a peak must stand above the mean of its surroundings. On this
/// crop the two stripes stand at about 20 and 13, the photograph's own
/// local maxima at 4 or less.
const PEAK_RATIO: f32 = 8.0;

fn main() -> Result<(), fovea::Error> {
    let args = Args::parse();

    // ── 1. The clean crop, and the interference ──────────────────────────────
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/data/Terrace.jpg");
    let bytes = fs::read(path).unwrap_or_else(|e| fail(&format!("cannot read {path}: {e}")));
    let decoded = jpeg::decode(&bytes).unwrap_or_else(|e| fail(&format!("bad JPEG: {e}")));
    let JpegImage::SrgbMono8(mono) = decoded.image else {
        fail("expected Terrace to decode as SrgbMono8");
    };
    let linear: Image<MonoF32> = convert_image(&mono, SrgbGamma);
    let (x0, y0) = ((linear.width() - SIDE) / 2, (linear.height() - SIDE) / 2);
    let clean = Image::generate(SIDE, SIDE, |x, y| linear.pixel_at(x0 + x, y0 + y));
    let noisy = Image::generate(SIDE, SIDE, |x, y| {
        let wave: f64 = STRIPES
            .iter()
            .map(|&(period, axis, amplitude)| {
                let (fx, fy) = injected(period, axis);
                amplitude * (TAU * (fx * x as f64 + fy * y as f64)).cos()
            })
            .sum();
        MonoF32::new(clean.pixel_at(x, y).0 + wave as f32)
    });
    println!("Terrace crop {SIDE}×{SIDE} at ({x0}, {y0}), linear MonoF32");
    println!("RMS(noisy − clean)    = {:.5}", rms(&noisy, &clean));

    // ── 2. The spectrum ──────────────────────────────────────────────────────
    let mut spectrum = dft(&noisy, Radix2)?;
    let magnitude = spectrum.magnitude();

    // ── 3. The peaks, from the spectrum alone ────────────────────────────────
    let peaks = find_peaks(&magnitude, &spectrum);
    println!(
        "\n{} peaks found (ratio to surroundings > {PEAK_RATIO}):",
        peaks.len()
    );
    for (k, &peak) in peaks.iter().enumerate() {
        let found: Frequency<CyclesPerPixel> = spectrum.convert(peak).expect("the crop has pixels");
        let axis = found.axis().expect("a peak lies off the zero frequency");
        println!(
            "  peak {k}: bin ({:7.2}, {:7.2}), period {:.3} px, axis {:6.2}°",
            peak.fx(),
            peak.fy(),
            1.0 / found.radius(),
            axis.degrees(),
        );
    }
    println!("injected:");
    for (k, &(period, axis, amplitude)) in STRIPES.iter().enumerate() {
        let (fx, fy) = injected(period, axis);
        let bins: Frequency<Bins> = spectrum
            .convert(Frequency::cycles_per_pixel(fx, fy))
            .expect("the crop has pixels");
        println!(
            "  wave {k}: bin ({:7.2}, {:7.2}), period {period:.3} px, axis {axis:6.2}°, \
             amplitude {amplitude}",
            bins.fx(),
            bins.fy(),
        );
    }

    // ── 4. Notch every peak, and return to the image ─────────────────────────
    // A sinusoid between two bins leaks into its neighbours, so the notch is
    // a few bins wide; in bins, because leakage is counted in bins. The
    // leakage falls off only as 1 / distance, in a cross along both axes, so
    // a round notch never catches all of it: on this crop a cutoff of 2, 3,
    // 4 and 6 bins leaves an RMS error of 0.022, 0.018, 0.016 and 0.015,
    // while taking more of the photograph with it.
    let width = Cutoff::bins(3.0)?;
    for &peak in &peaks {
        spectrum.notch(peak, width, Gaussian);
    }
    let notched = spectrum.magnitude();
    let restored = spectrum.inverse(Radix2)?;
    println!("\nRMS(restored − clean) = {:.5}", rms(&restored, &clean));

    // What the same notches take from the clean crop: the price of the
    // repair, the photograph's own content at those frequencies.
    let mut clean_spectrum = dft(&clean, Radix2)?;
    for &peak in &peaks {
        clean_spectrum.notch(peak, width, Gaussian);
    }
    let cost = rms(&clean_spectrum.inverse(Radix2)?, &clean);
    println!("RMS(notched clean − clean) = {cost:.5}, the photograph's share at the notches");

    // ── Archive or display ───────────────────────────────────────────────────
    // Both spectra on one scale, so that the notches show as dark holes.
    let log = LogMagnitude::scan(&magnitude);
    if let Some(path) = args.save {
        let panels = [
            render(&noisy, LinearToDisplay),
            render(&magnitude, log),
            render(&notched, log),
            render(&restored, LinearToDisplay),
        ];
        let sheet = Image::generate(2 * SIDE, 2 * SIDE, |x, y| {
            panels[2 * (y / SIDE) + x / SIDE].pixel_at(x % SIDE, y % SIDE)
        });
        let encoded = png::encode(&sheet, &PngEncodeOptions::default())
            .unwrap_or_else(|e| fail(&format!("PNG encode failed: {e}")));
        fs::write(&path, &encoded)
            .unwrap_or_else(|e| fail(&format!("cannot write {}: {e}", path.display())));
        println!("\nwrote {}", path.display());
        return Ok(());
    }
    println!("\nOpening 4 windows, press any key to close");
    DebugDisplay::run(move |ctx| {
        ctx.show("1: with interference", &noisy, LinearToDisplay);
        ctx.show("2: log-magnitude spectrum", &magnitude, log);
        ctx.show("3: spectrum after the notches", &notched, log);
        ctx.show("4: restored", &restored, LinearToDisplay);
        match ctx.wait_key() {
            Some(key) => println!("Key pressed: {key:?}"),
            None => println!("Window closed"),
        }
    });
    Ok(())
}

/// The frequency in cycles per pixel of stripes with `period` pixels whose
/// frequency points along `axis` degrees.
fn injected(period: f64, axis: f64) -> (f64, f64) {
    let a = axis.to_radians();
    (a.cos() / period, a.sin() / period)
}

/// The isolated peaks of a centred magnitude spectrum, one of each mirror
/// pair, refined to a fraction of a bin.
///
/// A bin is a peak if it is the largest in its 5 × 5 neighbourhood, lies
/// above 0.04 cycles per pixel (below that, the photograph's own content
/// dominates), and is `PEAK_RATIO` times the mean of the ring around that
/// neighbourhood, out to 9 × 9. The edge of the crop puts a bright cross on
/// the axes of the spectrum, since the transform wraps the crop around, but
/// it falls off along the axis and has no local maximum there.
///
/// The refinement uses the shape of a sinusoid's peak: between bins `k` and
/// `k + 1`, at `k + δ`, the two magnitudes stand in the ratio
/// `(1 − δ) : δ`, so `δ = |X[k+1]| / (|X[k]| + |X[k+1]|)`, along each axis.
fn find_peaks(
    magnitude: &Image<MonoF32, SpectrumPixels>,
    spectrum: &Spectrum<MonoF32>,
) -> Vec<Frequency<Bins>> {
    let m = |x: usize, y: usize| magnitude.pixel_at(x, y).0;
    let mut peaks = Vec::new();
    for y in 4..SIDE - 4 {
        for x in 4..SIDE - 4 {
            // The bin this view pixel shows: the inverse of the centred layout.
            let FrequencyIndex { kx, ky } = FrequencyIndex::from_centered(x, y, magnitude.size());
            // One of each mirror pair: the half with non-negative kx.
            if kx < 0 || (kx == 0 && ky <= 0) {
                continue;
            }
            let f: Frequency<CyclesPerPixel> = spectrum
                .frequency_of(FrequencyIndex::new(kx, ky))
                .expect("the crop has pixels");
            if f.radius() < 0.04 {
                continue;
            }
            let here = m(x, y);
            let (mut ring, mut count, mut is_max) = (0.0, 0, true);
            for j in y - 4..=y + 4 {
                for i in x - 4..=x + 4 {
                    if i.abs_diff(x) <= 2 && j.abs_diff(y) <= 2 {
                        is_max &= (i, j) == (x, y) || m(i, j) < here;
                    } else {
                        ring += m(i, j);
                        count += 1;
                    }
                }
            }
            if !is_max || here < PEAK_RATIO * ring / count as f32 {
                continue;
            }
            let offset = |below: f32, above: f32| {
                if above > below {
                    f64::from(above / (here + above))
                } else {
                    -f64::from(below / (here + below))
                }
            };
            let dx = offset(m(x - 1, y), m(x + 1, y));
            let dy = offset(m(x, y - 1), m(x, y + 1));
            peaks.push(Frequency::bins(kx as f64 + dx, ky as f64 + dy));
        }
    }
    peaks
}

/// The root-mean-square difference of two images of one size.
fn rms(a: &Image<MonoF32>, b: &Image<MonoF32>) -> f64 {
    let mut sum = 0.0;
    for y in 0..a.height() {
        for x in 0..a.width() {
            sum += f64::from(a.pixel_at(x, y).0 - b.pixel_at(x, y).0).powi(2);
        }
    }
    (sum / (a.width() * a.height()) as f64).sqrt()
}

/// `image` as `strategy` shows it, for the saved sheet. Generic over the
/// space: the photo lies in `Pixels`, the spectrum views in `SpectrumPixels`.
fn render<S: PixelSpace>(
    image: &Image<MonoF32, S>,
    strategy: impl DisplayStrategy<MonoF32>,
) -> Image<Srgba8> {
    Image::generate(image.width(), image.height(), |x, y| {
        strategy.to_display(&image.pixel_at(x, y))
    })
}

fn fail(msg: &str) -> ! {
    eprintln!("periodic_noise: {msg}");
    std::process::exit(1);
}
