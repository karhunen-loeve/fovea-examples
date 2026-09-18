//! # channel_swap — what a mislabelled buffer looks like
//!
//! Writes one photo twice:
//!
//! * `*_rgb.png` — the file as it really is.
//! * `*_bgr.png` — the same bytes, read by code that believes they are BGR.
//!
//! No colour is invented anywhere. The second image is produced by
//! *relabelling* the pixels and then letting an honest conversion do its job,
//! which is exactly the shape of the bug: nothing computes a wrong value, two
//! parts of a program simply disagree about what the bytes mean.
//!
//! The relabelling is byte-identical. `Srgb8` stores `r, g, b` and `SrgbBgr8`
//! stores `b, g, r`, so filling B's blue slot from A's red channel leaves
//! memory untouched and changes only the claim being made about it. Displaying
//! that claim needs `ColorSwap`, which does move the bytes, and the face turns
//! blue.
//!
//! ## Usage
//!
//! ```text
//! cargo run --release --bin channel_swap -- \
//!     --input portrait.jpg --out-dir out --width 320
//! ```

use std::path::PathBuf;

use clap::Parser;

use fovea::Size;
use fovea::image::{Image, ImageView};
use fovea::pixel::{RgbF32, Srgb8, SrgbBgr8};
use fovea::transform::{
    Bilinear, ColorSwap, ConvertPixel, SrgbGamma, convert_image, pyr_down, resize,
};

use fovea_io::png::{self, PngEncodeOptions, PngImage};
use fovea_io::{DecodedImage, jpeg};

#[derive(Parser)]
#[command(about = "Write a photo twice: as it is, and as BGR-believing code reads it")]
struct Args {
    /// Source photo (JPEG or PNG, 8-bit sRGB colour)
    #[arg(short, long)]
    input: PathBuf,
    /// Directory the two images are written to
    #[arg(short, long, default_value = ".")]
    out_dir: PathBuf,
    /// Output width in pixels; height follows the source aspect ratio
    #[arg(short, long, default_value_t = 320)]
    width: usize,
    /// Base name for the outputs; defaults to the input's file stem
    #[arg(short, long)]
    name: Option<String>,
}

/// The bug as a named strategy: the byte pattern is passed through unchanged
/// and relabelled as BGR. `Srgb8` is laid out `r, g, b` and `SrgbBgr8` is laid
/// out `b, g, r`, so taking B's blue slot from A's red channel rewrites the
/// label and not one byte of the data.
///
/// A camera SDK that documents its buffer as BGR, a decoder that hands back
/// RGB, and one line of glue between them: this is that line, with a name on
/// it. Without the types it is not a line at all, just an assumption nobody
/// wrote down.
struct MislabelAsBgr;

impl ConvertPixel<Srgb8, SrgbBgr8> for MislabelAsBgr {
    fn convert(&self, src: &Srgb8) -> SrgbBgr8 {
        SrgbBgr8::new(src.r.0, src.g.0, src.b.0)
    }
}

/// Gamma-correct reduction to the target width: decode the transfer curve,
/// halve while that overshoots, interpolate the rest, re-encode. Resizing on
/// the stored bytes would darken the result, which is the subject of part 2 of
/// the series and has no business in a figure about channel order.
fn downscale(src: &Image<Srgb8>, width: usize) -> Image<Srgb8> {
    let mut light: Image<RgbF32> = convert_image(src, SrgbGamma);
    while light.width() / 2 >= width {
        light = pyr_down(&light);
    }
    let height = (width as f64 * light.height() as f64 / light.width() as f64).round() as usize;
    let exact: Image<RgbF32> = resize(&light, Size::new(width, height.max(1)), Bilinear);
    convert_image(&exact, SrgbGamma)
}

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

    let truth = downscale(&source, args.width);
    println!("output: {}x{}", truth.width(), truth.height());

    // The bug, in two steps. Neither of them is wrong on its own.
    let mislabelled: Image<SrgbBgr8> = convert_image(&truth, MislabelAsBgr);
    let as_displayed: Image<Srgb8> = convert_image(&mislabelled, ColorSwap);

    // A pixel worth quoting in the caption: warm skin becomes cold sky.
    let (x, y) = (truth.width() / 2, truth.height() / 2);
    let a = truth.pixel_at(x, y);
    let b = as_displayed.pixel_at(x, y);
    println!(
        "centre pixel: [{}, {}, {}] read as RGB, [{}, {}, {}] once relabelled",
        a.r.0, a.g.0, a.b.0, b.r.0, b.g.0, b.b.0
    );

    let stem = args.name.unwrap_or_else(|| {
        args.input
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "portrait".into())
    });
    write_png(&args.out_dir.join(format!("{stem}_rgb.png")), &truth);
    write_png(&args.out_dir.join(format!("{stem}_bgr.png")), &as_displayed);
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
    eprintln!("channel_swap: {msg}");
    std::process::exit(1);
}
