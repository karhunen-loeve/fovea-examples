//! # gamma_thumbnails — the same downscale, done wrong and done right
//!
//! Produces two thumbnails from one source photo:
//!
//! * `*_naive.png` — the byte average. Every mainstream pipeline does this:
//!   the sRGB-encoded bytes are treated as if they were light, averaged, and
//!   stored again. Results are systematically too dark wherever bright and
//!   dark samples meet.
//! * `*_linear.png` — the physically meaningful average: decode the sRGB
//!   transfer curve, interpolate in linear light, re-encode.
//!
//! Both paths take the *same* crop, the *same* target size and the *same*
//! filter, so the only difference between the two outputs is whether the
//! gamma curve was respected.
//!
//! Note what the naive path costs in this library: `Srgb8` deliberately does
//! not implement `LinearSpace`, so `resize(&srgb, size, Bilinear)` does not
//! compile. To reproduce the classic bug the encoded bytes have to be copied
//! into a linear pixel type on purpose — see `DropTheEncoding` below. The
//! bug is still writable, it just cannot happen by accident.
//!
//! ## Usage
//!
//! ```text
//! cargo run --release --bin gamma_thumbnails -- \
//!     --input photo.jpg --out-dir out --width 320 \
//!     --crop 160,105,2490,1640
//! ```

use std::path::PathBuf;

use clap::Parser;

use fovea::image::{Image, ImageView, SubView};
use fovea::pixel::{Rgb8, RgbF32, Srgb8};
use fovea::transform::{ConvertPixel, SrgbGamma, convert_image, pyr_down};
use fovea::{Coordinate, Rectangle, Size};

use fovea_io::png::{self, PngEncodeOptions, PngImage};
use fovea_io::{DecodedImage, jpeg};

#[derive(Parser)]
#[command(about = "Downscale one photo twice: naive byte average vs. linear light")]
struct Args {
    /// Source photo (JPEG or PNG, 8-bit sRGB colour)
    #[arg(short, long)]
    input: PathBuf,
    /// Directory the two thumbnails are written to
    #[arg(short, long, default_value = ".")]
    out_dir: PathBuf,
    /// Number of halving steps; each step is a Gaussian blur plus decimation
    #[arg(short, long, default_value_t = 3)]
    levels: u32,
    /// Crop taken before scaling, as `x,y,width,height` in source pixels
    #[arg(short, long, value_parser = parse_crop)]
    crop: Option<(usize, usize, usize, usize)>,
    /// Base name for the outputs; defaults to the input's file stem
    #[arg(short, long)]
    name: Option<String>,
}

fn parse_crop(s: &str) -> Result<(usize, usize, usize, usize), String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 4 {
        return Err("expected four comma-separated numbers: x,y,width,height".into());
    }
    let mut n = [0usize; 4];
    for (slot, text) in n.iter_mut().zip(&parts) {
        *slot = text
            .trim()
            .parse()
            .map_err(|_| format!("not a non-negative integer: {text}"))?;
    }
    Ok((n[0], n[1], n[2], n[3]))
}

// ─────────────────────────────────────────────────────────────────────────────
// The two paths
// ─────────────────────────────────────────────────────────────────────────────

/// The bug, as a named conversion strategy.
///
/// It copies the byte pattern through unchanged and redefines what it means:
/// `Srgb8` in, `Rgb8` out, no transfer curve anywhere. That is precisely what
/// happens when an ordinary photo is handed to `cv2.resize` — except there the
/// reinterpretation has no name, no call site and no place to hang a comment.
///
/// Written as a `ConvertPixel` strategy it becomes a thing with a name that
/// shows up in a diff, which is the whole argument of part 1 of the series
/// turned against the bug from part 2.
struct DropTheEncoding;

impl ConvertPixel<Srgb8, Rgb8> for DropTheEncoding {
    fn convert(&self, src: &Srgb8) -> Rgb8 {
        Rgb8::new(src.r.0, src.g.0, src.b.0)
    }
}

impl ConvertPixel<Rgb8, Srgb8> for DropTheEncoding {
    fn convert(&self, src: &Rgb8) -> Srgb8 {
        Srgb8::new(src.r.0, src.g.0, src.b.0)
    }
}

/// The naive path: pretend the encoded bytes are light, then halve `levels`
/// times. Each step is a 5-tap Gaussian followed by decimation, so every
/// output pixel really is an average of its neighbourhood — which is where
/// the gamma error comes from. A plain `Bilinear` reduction by a large factor
/// would show almost nothing, because it samples four neighbours per output
/// pixel and averages nothing.
fn downscale_naive(src: &Image<Srgb8>, levels: u32) -> Image<Srgb8> {
    let mut current: Image<Rgb8> = convert_image(src, DropTheEncoding);
    for _ in 0..levels {
        current = pyr_down(&current);
    }
    convert_image(&current, DropTheEncoding)
}

/// The same reduction, with the transfer curve respected: decode once,
/// halve in linear light, re-encode once.
fn downscale_linear(src: &Image<Srgb8>, levels: u32) -> Image<Srgb8> {
    let mut light: Image<RgbF32> = convert_image(src, SrgbGamma);
    for _ in 0..levels {
        light = pyr_down(&light);
    }
    convert_image(&light, SrgbGamma)
}

// ─────────────────────────────────────────────────────────────────────────────
// Measurement, so the caption can quote a number instead of an impression
// ─────────────────────────────────────────────────────────────────────────────

/// Mean relative luminance of an sRGB image, measured in **linear light**
/// (Rec. 709 weights). Averaging the stored bytes instead would reproduce the
/// very mistake being measured.
fn mean_luminance(img: &Image<Srgb8>) -> f64 {
    let light: Image<RgbF32> = convert_image(img, SrgbGamma);
    let mut sum = 0.0f64;
    for y in 0..light.height() {
        for x in 0..light.width() {
            let p = light.pixel_at(x, y);
            sum += 0.2126 * f64::from(p.r) + 0.7152 * f64::from(p.g) + 0.0722 * f64::from(p.b);
        }
    }
    sum / (light.width() * light.height()) as f64
}

/// Largest single-channel difference between the two thumbnails, in 8-bit codes.
fn max_channel_delta(a: &Image<Srgb8>, b: &Image<Srgb8>) -> u8 {
    let mut worst = 0u8;
    for y in 0..a.height() {
        for x in 0..a.width() {
            let (p, q) = (a.pixel_at(x, y), b.pixel_at(x, y));
            for (l, r) in [(p.r.0, q.r.0), (p.g.0, q.g.0), (p.b.0, q.b.0)] {
                worst = worst.max(l.abs_diff(r));
            }
        }
    }
    worst
}

// ─────────────────────────────────────────────────────────────────────────────

fn main() {
    let args = Args::parse();

    let bytes = std::fs::read(&args.input).unwrap_or_else(|e| {
        eprintln!("cannot read {}: {e}", args.input.display());
        std::process::exit(1);
    });

    let source: Image<Srgb8> = match fovea_io::load(&bytes) {
        Ok(DecodedImage::Jpeg(d)) => match d.image {
            jpeg::JpegImage::Srgb8(img) => img,
            _ => fail("expected an 8-bit sRGB colour JPEG"),
        },
        Ok(DecodedImage::Png(d)) => match d.image {
            PngImage::Srgb8(img) => img,
            _ => fail("expected an 8-bit sRGB colour PNG"),
        },
        Ok(_) => fail("unsupported container; use JPEG or PNG"),
        Err(e) => fail(&format!("decode failed: {e}")),
    };
    println!(
        "source: {} ({}x{}, Srgb8)",
        args.input.display(),
        source.width(),
        source.height()
    );

    // The crop both paths share. Taken before scaling so that neither path
    // sees a single pixel the other did not.
    let cropped: Image<Srgb8> = match args.crop {
        Some((x, y, w, h)) => {
            let rect = Rectangle::new(Coordinate::new(x, y), Size::new(w, h));
            let view = source
                .roi(rect)
                .unwrap_or_else(|| fail("crop lies outside the image"));
            let (crop_w, crop_h) = (view.width(), view.height());
            println!("crop:   {crop_w}x{crop_h} at ({x},{y})");
            Image::generate(crop_w, crop_h, |xx, yy| view.pixel_at(xx, yy))
        }
        None => source,
    };

    let naive = downscale_naive(&cropped, args.levels);
    let linear = downscale_linear(&cropped, args.levels);
    println!(
        "target: {}x{} ({} halving steps)",
        naive.width(),
        naive.height(),
        args.levels
    );

    let (l_naive, l_linear) = (mean_luminance(&naive), mean_luminance(&linear));
    let delta = max_channel_delta(&naive, &linear);
    println!("mean luminance: naive {l_naive:.4}, linear {l_linear:.4}");
    println!(
        "  the naive thumbnail is {:.1}% darker; worst single channel differs by {delta} codes",
        100.0 * (1.0 - l_naive / l_linear)
    );

    let stem = args.name.unwrap_or_else(|| {
        args.input
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "thumbnail".into())
    });
    write_png(&args.out_dir.join(format!("{stem}_naive.png")), &naive);
    write_png(&args.out_dir.join(format!("{stem}_linear.png")), &linear);
}

fn write_png(path: &std::path::Path, img: &Image<Srgb8>) {
    let encoded = png::encode(img, &PngEncodeOptions::default())
        .unwrap_or_else(|e| fail(&format!("PNG encode failed: {e}")));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(path, &encoded)
        .unwrap_or_else(|e| fail(&format!("cannot write {}: {e}", path.display())));
    println!("wrote {}", path.display());
}

fn fail(msg: &str) -> ! {
    eprintln!("gamma_thumbnails: {msg}");
    std::process::exit(1);
}
