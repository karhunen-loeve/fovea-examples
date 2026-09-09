//! # web_copy: a lighter JPEG of a master, with the cost measured
//!
//! Some images have two jobs. A master is the thing figures are generated
//! from, and it wants every bit it has. A copy served to readers wants to be
//! small. Those pull in opposite directions, and the usual resolution is to
//! quietly recompress the master, which loses the master.
//!
//! So this writes a second file instead, and prints what the second file cost:
//! the sizes, and the difference from the master in code values, worst single
//! channel and mean. A caption can then say how far off the served copy is
//! rather than asserting that nobody would notice.
//!
//! The measurement is made by decoding what was just encoded, so it is the
//! error a reader's browser will actually see, not an estimate from the
//! quantisation tables.
//!
//! ## Usage
//!
//! ```text
//! cargo run --release --bin web_copy -- \
//!     --input master.jpg --out served.jpg --quality 90
//! ```

use std::path::PathBuf;

use clap::Parser;

use fovea::image::{Image, ImageView};
use fovea::pixel::Srgb8;

use fovea_io::DecodedImage;
use fovea_io::jpeg::{self, JpegEncodeOptions};
use fovea_io::png::PngImage;

#[derive(Parser)]
#[command(about = "Write a lighter JPEG copy of a master and measure what it cost")]
struct Args {
    /// The master, left untouched (JPEG or PNG, 8-bit sRGB colour)
    #[arg(short, long)]
    input: PathBuf,
    /// Where the lighter copy is written
    #[arg(short, long)]
    out: PathBuf,
    /// JPEG quality for the copy (1 to 100)
    #[arg(short, long, default_value_t = 90)]
    quality: u8,
    /// Progressive rather than baseline. Received wisdom says this is smaller
    /// at identical quality, since it only reorders the scans. Measured on
    /// this encoder it is larger, by 6 % on a 1024 px portrait at quality 95,
    /// so check the printed size before believing the folklore.
    #[arg(short, long)]
    progressive: bool,
}

fn main() {
    let args = Args::parse();

    let bytes = std::fs::read(&args.input)
        .unwrap_or_else(|e| fail(&format!("cannot read {}: {e}", args.input.display())));
    let master = decode(&bytes, &args.input);
    println!(
        "master: {} ({}x{}, {} bytes)",
        args.input.display(),
        master.width(),
        master.height(),
        bytes.len()
    );

    // Built from the default and then adjusted: the options struct is
    // non-exhaustive, so a struct expression will not compile against it.
    let mut options = JpegEncodeOptions::default();
    options.quality = args.quality;
    options.progressive = args.progressive;

    let encoded = jpeg::encode(&master, &options)
        .unwrap_or_else(|e| fail(&format!("JPEG encode failed: {e}")));

    // Decode it back. The point of this tool is the number below, and the only
    // honest source for it is the file that will actually be served.
    let copy = decode(&encoded, &args.out);
    if copy.width() != master.width() || copy.height() != master.height() {
        fail("the copy came back at a different size than the master");
    }

    let mut worst = 0u16;
    let mut sum = 0u64;
    for y in 0..master.height() {
        for x in 0..master.width() {
            let a = master.pixel_at(x, y);
            let b = copy.pixel_at(x, y);
            for (p, q) in [(a.r.0, b.r.0), (a.g.0, b.g.0), (a.b.0, b.b.0)] {
                let d = u16::from(p.abs_diff(q));
                if d > worst {
                    worst = d;
                }
                sum += u64::from(d);
            }
        }
    }
    let channels = (master.width() * master.height() * 3) as f64;

    println!(
        "copy:   quality {}{}, {} bytes ({:.0} % of the master)",
        args.quality,
        if args.progressive {
            ", progressive"
        } else {
            ", baseline"
        },
        encoded.len(),
        100.0 * encoded.len() as f64 / bytes.len() as f64
    );
    println!(
        "cost:   worst single channel off by {} codes, mean {:.3} codes out of 255",
        worst,
        sum as f64 / channels
    );

    if let Some(parent) = args.out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&args.out, &encoded)
        .unwrap_or_else(|e| fail(&format!("cannot write {}: {e}", args.out.display())));
    println!("wrote {}", args.out.display());
}

fn decode(bytes: &[u8], what: &std::path::Path) -> Image<Srgb8> {
    match fovea_io::load(bytes) {
        Ok(DecodedImage::Jpeg(d)) => match d.image {
            jpeg::JpegImage::Srgb8(img) => img,
            _ => fail(&format!("{}: expected 8-bit sRGB colour", what.display())),
        },
        Ok(DecodedImage::Png(d)) => match d.image {
            PngImage::Srgb8(img) => img,
            _ => fail(&format!("{}: expected 8-bit sRGB colour", what.display())),
        },
        Ok(_) => fail(&format!("{}: use JPEG or PNG", what.display())),
        Err(e) => fail(&format!("{}: decode failed: {e}", what.display())),
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("web_copy: {msg}");
    std::process::exit(1);
}
