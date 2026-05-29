//! # resize — colour-space-aware image resizer
//!
//! A small CLI tool that resizes images while preserving the pixel format
//! and colour space.  sRGB-encoded pixels are linearised before bilinear
//! interpolation and re-encoded afterwards so that the resize is
//! perceptually correct.
//!
//! ## Usage
//!
//! ```text
//! cargo run --bin resize -- [OPTIONS] --input <FILE>
//!
//! Options:
//!   -i, --input <FILE>            Input image file
//!   -o, --output <PATH>           Output file or directory (optional)
//!   -W, --width <PX>              Target width in pixels
//!   -H, --height <PX>             Target height in pixels
//!   -s, --scaling-factor <FLOAT>  Uniform scaling factor (e.g. 0.5, 2.0)
//!   -h, --help                    Print help
//! ```
//!
//! ### Size rules
//!
//! * Specify **width and height** for an exact target size.
//! * Specify **only width or only height** — the other dimension is
//!   computed to preserve the aspect ratio.
//! * Specify **scaling-factor** — both dimensions are scaled uniformly.
//! * Combining width/height with scaling-factor is an error.
//! * At least one sizing parameter is required.
//!
//! ### Output rules
//!
//! * No `--output` → the resized file is written next to the input with a
//!   `_<W>x<H>` suffix (e.g. `photo_320x240.png`).
//! * `--output` is a **directory** (or has no file extension) → the file
//!   is placed there with the same suffix scheme.
//! * `--output` is a **file name** (has a recognised image extension) →
//!   that exact path is used, and the output format is determined by the
//!   extension.  A compatibility check ensures the pixel type is encodable
//!   in the target format; incompatible combinations (e.g. alpha → JPEG)
//!   are rejected with a descriptive error message.

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process;

use clap::Parser;

use fovea::Size;
use fovea::image::{Image, ImageView};
use fovea::pixel::*;
use fovea::transform::*;

use fovea_io::bmp::{self, BmpEncodeOptions};
use fovea_io::jpeg::{self, JpegEncodeOptions};
use fovea_io::png::{self, PngEncodeOptions, PngImage};
use fovea_io::{self, DecodedImage, ImageFormat};

// ─────────────────────────────────────────────────────────────────────────────
// CLI definition
// ─────────────────────────────────────────────────────────────────────────────

/// Resize images while preserving pixel format and colour space.
#[derive(Parser)]
#[command(name = "resize", version, about)]
struct Args {
    /// Input image file path.
    #[arg(short, long)]
    input: PathBuf,

    /// Output file or directory (optional).
    ///
    /// If omitted, the resized image is written next to the input with a
    /// `_<W>x<H>` suffix.  If a directory is given, the file is placed
    /// there with the same suffix.  If a file name with a recognised image
    /// extension is given, that exact path and format are used.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Target width in pixels.
    #[arg(short = 'W', long)]
    width: Option<usize>,

    /// Target height in pixels.
    #[arg(short = 'H', long)]
    height: Option<usize>,

    /// Uniform scaling factor (e.g. 0.5 for half size, 2.0 for double).
    #[arg(short, long)]
    scaling_factor: Option<f64>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Output format
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

/// Map a file extension to an output format.
fn format_from_extension(ext: &str) -> Option<OutputFormat> {
    match ext.to_ascii_lowercase().as_str() {
        "png" => Some(OutputFormat::Png),
        "jpg" | "jpeg" => Some(OutputFormat::Jpeg),
        "bmp" => Some(OutputFormat::Bmp),
        _ => None,
    }
}

fn input_format_to_output(fmt: ImageFormat) -> OutputFormat {
    match fmt {
        ImageFormat::Png => OutputFormat::Png,
        ImageFormat::Jpeg => OutputFormat::Jpeg,
        ImageFormat::Bmp => OutputFormat::Bmp,
    }
}

/// Return the default file extension for an output format.
fn format_extension(fmt: OutputFormat) -> &'static str {
    match fmt {
        OutputFormat::Png => "png",
        OutputFormat::Jpeg => "jpg",
        OutputFormat::Bmp => "bmp",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pixel-type descriptor (runtime tag for compatibility checks)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct PixelDesc {
    name: &'static str,
    has_alpha: bool,
    is_linear: bool,
    bit_depth: u8,
    is_indexed: bool,
    is_mono: bool,
}

impl PixelDesc {
    /// Check whether this pixel type can be encoded in `fmt`.
    /// Returns `Ok(())` or an `Err` with a human-readable reason.
    fn check_format_compat(&self, fmt: OutputFormat) -> Result<(), String> {
        match fmt {
            OutputFormat::Png => {
                // PNG can encode every pixel type we support.
                Ok(())
            }
            OutputFormat::Jpeg => {
                if self.has_alpha {
                    Err(format!(
                        "JPEG does not support alpha channels. \
                         Input pixel type is {} (has alpha).",
                        self.name
                    ))
                } else if self.is_linear {
                    Err(format!(
                        "JPEG requires sRGB-encoded data. \
                         Input pixel type {} uses linear colour space.",
                        self.name
                    ))
                } else if self.bit_depth > 8 {
                    Err(format!(
                        "JPEG encoder only supports 8-bit data. \
                         Input pixel type {} is {}-bit.",
                        self.name, self.bit_depth
                    ))
                } else if self.is_indexed {
                    Err(format!(
                        "JPEG does not support palette-indexed images ({}). \
                         Consider converting to truecolour first.",
                        self.name
                    ))
                } else {
                    // SrgbMono8, Srgb8
                    Ok(())
                }
            }
            OutputFormat::Bmp => {
                if self.is_linear {
                    Err(format!(
                        "BMP expects sRGB-encoded data. \
                         Input pixel type {} uses linear colour space.",
                        self.name
                    ))
                } else if self.bit_depth > 8 {
                    Err(format!(
                        "BMP only supports 8-bit depth. \
                         Input pixel type {} is {}-bit.",
                        self.name, self.bit_depth
                    ))
                } else if self.is_mono && !self.is_indexed {
                    Err(format!(
                        "BMP does not support grayscale pixel types ({}). \
                         Consider using PNG instead.",
                        self.name
                    ))
                } else {
                    // Srgb8, Srgba8, Indexed8
                    Ok(())
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Target-size computation
// ─────────────────────────────────────────────────────────────────────────────

/// Validate that the size arguments are not conflicting, and that at least
/// one sizing parameter is provided.  Called before any I/O so the user
/// gets fast feedback on bad arguments.
fn validate_size_args(
    width: Option<usize>,
    height: Option<usize>,
    scaling_factor: Option<f64>,
) -> Result<(), String> {
    match (
        width.is_some() || height.is_some(),
        scaling_factor.is_some(),
    ) {
        (true, true) => Err("cannot combine --width / --height with --scaling-factor".into()),
        (false, false) => {
            Err("specify at least one of --width, --height, or --scaling-factor".into())
        }
        _ => Ok(()),
    }
}

fn compute_target_size(
    src_w: usize,
    src_h: usize,
    width: Option<usize>,
    height: Option<usize>,
    scaling_factor: Option<f64>,
) -> Result<Size, String> {
    match (width, height, scaling_factor) {
        // Both width and height — use as-is.
        (Some(w), Some(h), None) => {
            if w == 0 || h == 0 {
                return Err("width and height must be > 0".into());
            }
            Ok(Size::new(w, h))
        }
        // Only width — compute height from aspect ratio.
        (Some(w), None, None) => {
            if w == 0 {
                return Err("width must be > 0".into());
            }
            let h = ((w as f64 / src_w as f64) * src_h as f64).round() as usize;
            Ok(Size::new(w, h.max(1)))
        }
        // Only height — compute width from aspect ratio.
        (None, Some(h), None) => {
            if h == 0 {
                return Err("height must be > 0".into());
            }
            let w = ((h as f64 / src_h as f64) * src_w as f64).round() as usize;
            Ok(Size::new(w.max(1), h))
        }
        // Scaling factor — apply to both dimensions.
        (None, None, Some(s)) => {
            if s <= 0.0 || !s.is_finite() {
                return Err("scaling-factor must be a positive finite number".into());
            }
            let w = (src_w as f64 * s).round() as usize;
            let h = (src_h as f64 * s).round() as usize;
            Ok(Size::new(w.max(1), h.max(1)))
        }
        // Conflict: width/height mixed with scaling-factor.
        (Some(_), _, Some(_)) | (_, Some(_), Some(_)) => {
            Err("cannot combine --width / --height with --scaling-factor".into())
        }
        // Nothing specified.
        (None, None, None) => {
            Err("specify at least one of --width, --height, or --scaling-factor".into())
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Output-path resolution
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `true` if `ext` is a recognised image file extension.
fn is_image_extension(ext: &OsStr) -> bool {
    let s = ext.to_string_lossy();
    matches!(
        s.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "bmp"
    )
}

/// Derive the output path, output format, and the size suffix string.
///
/// Rules:
/// * No `--output` → same directory as input, filename gets `_WxH` suffix,
///   extension matches the input format.
/// * `--output` has a recognised image extension → treat as file, derive
///   format from extension.
/// * Otherwise → treat as directory, place file there with the suffix and
///   the input format's extension.
fn resolve_output(
    input: &Path,
    output: Option<&Path>,
    target: &Size,
    input_fmt: OutputFormat,
) -> Result<(PathBuf, OutputFormat), String> {
    let suffix = format!("_{}x{}", target.width, target.height);

    let make_suffixed_name = |stem: &OsStr, ext: &str| -> String {
        format!("{}{}.{}", stem.to_string_lossy(), suffix, ext)
    };

    let input_stem = input
        .file_stem()
        .ok_or_else(|| "input path has no file stem".to_string())?;

    match output {
        None => {
            // Same directory as input, with suffix.
            let dir = input.parent().unwrap_or_else(|| Path::new("."));
            let ext = format_extension(input_fmt);
            let name = make_suffixed_name(input_stem, ext);
            Ok((dir.join(name), input_fmt))
        }
        Some(out) => {
            // Does the path have a recognised image extension?
            if let Some(ext) = out.extension() {
                if is_image_extension(ext) {
                    let fmt = format_from_extension(&ext.to_string_lossy()).ok_or_else(|| {
                        format!(
                            "recognised image extension '{}' has no encoder yet",
                            ext.to_string_lossy()
                        )
                    })?;
                    return Ok((out.to_path_buf(), fmt));
                }
            }
            // Treat as directory.
            let ext = format_extension(input_fmt);
            let name = make_suffixed_name(input_stem, ext);
            Ok((out.join(name), input_fmt))
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Encoding helpers
// ─────────────────────────────────────────────────────────────────────────────

fn encode_png_typed<P: png::PngPixel>(img: &Image<P>) -> Result<Vec<u8>, fovea_io::IoError> {
    png::encode(img, &PngEncodeOptions::default())
}

fn encode_jpeg_srgb8(img: &Image<Srgb8>) -> Result<Vec<u8>, fovea_io::IoError> {
    jpeg::encode(img, &JpegEncodeOptions::default())
}

fn encode_jpeg_srgb_mono8(img: &Image<SrgbMono8>) -> Result<Vec<u8>, fovea_io::IoError> {
    jpeg::encode(img, &JpegEncodeOptions::default())
}

fn encode_bmp_srgb8(img: &Image<Srgb8>) -> Result<Vec<u8>, fovea_io::IoError> {
    bmp::encode(img, &BmpEncodeOptions::default())
}

fn encode_bmp_srgba8(img: &Image<Srgba8>) -> Result<Vec<u8>, fovea_io::IoError> {
    bmp::encode(img, &BmpEncodeOptions::default())
}

// ─────────────────────────────────────────────────────────────────────────────
// Resize + encode dispatch (macros to tame the variant explosion)
// ─────────────────────────────────────────────────────────────────────────────

/// Resize a linear-space image with bilinear interpolation and encode it.
macro_rules! resize_linear {
    ($img:expr, $target:expr, $fmt:expr, $desc:expr, $pixel_ty:ty) => {{
        $desc.check_format_compat($fmt)?;
        let resized: Image<$pixel_ty> = resize($img, $target, Bilinear);
        encode_result(&resized, $fmt, $desc)
    }};
}

/// Resize an sRGB image: linearise → bilinear resize → re-encode sRGB.
macro_rules! resize_srgb {
    ($img:expr, $target:expr, $fmt:expr, $desc:expr, $linear_ty:ty, $srgb_ty:ty) => {{
        $desc.check_format_compat($fmt)?;
        let linear: Image<$linear_ty> = convert_image($img, SrgbGamma);
        let resized: Image<$linear_ty> = resize(&linear, $target, Bilinear);
        let result: Image<$srgb_ty> = convert_image(&resized, SrgbGamma);
        encode_result(&result, $fmt, $desc)
    }};
}

/// Encode a result image whose concrete type is known at the call site.
///
/// This is intentionally written as one function per (pixel, format) pair
/// hidden behind a `match` — the function is monomorphised at each call
/// site via the macros above, so we already know the concrete pixel type
/// and can call the right encoder.
///
/// The compatibility check has already been performed before this point,
/// so reaching an unsupported arm is a logic error.
fn encode_result<P>(img: &Image<P>, fmt: OutputFormat, desc: PixelDesc) -> Result<Vec<u8>, String>
where
    P: png::PngPixel,
{
    match fmt {
        OutputFormat::Png => encode_png_typed(img).map_err(|e| e.to_string()),
        _ => Err(format!(
            "internal: {} cannot be encoded as {} (should have been caught earlier)",
            desc.name, fmt
        )),
    }
}

/// Specialised encoder for `Srgb8` — supports PNG, JPEG, and BMP.
fn encode_srgb8(img: &Image<Srgb8>, fmt: OutputFormat) -> Result<Vec<u8>, String> {
    match fmt {
        OutputFormat::Png => encode_png_typed(img).map_err(|e| e.to_string()),
        OutputFormat::Jpeg => encode_jpeg_srgb8(img).map_err(|e| e.to_string()),
        OutputFormat::Bmp => encode_bmp_srgb8(img).map_err(|e| e.to_string()),
    }
}

/// Specialised encoder for `Srgba8` — supports PNG and BMP.
fn encode_srgba8(
    img: &Image<Srgba8>,
    fmt: OutputFormat,
    desc: PixelDesc,
) -> Result<Vec<u8>, String> {
    desc.check_format_compat(fmt)?;
    match fmt {
        OutputFormat::Png => encode_png_typed(img).map_err(|e| e.to_string()),
        OutputFormat::Bmp => encode_bmp_srgba8(img).map_err(|e| e.to_string()),
        _ => unreachable!(),
    }
}

/// Specialised encoder for `SrgbMono8` — supports PNG and JPEG.
fn encode_srgb_mono8(
    img: &Image<SrgbMono8>,
    fmt: OutputFormat,
    desc: PixelDesc,
) -> Result<Vec<u8>, String> {
    desc.check_format_compat(fmt)?;
    match fmt {
        OutputFormat::Png => encode_png_typed(img).map_err(|e| e.to_string()),
        OutputFormat::Jpeg => encode_jpeg_srgb_mono8(img).map_err(|e| e.to_string()),
        _ => unreachable!(),
    }
}

/// Encode indexed data — supports PNG and BMP.
fn encode_indexed(
    img: &Image<Indexed8>,
    palette: &[Srgba8; 256],
    fmt: OutputFormat,
    desc: PixelDesc,
) -> Result<Vec<u8>, String> {
    desc.check_format_compat(fmt)?;
    match fmt {
        OutputFormat::Png => {
            png::encode_indexed(img, palette.as_ref(), &PngEncodeOptions::default())
                .map_err(|e| e.to_string())
        }
        OutputFormat::Bmp => {
            bmp::encode_indexed(img, palette.as_ref(), &BmpEncodeOptions::default())
                .map_err(|e| e.to_string())
        }
        _ => unreachable!(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pixel descriptors
// ─────────────────────────────────────────────────────────────────────────────

const DESC_SRGB_MONO8: PixelDesc = PixelDesc {
    name: "SrgbMono8",
    has_alpha: false,
    is_linear: false,
    bit_depth: 8,
    is_indexed: false,
    is_mono: true,
};
const DESC_SRGB_MONO_A8: PixelDesc = PixelDesc {
    name: "SrgbMonoA8",
    has_alpha: true,
    is_linear: false,
    bit_depth: 8,
    is_indexed: false,
    is_mono: true,
};
const DESC_SRGB8: PixelDesc = PixelDesc {
    name: "Srgb8",
    has_alpha: false,
    is_linear: false,
    bit_depth: 8,
    is_indexed: false,
    is_mono: false,
};
const DESC_SRGBA8: PixelDesc = PixelDesc {
    name: "Srgba8",
    has_alpha: true,
    is_linear: false,
    bit_depth: 8,
    is_indexed: false,
    is_mono: false,
};
const DESC_MONO8: PixelDesc = PixelDesc {
    name: "Mono8",
    has_alpha: false,
    is_linear: true,
    bit_depth: 8,
    is_indexed: false,
    is_mono: true,
};
const DESC_MONO_A8: PixelDesc = PixelDesc {
    name: "MonoA8",
    has_alpha: true,
    is_linear: true,
    bit_depth: 8,
    is_indexed: false,
    is_mono: true,
};
const DESC_RGB8: PixelDesc = PixelDesc {
    name: "Rgb8",
    has_alpha: false,
    is_linear: true,
    bit_depth: 8,
    is_indexed: false,
    is_mono: false,
};
const DESC_RGBA8: PixelDesc = PixelDesc {
    name: "Rgba8",
    has_alpha: true,
    is_linear: true,
    bit_depth: 8,
    is_indexed: false,
    is_mono: false,
};
const DESC_SRGB_MONO16: PixelDesc = PixelDesc {
    name: "SrgbMono16",
    has_alpha: false,
    is_linear: false,
    bit_depth: 16,
    is_indexed: false,
    is_mono: true,
};
const DESC_SRGB_MONO_A16: PixelDesc = PixelDesc {
    name: "SrgbMonoA16",
    has_alpha: true,
    is_linear: false,
    bit_depth: 16,
    is_indexed: false,
    is_mono: true,
};
const DESC_SRGB16: PixelDesc = PixelDesc {
    name: "Srgb16",
    has_alpha: false,
    is_linear: false,
    bit_depth: 16,
    is_indexed: false,
    is_mono: false,
};
const DESC_SRGBA16: PixelDesc = PixelDesc {
    name: "Srgba16",
    has_alpha: true,
    is_linear: false,
    bit_depth: 16,
    is_indexed: false,
    is_mono: false,
};
const DESC_MONO16: PixelDesc = PixelDesc {
    name: "Mono16",
    has_alpha: false,
    is_linear: true,
    bit_depth: 16,
    is_indexed: false,
    is_mono: true,
};
const DESC_MONO_A16: PixelDesc = PixelDesc {
    name: "MonoA16",
    has_alpha: true,
    is_linear: true,
    bit_depth: 16,
    is_indexed: false,
    is_mono: true,
};
const DESC_RGB16: PixelDesc = PixelDesc {
    name: "Rgb16",
    has_alpha: false,
    is_linear: true,
    bit_depth: 16,
    is_indexed: false,
    is_mono: false,
};
const DESC_RGBA16: PixelDesc = PixelDesc {
    name: "Rgba16",
    has_alpha: true,
    is_linear: true,
    bit_depth: 16,
    is_indexed: false,
    is_mono: false,
};
const DESC_INDEXED8: PixelDesc = PixelDesc {
    name: "Indexed8",
    has_alpha: false,
    is_linear: false,
    bit_depth: 8,
    is_indexed: true,
    is_mono: false,
};

// ─────────────────────────────────────────────────────────────────────────────
// Core dispatch
// ─────────────────────────────────────────────────────────────────────────────

fn resize_png_image(image: &PngImage, target: Size, fmt: OutputFormat) -> Result<Vec<u8>, String> {
    match image {
        // ── sRGB types: linearise → bilinear → re-encode ─────────────
        PngImage::SrgbMono8(img) => {
            let desc = DESC_SRGB_MONO8;
            desc.check_format_compat(fmt)?;
            let linear: Image<MonoF32> = convert_image(img, SrgbGamma);
            let resized: Image<MonoF32> = resize(&linear, target, Bilinear);
            let result: Image<SrgbMono8> = convert_image(&resized, SrgbGamma);
            encode_srgb_mono8(&result, fmt, desc)
        }
        PngImage::SrgbMonoA8(img) => {
            resize_srgb!(img, target, fmt, DESC_SRGB_MONO_A8, MonoAF32, SrgbMonoA8)
        }
        PngImage::Srgb8(img) => {
            let desc = DESC_SRGB8;
            desc.check_format_compat(fmt)?;
            let linear: Image<RgbF32> = convert_image(img, SrgbGamma);
            let resized: Image<RgbF32> = resize(&linear, target, Bilinear);
            let result: Image<Srgb8> = convert_image(&resized, SrgbGamma);
            encode_srgb8(&result, fmt)
        }
        PngImage::Srgba8(img) => {
            let desc = DESC_SRGBA8;
            desc.check_format_compat(fmt)?;
            let linear: Image<RgbaF32> = convert_image(img, SrgbGamma);
            let resized: Image<RgbaF32> = resize(&linear, target, Bilinear);
            let result: Image<Srgba8> = convert_image(&resized, SrgbGamma);
            encode_srgba8(&result, fmt, desc)
        }
        PngImage::Srgb16(img) => {
            resize_srgb!(img, target, fmt, DESC_SRGB16, RgbF32, Srgb16)
        }
        PngImage::Srgba16(img) => {
            resize_srgb!(img, target, fmt, DESC_SRGBA16, RgbaF32, Srgba16)
        }
        PngImage::SrgbMono16(img) => {
            resize_srgb!(img, target, fmt, DESC_SRGB_MONO16, MonoF32, SrgbMono16)
        }
        PngImage::SrgbMonoA16(img) => {
            resize_srgb!(img, target, fmt, DESC_SRGB_MONO_A16, MonoAF32, SrgbMonoA16)
        }
        // ── Linear types: bilinear directly ──────────────────────────
        PngImage::Mono8(img) => resize_linear!(img, target, fmt, DESC_MONO8, Mono8),
        PngImage::MonoA8(img) => resize_linear!(img, target, fmt, DESC_MONO_A8, MonoA8),
        PngImage::Rgb8(img) => resize_linear!(img, target, fmt, DESC_RGB8, Rgb8),
        PngImage::Rgba8(img) => resize_linear!(img, target, fmt, DESC_RGBA8, Rgba8),
        PngImage::Mono16(img) => resize_linear!(img, target, fmt, DESC_MONO16, Mono16),
        PngImage::MonoA16(img) => resize_linear!(img, target, fmt, DESC_MONO_A16, MonoA16),
        PngImage::Rgb16(img) => resize_linear!(img, target, fmt, DESC_RGB16, Rgb16),
        PngImage::Rgba16(img) => resize_linear!(img, target, fmt, DESC_RGBA16, Rgba16),
        // ── Indexed: nearest-neighbour (palette preserved) ───────────
        PngImage::Indexed8 { data, palette } => {
            let desc = DESC_INDEXED8;
            desc.check_format_compat(fmt)?;
            let resized: Image<Indexed8> = resize(data, target, NearestNeighbor);
            encode_indexed(&resized, palette, fmt, desc)
        }
    }
}

fn resize_jpeg_image(
    image: &jpeg::JpegImage,
    target: Size,
    fmt: OutputFormat,
) -> Result<Vec<u8>, String> {
    match image {
        jpeg::JpegImage::Srgb8(img) => {
            let desc = DESC_SRGB8;
            desc.check_format_compat(fmt)?;
            let linear: Image<RgbF32> = convert_image(img, SrgbGamma);
            let resized: Image<RgbF32> = resize(&linear, target, Bilinear);
            let result: Image<Srgb8> = convert_image(&resized, SrgbGamma);
            encode_srgb8(&result, fmt)
        }
        jpeg::JpegImage::SrgbMono8(img) => {
            let desc = DESC_SRGB_MONO8;
            desc.check_format_compat(fmt)?;
            let linear: Image<MonoF32> = convert_image(img, SrgbGamma);
            let resized: Image<MonoF32> = resize(&linear, target, Bilinear);
            let result: Image<SrgbMono8> = convert_image(&resized, SrgbGamma);
            encode_srgb_mono8(&result, fmt, desc)
        }
        jpeg::JpegImage::SrgbMono16(img) => {
            // 12-bit JPEG decoded to SrgbMono16.
            // Can only re-encode to PNG (JPEG encoder doesn't support 16-bit).
            let desc = DESC_SRGB_MONO16;
            desc.check_format_compat(fmt)?;
            let linear: Image<MonoF32> = convert_image(img, SrgbGamma);
            let resized: Image<MonoF32> = resize(&linear, target, Bilinear);
            let result: Image<SrgbMono16> = convert_image(&resized, SrgbGamma);
            encode_result(&result, fmt, desc)
        }
    }
}

fn resize_bmp_image(
    image: &bmp::BmpImage,
    target: Size,
    fmt: OutputFormat,
) -> Result<Vec<u8>, String> {
    match image {
        bmp::BmpImage::Srgb8(img) => {
            let desc = DESC_SRGB8;
            desc.check_format_compat(fmt)?;
            let linear: Image<RgbF32> = convert_image(img, SrgbGamma);
            let resized: Image<RgbF32> = resize(&linear, target, Bilinear);
            let result: Image<Srgb8> = convert_image(&resized, SrgbGamma);
            encode_srgb8(&result, fmt)
        }
        bmp::BmpImage::Srgba8(img) => {
            let desc = DESC_SRGBA8;
            desc.check_format_compat(fmt)?;
            let linear: Image<RgbaF32> = convert_image(img, SrgbGamma);
            let resized: Image<RgbaF32> = resize(&linear, target, Bilinear);
            let result: Image<Srgba8> = convert_image(&resized, SrgbGamma);
            encode_srgba8(&result, fmt, desc)
        }
        bmp::BmpImage::Indexed8 { data, palette } => {
            let desc = DESC_INDEXED8;
            desc.check_format_compat(fmt)?;
            let resized: Image<Indexed8> = resize(data, target, NearestNeighbor);
            encode_indexed(&resized, palette, fmt, desc)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Extract (width, height) from a decoded image.
fn decoded_dimensions(decoded: &DecodedImage) -> (usize, usize) {
    match decoded {
        DecodedImage::Png(d) => png_dimensions(&d.image),
        DecodedImage::Jpeg(d) => jpeg_dimensions(&d.image),
        DecodedImage::Bmp(d) => bmp_dimensions(&d.image),
        _ => panic!("unsupported decoded format"),
    }
}

fn png_dimensions(img: &PngImage) -> (usize, usize) {
    match img {
        PngImage::SrgbMono8(i) => (i.width(), i.height()),
        PngImage::SrgbMonoA8(i) => (i.width(), i.height()),
        PngImage::Srgb8(i) => (i.width(), i.height()),
        PngImage::Srgba8(i) => (i.width(), i.height()),
        PngImage::Mono8(i) => (i.width(), i.height()),
        PngImage::MonoA8(i) => (i.width(), i.height()),
        PngImage::Rgb8(i) => (i.width(), i.height()),
        PngImage::Rgba8(i) => (i.width(), i.height()),
        PngImage::SrgbMono16(i) => (i.width(), i.height()),
        PngImage::SrgbMonoA16(i) => (i.width(), i.height()),
        PngImage::Srgb16(i) => (i.width(), i.height()),
        PngImage::Srgba16(i) => (i.width(), i.height()),
        PngImage::Mono16(i) => (i.width(), i.height()),
        PngImage::MonoA16(i) => (i.width(), i.height()),
        PngImage::Rgb16(i) => (i.width(), i.height()),
        PngImage::Rgba16(i) => (i.width(), i.height()),
        PngImage::Indexed8 { data, .. } => (data.width(), data.height()),
    }
}

fn jpeg_dimensions(img: &jpeg::JpegImage) -> (usize, usize) {
    match img {
        jpeg::JpegImage::SrgbMono8(i) => (i.width(), i.height()),
        jpeg::JpegImage::SrgbMono16(i) => (i.width(), i.height()),
        jpeg::JpegImage::Srgb8(i) => (i.width(), i.height()),
    }
}

fn bmp_dimensions(img: &bmp::BmpImage) -> (usize, usize) {
    match img {
        bmp::BmpImage::Srgb8(i) => (i.width(), i.height()),
        bmp::BmpImage::Srgba8(i) => (i.width(), i.height()),
        bmp::BmpImage::Indexed8 { data, .. } => (data.width(), data.height()),
    }
}

/// Return the name of the decoded pixel variant for diagnostics.
fn decoded_pixel_name(decoded: &DecodedImage) -> &'static str {
    match decoded {
        DecodedImage::Png(d) => match &d.image {
            PngImage::SrgbMono8(_) => "SrgbMono8",
            PngImage::SrgbMonoA8(_) => "SrgbMonoA8",
            PngImage::Srgb8(_) => "Srgb8",
            PngImage::Srgba8(_) => "Srgba8",
            PngImage::Mono8(_) => "Mono8",
            PngImage::MonoA8(_) => "MonoA8",
            PngImage::Rgb8(_) => "Rgb8",
            PngImage::Rgba8(_) => "Rgba8",
            PngImage::SrgbMono16(_) => "SrgbMono16",
            PngImage::SrgbMonoA16(_) => "SrgbMonoA16",
            PngImage::Srgb16(_) => "Srgb16",
            PngImage::Srgba16(_) => "Srgba16",
            PngImage::Mono16(_) => "Mono16",
            PngImage::MonoA16(_) => "MonoA16",
            PngImage::Rgb16(_) => "Rgb16",
            PngImage::Rgba16(_) => "Rgba16",
            PngImage::Indexed8 { .. } => "Indexed8",
        },
        DecodedImage::Jpeg(d) => match &d.image {
            jpeg::JpegImage::SrgbMono8(_) => "SrgbMono8",
            jpeg::JpegImage::SrgbMono16(_) => "SrgbMono16",
            jpeg::JpegImage::Srgb8(_) => "Srgb8",
        },
        DecodedImage::Bmp(d) => match &d.image {
            bmp::BmpImage::Srgb8(_) => "Srgb8",
            bmp::BmpImage::Srgba8(_) => "Srgba8",
            bmp::BmpImage::Indexed8 { .. } => "Indexed8",
        },
        _ => "unknown",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main
// ─────────────────────────────────────────────────────────────────────────────

fn run() -> Result<(), String> {
    let args = Args::parse();

    // ── 1. Validate CLI args early (before any I/O) ──────────────────────
    validate_size_args(args.width, args.height, args.scaling_factor)?;

    // ── 2. Read input ────────────────────────────────────────────────────
    let input_bytes = std::fs::read(&args.input)
        .map_err(|e| format!("failed to read '{}': {e}", args.input.display()))?;

    // ── 3. Detect input format ───────────────────────────────────────────
    let input_fmt = fovea_io::detect_format(&input_bytes)
        .ok_or_else(|| format!("unrecognised image format for '{}'", args.input.display()))?;

    // ── 4. Decode ────────────────────────────────────────────────────────
    let decoded = fovea_io::load(&input_bytes)
        .map_err(|e| format!("failed to decode '{}': {e}", args.input.display()))?;

    let (src_w, src_h) = decoded_dimensions(&decoded);
    let pixel_name = decoded_pixel_name(&decoded);

    eprintln!(
        "Input: {} ({}x{}, {}, {})",
        args.input.display(),
        src_w,
        src_h,
        pixel_name,
        input_fmt_name(input_fmt),
    );

    // ── 5. Compute target size ───────────────────────────────────────────
    let target = compute_target_size(src_w, src_h, args.width, args.height, args.scaling_factor)?;

    eprintln!("Target size: {}x{}", target.width, target.height);

    // ── 6. Resolve output path and format ────────────────────────────────
    let out_input_fmt = input_format_to_output(input_fmt);
    let (output_path, output_fmt) =
        resolve_output(&args.input, args.output.as_deref(), &target, out_input_fmt)?;

    eprintln!("Output: {} ({})", output_path.display(), output_fmt);

    // ── 7. Resize and encode ─────────────────────────────────────────────
    let encoded = match &decoded {
        DecodedImage::Png(d) => resize_png_image(&d.image, target, output_fmt)?,
        DecodedImage::Jpeg(d) => resize_jpeg_image(&d.image, target, output_fmt)?,
        DecodedImage::Bmp(d) => resize_bmp_image(&d.image, target, output_fmt)?,
        _ => return Err("unsupported input format".into()),
    };

    // ── 8. Write output ──────────────────────────────────────────────────
    if let Some(parent) = output_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create directory '{}': {e}", parent.display()))?;
        }
    }

    std::fs::write(&output_path, &encoded)
        .map_err(|e| format!("failed to write '{}': {e}", output_path.display()))?;

    eprintln!(
        "Done. Wrote {} bytes to {}",
        encoded.len(),
        output_path.display()
    );

    Ok(())
}

fn input_fmt_name(fmt: ImageFormat) -> &'static str {
    match fmt {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Bmp => "BMP",
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
        process::exit(1);
    }
}
