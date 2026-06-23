//! # perona_malik — Perona-Malik anisotropic diffusion filter
//!
//! Edge-preserving image smoother based on Perona & Malik (1990).  Each
//! iteration updates every pixel according to the four-neighbour (N, S, E, W)
//! discretisation:
//!
//! ```text
//! I(t+1) = I(t) + λ · Σ_{n ∈ {N,S,E,W}} g(‖∇I_n‖) · (I_n − I)
//! ```
//!
//! The conductance function `g` controls the trade-off between smoothing and
//! edge preservation:
//!
//! | `--function` | Formula                      | Behaviour                      |
//! |--------------|------------------------------|--------------------------------|
//! | `exp`        | g(d) = exp(−(d/κ)²)          | Favours high-contrast edges    |
//! | `rat`        | g(d) = 1 / (1 + (d/κ)²)     | Favours wide, smooth regions   |
//!
//! Processing is done in **linear light** (`f32` / `RgbF32`), with sRGB gamma
//! encoding/decoding applied around the diffusion loop.  For colour images the
//! conductance is derived from an RMS RGB gradient magnitude so all three
//! channels share a single edge map while κ remains comparable to grayscale
//! inputs (vector Perona-Malik).
//!
//! ## Supported pixel formats
//!
//! | Pixel type  | PNG | JPEG | BMP |
//! |-------------|-----|------|-----|
//! | `SrgbMono8` | ✓   | ✓    | —   |
//! | `Srgb8`     | ✓   | ✓    | ✓   |
//!
//! Other pixel types (16-bit, alpha, indexed, …) are rejected with a
//! descriptive error.  Convert the image to `SrgbMono8` or `Srgb8` first —
//! for example with the `resize` example binary.
//!
//! ## Quick start
//!
//! ```text
//! # Demo defaults: 15 iterations, κ = 30, λ = 0.15, rational conductance
//! cargo run --bin perona_malik -- -i data/Mandrill.jpg
//!
//! # Subtler, more edge-preserving settings and a custom output path
//! cargo run --bin perona_malik -- \
//!     -i photo.png -n 15 -k 20 -l 0.1 -f exp -o output/photo_smooth.png
//! ```

use std::fmt;
use std::path::{Path, PathBuf};
use std::process;

use clap::Parser;

use fovea::border::Clamp;
use fovea::image::{Image, Neighborhood};
use fovea::pixel::*;
use fovea::transform::{MapItem, SrgbGamma, convert_image, map_neighborhood_fn};

use fovea_io::bmp::{self, BmpEncodeOptions};
use fovea_io::jpeg::{self, JpegEncodeOptions};
use fovea_io::png::{self, PngEncodeOptions, PngImage};
use fovea_io::{DecodedImage, ImageFormat};

// ─────────────────────────────────────────────────────────────────────────────
// CLI
// ─────────────────────────────────────────────────────────────────────────────

/// Apply Perona-Malik anisotropic diffusion to a PNG, JPEG, or BMP image.
///
/// The filter smooths homogeneous regions while sharpening or preserving
/// edges.  Edge sensitivity is governed by the diffusion coefficient κ.
#[derive(Parser)]
#[command(name = "perona_malik", version, about)]
struct Args {
    /// Input image file (PNG, JPEG, or BMP; SrgbMono8 or Srgb8 pixel formats).
    #[arg(short, long)]
    input: PathBuf,

    /// Output path (file or directory).
    ///
    /// If omitted, the result is written next to the input with a `_pm` suffix
    /// (e.g. `photo.jpg` → `photo_pm.jpg`).  If a directory is given, the
    /// same suffix scheme is applied inside that directory.  If a file path
    /// with a recognised image extension is given, that exact path and format
    /// are used.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Number of diffusion iterations.
    #[arg(short = 'n', long, default_value = "15")]
    iterations: u32,

    /// Diffusion coefficient κ (0–255 scale).
    ///
    /// Controls edge sensitivity: low κ preserves even faint edges; high κ
    /// allows diffusion across stronger edges.  Typical range: 5–60.
    #[arg(short = 'k', long, default_value = "30.0")]
    kappa: f32,

    /// Time-step λ per iteration.
    ///
    /// Must be in (0, 0.25] for the four-neighbour scheme to remain
    /// numerically stable.  Smaller values produce subtler changes per
    /// iteration but require more iterations for the same visible effect.
    #[arg(short = 'l', long, default_value = "0.15")]
    lambda: f32,

    /// Conductance function: "exp" (exponential) or "rat" (rational).
    ///
    ///   exp  g(d) = exp(-(d/κ)²)         favours high-contrast edges
    ///   rat  g(d) = 1 / (1 + (d/κ)²)    favours wide smooth regions
    #[arg(short = 'f', long, default_value = "rat")]
    function: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Conductance function
// ─────────────────────────────────────────────────────────────────────────────

/// Controls the relationship between gradient magnitude and diffusion rate.
#[derive(Debug, Clone, Copy)]
enum Conductance {
    /// Perona-Malik option 1: `g(d) = exp(-(d/κ)²)`.
    ///
    /// Gaussian fall-off gives a sharper transition at the edge threshold.
    /// Preferred when fine-detail preservation matters most.
    Exponential,

    /// Perona-Malik option 2: `g(d) = 1 / (1 + (d/κ)²)`.
    ///
    /// Lorentzian (Cauchy) fall-off is heavier-tailed, allowing more
    /// diffusion near the edge threshold.  Preferred for wide smooth regions.
    Rational,
}

impl Conductance {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "exp" => Ok(Conductance::Exponential),
            "rat" => Ok(Conductance::Rational),
            other => Err(format!(
                "unknown conductance function '{other}': choose 'exp' or 'rat'"
            )),
        }
    }

    /// Evaluate g for a **squared** difference `d²` and **squared** threshold `κ²`.
    ///
    /// Taking squared values avoids a `sqrt` on the inner hot-path loop.
    #[inline(always)]
    fn eval(self, d_sq: f32, kappa_sq: f32) -> f32 {
        match self {
            Conductance::Exponential => (-d_sq / kappa_sq).exp(),
            Conductance::Rational => 1.0 / (1.0 + d_sq / kappa_sq),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PM iteration steps
// ─────────────────────────────────────────────────────────────────────────────

/// One Perona-Malik iteration on a linear mono (`f32`) image.
///
/// Pixel values are expected in `[0.0, 1.0]` linear light.  With `λ ≤ 0.25`
/// and the four-neighbour cross stencil the scheme remains numerically bounded.
///
/// The cross mask includes the center pixel, but because its difference to
/// itself is zero, `g(0) · 0 = 0` — it contributes nothing to the update sum.
fn pm_step_mono(
    src: &Image<MonoF32>,
    lambda: f32,
    kappa_sq: f32,
    cond: Conductance,
) -> Image<MonoF32> {
    let mask = Neighborhood::<bool, 3, 3>::cross_3x3();
    map_neighborhood_fn(
        src,
        mask.weights(),
        mask.anchor(),
        &Clamp,
        move |center: MonoF32, neighbors: &mut dyn Iterator<Item = MapItem<MonoF32>>| {
            // Pixel role is MonoF32, but the PM math is a
            // plain scalar recurrence. Extract at the boundary, do arithmetic
            // in f32, and wrap the result — avoids redesigning the algebra.
            let center: f32 = center.0;
            let update: f32 = neighbors
                .map(|n| {
                    let diff = n.pixel.0 - center;
                    cond.eval(diff * diff, kappa_sq) * diff
                })
                .sum();
            MonoF32::new(center + lambda * update)
        },
    )
}

/// One Perona-Malik iteration on a linear colour (`RgbF32`) image.
///
/// Conductance is derived from the RMS RGB gradient magnitude so all three
/// channels share a single edge map — the vector extension of Perona-Malik.
/// RMS scaling keeps the user-facing κ parameter comparable to the mono case
/// instead of making colour edges artificially three times stronger.
fn pm_step_color(
    src: &Image<RgbF32>,
    lambda: f32,
    kappa_sq: f32,
    cond: Conductance,
) -> Image<RgbF32> {
    let mask = Neighborhood::<bool, 3, 3>::cross_3x3();
    map_neighborhood_fn(
        src,
        mask.weights(),
        mask.anchor(),
        &Clamp,
        move |center: RgbF32, neighbors: &mut dyn Iterator<Item = MapItem<RgbF32>>| {
            // Accumulate weighted flux for each channel via fold to keep all
            // captured variables immutable (avoids borrow-checker friction
            // with mutable temporaries inside nested closures).
            let (dr, dg, db) = neighbors.fold((0.0_f32, 0.0_f32, 0.0_f32), |(dr, dg, db), n| {
                let diff_r = n.pixel.r - center.r;
                let diff_g = n.pixel.g - center.g;
                let diff_b = n.pixel.b - center.b;
                // RMS colour squared gradient magnitude → shared conductance.
                let mag2 = (diff_r * diff_r + diff_g * diff_g + diff_b * diff_b) / 3.0;
                let g = cond.eval(mag2, kappa_sq);
                (dr + g * diff_r, dg + g * diff_g, db + g * diff_b)
            });
            RgbF32 {
                r: center.r + lambda * dr,
                g: center.g + lambda * dg,
                b: center.b + lambda * db,
            }
        },
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Type-specific diffusion pipelines
// ─────────────────────────────────────────────────────────────────────────────

/// Full Perona-Malik pipeline for `SrgbMono8` images.
///
/// 1. Linearise sRGB → `f32` in `[0, 1]` via `SrgbGamma`
/// 2. Run `iterations` PM steps in linear light
/// 3. Re-encode `f32` → `SrgbMono8` via `SrgbGamma` (clamps during quantisation)
fn diffuse_mono(
    img: &Image<SrgbMono8>,
    iterations: u32,
    lambda: f32,
    kappa_sq: f32,
    cond: Conductance,
) -> Image<SrgbMono8> {
    let mut linear: Image<MonoF32> = convert_image(img, SrgbGamma);
    for _ in 0..iterations {
        linear = pm_step_mono(&linear, lambda, kappa_sq, cond);
    }
    // SrgbGamma clamps values to [0, 1] during quantisation to u8.
    convert_image(&linear, SrgbGamma)
}

/// Full Perona-Malik pipeline for `Srgb8` images.
///
/// 1. Linearise sRGB → `RgbF32` in `[0, 1]³` via `SrgbGamma`
/// 2. Run `iterations` PM steps in linear light
/// 3. Re-encode `RgbF32` → `Srgb8` via `SrgbGamma`
fn diffuse_color(
    img: &Image<Srgb8>,
    iterations: u32,
    lambda: f32,
    kappa_sq: f32,
    cond: Conductance,
) -> Image<Srgb8> {
    let mut linear: Image<RgbF32> = convert_image(img, SrgbGamma);
    for _ in 0..iterations {
        linear = pm_step_color(&linear, lambda, kappa_sq, cond);
    }
    convert_image(&linear, SrgbGamma)
}

// ─────────────────────────────────────────────────────────────────────────────
// Output format helpers
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Png,
    Jpeg,
    Bmp,
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputFormat::Png => write!(f, "PNG"),
            OutputFormat::Jpeg => write!(f, "JPEG"),
            OutputFormat::Bmp => write!(f, "BMP"),
        }
    }
}

fn format_from_ext(ext: &str) -> Option<OutputFormat> {
    match ext.to_ascii_lowercase().as_str() {
        "png" => Some(OutputFormat::Png),
        "jpg" | "jpeg" => Some(OutputFormat::Jpeg),
        "bmp" => Some(OutputFormat::Bmp),
        _ => None,
    }
}

fn format_extension(fmt: OutputFormat) -> &'static str {
    match fmt {
        OutputFormat::Png => "png",
        OutputFormat::Jpeg => "jpg",
        OutputFormat::Bmp => "bmp",
    }
}

fn input_to_output_format(fmt: ImageFormat) -> Option<OutputFormat> {
    match fmt {
        ImageFormat::Png => Some(OutputFormat::Png),
        ImageFormat::Jpeg => Some(OutputFormat::Jpeg),
        ImageFormat::Bmp => Some(OutputFormat::Bmp),
    }
}

fn fmt_name(fmt: ImageFormat) -> &'static str {
    match fmt {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Bmp => "BMP",
    }
}

/// Resolve the output path and format from the CLI arguments.
///
/// Rules (in priority order):
///
/// 1. `--output` with a recognised image extension → use that path and format.
/// 2. `--output` with no extension → treat as a directory; derive the filename
///    from the input stem with a `_pm` suffix.
/// 3. No `--output` → write `<stem>_pm.<ext>` next to the input file.
fn resolve_output(
    input: &Path,
    output: Option<&Path>,
    default_fmt: OutputFormat,
) -> Result<(PathBuf, OutputFormat), String> {
    match output {
        None => {
            let stem = input.file_stem().unwrap_or_default().to_string_lossy();
            let ext = format_extension(default_fmt);
            let out_path = input.with_file_name(format!("{stem}_pm.{ext}"));
            Ok((out_path, default_fmt))
        }
        Some(out) => {
            let ext_str = out.extension().map(|e| e.to_string_lossy().into_owned());

            let fmt = match &ext_str {
                Some(ext) => format_from_ext(ext)
                    .ok_or_else(|| format!("unrecognised output extension '.{ext}'"))?,
                None => default_fmt,
            };

            let out_path = if ext_str.is_some() {
                // Treat as a complete file path.
                out.to_path_buf()
            } else {
                // No extension — treat as a directory.
                let stem = input.file_stem().unwrap_or_default().to_string_lossy();
                let ext = format_extension(fmt);
                out.join(format!("{stem}_pm.{ext}"))
            };

            Ok((out_path, fmt))
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Encode helpers
// ─────────────────────────────────────────────────────────────────────────────

fn encode_mono(img: &Image<SrgbMono8>, fmt: OutputFormat) -> Result<Vec<u8>, String> {
    match fmt {
        OutputFormat::Png => {
            png::encode(img, &PngEncodeOptions::default()).map_err(|e| e.to_string())
        }
        OutputFormat::Jpeg => {
            jpeg::encode(img, &JpegEncodeOptions::default()).map_err(|e| e.to_string())
        }
        OutputFormat::Bmp => Err("BMP does not support grayscale images; \
             use --output with a .png or .jpg extension instead"
            .into()),
    }
}

fn encode_color(img: &Image<Srgb8>, fmt: OutputFormat) -> Result<Vec<u8>, String> {
    match fmt {
        OutputFormat::Png => {
            png::encode(img, &PngEncodeOptions::default()).map_err(|e| e.to_string())
        }
        OutputFormat::Jpeg => {
            jpeg::encode(img, &JpegEncodeOptions::default()).map_err(|e| e.to_string())
        }
        OutputFormat::Bmp => {
            bmp::encode(img, &BmpEncodeOptions::default()).map_err(|e| e.to_string())
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main
// ─────────────────────────────────────────────────────────────────────────────

fn run() -> Result<(), String> {
    let args = Args::parse();

    // ── Validate filter parameters ───────────────────────────────────────────
    if args.lambda <= 0.0 || args.lambda > 0.25 {
        return Err(format!(
            "λ = {} is outside the valid range (0, 0.25]; the four-neighbour \
             PM scheme becomes numerically unstable for λ > 0.25",
            args.lambda
        ));
    }
    if args.kappa <= 0.0 {
        return Err(format!("κ = {} must be > 0", args.kappa));
    }
    if args.iterations == 0 {
        return Err("--iterations must be ≥ 1".into());
    }

    let cond = Conductance::parse(&args.function)?;

    // Scale κ from the conventional 0–255 range to the [0, 1] linear-light
    // space that the PM steps operate in.  The squared form avoids a repeated
    // multiply inside the inner loop.
    let kappa_linear = args.kappa / 255.0;
    let kappa_sq = kappa_linear * kappa_linear;

    // ── Read input ───────────────────────────────────────────────────────────
    let bytes = std::fs::read(&args.input)
        .map_err(|e| format!("failed to read '{}': {e}", args.input.display()))?;

    let input_fmt = fovea_io::detect_format(&bytes)
        .ok_or_else(|| format!("unrecognised image format: '{}'", args.input.display()))?;

    let default_output_fmt = input_to_output_format(input_fmt).ok_or_else(|| {
        format!(
            "input format {} is not supported; only PNG, JPEG, and BMP are accepted",
            fmt_name(input_fmt)
        )
    })?;

    // ── Decode ───────────────────────────────────────────────────────────────
    let decoded = fovea_io::load(&bytes)
        .map_err(|e| format!("failed to decode '{}': {e}", args.input.display()))?;

    // ── Resolve output path and format ───────────────────────────────────────
    let (out_path, out_fmt) =
        resolve_output(&args.input, args.output.as_deref(), default_output_fmt)?;

    eprintln!(
        "Input:  {}  ({})",
        args.input.display(),
        fmt_name(input_fmt)
    );
    eprintln!("Output: {}  ({})", out_path.display(), out_fmt);
    eprintln!(
        "Params: iterations={}, κ={}, λ={}, function={}",
        args.iterations, args.kappa, args.lambda, args.function
    );

    // ── Apply PM filter and encode ───────────────────────────────────────────
    //
    // Only SrgbMono8 and Srgb8 are supported — they are by far the most
    // common pixel types produced by PNG, JPEG, and BMP decoders.
    // All other pixel types yield a descriptive error suggesting a prior
    // conversion step (e.g. via the `resize` example).
    let encoded: Vec<u8> = match &decoded {
        // ── PNG ──────────────────────────────────────────────────────────────
        DecodedImage::Png(d) => match &d.image {
            PngImage::SrgbMono8(img) => {
                let result = diffuse_mono(img, args.iterations, args.lambda, kappa_sq, cond);
                encode_mono(&result, out_fmt)?
            }
            PngImage::Srgb8(img) => {
                let result = diffuse_color(img, args.iterations, args.lambda, kappa_sq, cond);
                encode_color(&result, out_fmt)?
            }
            _ => {
                return Err(
                    "unsupported PNG pixel format: only SrgbMono8 and Srgb8 are supported. \
                     Convert the image to 8-bit sRGB first (e.g. with the `resize` example)."
                        .into(),
                );
            }
        },

        // ── JPEG ─────────────────────────────────────────────────────────────
        DecodedImage::Jpeg(d) => match &d.image {
            jpeg::JpegImage::SrgbMono8(img) => {
                let result = diffuse_mono(img, args.iterations, args.lambda, kappa_sq, cond);
                encode_mono(&result, out_fmt)?
            }
            jpeg::JpegImage::Srgb8(img) => {
                let result = diffuse_color(img, args.iterations, args.lambda, kappa_sq, cond);
                encode_color(&result, out_fmt)?
            }
            _ => {
                return Err(
                    "unsupported JPEG pixel format: only SrgbMono8 and Srgb8 are supported.".into(),
                );
            }
        },

        // ── BMP ──────────────────────────────────────────────────────────────
        DecodedImage::Bmp(d) => match &d.image {
            bmp::BmpImage::Srgb8(img) => {
                let result = diffuse_color(img, args.iterations, args.lambda, kappa_sq, cond);
                encode_color(&result, out_fmt)?
            }
            _ => {
                return Err("unsupported BMP pixel format: only Srgb8 is supported. \
                     Indexed and RGBA BMP images must be converted to Srgb8 first."
                    .into());
            }
        },

        // Non-exhaustive guard — future codecs (e.g. WebP).
        _ => return Err("unsupported input format".into()),
    };

    // ── Write output ─────────────────────────────────────────────────────────
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create directory '{}': {e}", parent.display()))?;
        }
    }

    std::fs::write(&out_path, &encoded)
        .map_err(|e| format!("failed to write '{}': {e}", out_path.display()))?;

    eprintln!(
        "Done.  Wrote {} bytes to '{}'.",
        encoded.len(),
        out_path.display()
    );

    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        process::exit(1);
    }
}
