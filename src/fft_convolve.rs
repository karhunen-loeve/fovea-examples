//! # fft_convolve: a large defocus blur, directly and through the FFT
//!
//! Demonstrates `fovea::frequency::convolve` against the direct
//! `fovea::transform::convolve` on a real photograph:
//!
//! 1. Load the Terrace sample and linearise it to `MonoF32` with `SrgbGamma`.
//!    A defocus blur is a physical process on light, so it is applied to
//!    linear values, never to gamma-encoded ones.
//! 2. Build disc-shaped defocus kernels ("bokeh") of 15 × 15, 31 × 31 and
//!    61 × 61 pixels, each normalised to a sum of one.
//! 3. Convolve the image with each kernel twice, under the same border
//!    policy: directly, and through the DFT by `Radix2`.
//! 4. Print, per kernel, the time of each path, the speed-up, and the largest
//!    absolute difference between the two results. They compute the same
//!    convolution and agree to rounding.
//! 5. Show the original and the most blurred result, or save the blurred
//!    result as a PNG.
//!
//! ```text
//! cargo run --release --bin fft_convolve
//! cargo run --release --bin fft_convolve -- --save blurred.png
//! ```
//!
//! Run it with `--release`: a debug build makes the timings meaningless.
//! Without `--save`, press any key or close any window to exit.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Parser;

use fovea::border::Clamp;
use fovea::frequency::{self, Radix2};
use fovea::image::{Image, ImageView, Kernel, Neighborhood};
use fovea::pixel::{MonoF32, SrgbMono8};
use fovea::transform::{self, SrgbGamma, convert_image};
use fovea_display::{DebugDisplay, LinearToDisplay};
use fovea_io::jpeg::{self, JpegImage};
use fovea_io::png::{self, PngEncodeOptions};

#[derive(Parser)]
#[command(about = "A large defocus blur, directly and through the FFT")]
struct Args {
    /// Write the 61 × 61 result to this PNG file instead of opening windows
    #[arg(long)]
    save: Option<PathBuf>,
}

fn main() -> Result<(), fovea::Error> {
    let args = Args::parse();

    // ── 1. Load and linearise ────────────────────────────────────────────────
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/data/Terrace.jpg");
    let bytes = std::fs::read(path).unwrap_or_else(|e| fail(&format!("cannot read {path}: {e}")));
    let decoded =
        jpeg::decode(&bytes).unwrap_or_else(|e| fail(&format!("JPEG decode failed: {e}")));
    let JpegImage::SrgbMono8(mono) = decoded.image else {
        fail("expected an SrgbMono8 JPEG");
    };
    let linear: Image<MonoF32> = convert_image(&mono, SrgbGamma);
    println!(
        "Terrace {}×{} (MonoF32, linear)",
        linear.width(),
        linear.height()
    );

    if cfg!(debug_assertions) {
        println!(
            "\n  debug build: the timings below are not meaningful.\n    \
             Re-run with `cargo run --release --bin fft_convolve` to compare them.\n"
        );
    }

    // ── 2. The kernels ───────────────────────────────────────────────────────
    // A kernel's size is part of its type: `Neighborhood<f32, KW, KH>`, built
    // from a `[f32; KW * KH]` array. The array length is inferred from the
    // type, so `disc` only needs the side once more, as a value. Both
    // convolutions take any `Kernel<Weight = f32>`, and `Neighborhood` is the
    // one the crate provides, for sides up to 64. A 61 × 61 disc fits; a
    // 101 × 101 one, where the FFT would win by far more, cannot be written.
    let k15 = Neighborhood::<f32, 15, 15>::new(disc(15));
    let k31 = Neighborhood::<f32, 31, 31>::new(disc(31));
    let k61 = Neighborhood::<f32, 61, 61>::new(disc(61));

    // ── 3. and 4. Both paths, timed ──────────────────────────────────────────
    // `Clamp` for both: the FFT path pads the image by the kernel's extent
    // with values from the same policy, so the two outputs have the same
    // size and the same values at the edges, up to rounding.
    //
    // Where the two paths cross over depends on the machine, the image size
    // and the kernel size. The crate measures it in the docs of
    // `frequency::convolve` and leaves the choice to the caller: no function
    // switches between the two on its own. This table is how to find the
    // crossover for your own machine and image.
    println!("\n kernel    direct       FFT (Radix2)  speed-up   max |diff|");
    compare(&linear, &k15);
    compare(&linear, &k31);
    let blurred = compare(&linear, &k61);

    // ── 5. Save or display ───────────────────────────────────────────────────
    if let Some(path) = args.save {
        let encoded: Image<SrgbMono8> = convert_image(&blurred, SrgbGamma);
        let encoded = png::encode(&encoded, &PngEncodeOptions::default())
            .unwrap_or_else(|e| fail(&format!("PNG encode failed: {e}")));
        std::fs::write(&path, &encoded)
            .unwrap_or_else(|e| fail(&format!("cannot write {}: {e}", path.display())));
        println!("\nwrote {}", path.display());
        return Ok(());
    }
    println!("\nOpening 2 windows, press any key to close all");
    DebugDisplay::run(move |ctx| {
        ctx.show("1: Terrace luminance (linear)", &linear, LinearToDisplay);
        ctx.show(
            "2: 61 × 61 disc defocus, through the FFT",
            &blurred,
            LinearToDisplay,
        );
        match ctx.wait_key() {
            Some(key) => println!("Key pressed: {key:?}"),
            None => println!("All windows closed"),
        }
    });
    Ok(())
}

/// Convolve `image` with `kernel` directly and through the DFT, print one
/// row of the table, and return the result of the FFT path.
fn compare<K: Kernel<Weight = f32>>(image: &Image<MonoF32>, kernel: &K) -> Image<MonoF32> {
    let size = kernel.weights().size();

    let started = Instant::now();
    let direct: Image<MonoF32> = transform::convolve(image, kernel, &Clamp);
    let direct_time = started.elapsed();

    // `Radix2` pads every side to a power of two; `Auto` pads as it does in
    // a convolution and gives the same result, `Bluestein` pads less but
    // transforms each size more slowly.
    let started = Instant::now();
    let through_dft = frequency::convolve(image, kernel, &Clamp, Radix2);
    let fft_time = started.elapsed();

    println!(
        " {:>2} × {:<2}  {:>10}  {:>12}  {:>7.2}×   {:.2e}",
        size.width,
        size.height,
        format!("{direct_time:.1?}"),
        format!("{fft_time:.1?}"),
        ratio(direct_time, fft_time),
        largest_difference(&direct, &through_dft),
    );
    through_dft
}

/// A disc of diameter `side`, centred on the kernel, each weight the share
/// of the pixel's area inside the disc (8 × 8 samples per pixel), normalised
/// so that the weights sum to one and the blur keeps the mean brightness.
///
/// `N` is `side * side`, inferred from the `Neighborhood` it is passed to.
fn disc<const N: usize>(side: usize) -> [f32; N] {
    const SUB: usize = 8;
    assert_eq!(side * side, N, "the side must match the kernel's size");
    let centre = (side as f64 - 1.0) / 2.0;
    let radius = side as f64 / 2.0;
    let mut weights: [f32; N] = std::array::from_fn(|i| {
        let (x, y) = ((i % side) as f64, (i / side) as f64);
        let mut inside = 0;
        for j in 0..SUB {
            for k in 0..SUB {
                let sx = x - 0.5 + (k as f64 + 0.5) / SUB as f64 - centre;
                let sy = y - 0.5 + (j as f64 + 0.5) / SUB as f64 - centre;
                if sx * sx + sy * sy < radius * radius {
                    inside += 1;
                }
            }
        }
        inside as f32 / (SUB * SUB) as f32
    });
    let total: f32 = weights.iter().sum();
    weights.iter_mut().for_each(|w| *w /= total);
    weights
}

/// The largest absolute difference between two images of the same size.
fn largest_difference(a: &Image<MonoF32>, b: &Image<MonoF32>) -> f32 {
    assert_eq!(a.size(), b.size(), "both paths give the same output region");
    (0..a.height())
        .flat_map(|y| (0..a.width()).map(move |x| (x, y)))
        .map(|(x, y)| (a.pixel_at(x, y).0 - b.pixel_at(x, y).0).abs())
        .fold(0.0, f32::max)
}

/// How many times faster `b` ran than `a`.
fn ratio(a: Duration, b: Duration) -> f64 {
    a.as_secs_f64() / b.as_secs_f64()
}

fn fail(msg: &str) -> ! {
    eprintln!("fft_convolve: {msg}");
    std::process::exit(1);
}
