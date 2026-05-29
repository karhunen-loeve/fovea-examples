//! # tile_flip_rotate — per-tile axis-aligned geometric transforms
//!
//! Loads the Terrace sample image (decoded as `SrgbMono8`), crops a square
//! region, splits it into an `N × N` grid of square tiles, and applies a different
//! axis-aligned transform from [`fovea::transform`] to each tile, writes
//! the result into the matching tile of an output
//! image, and finally opens two debug windows showing the original and
//! the transformed result side by side.
//!
//! ```text
//! cargo run --bin tile_flip_rotate
//! ```
//!
//! Press any key (or close both windows) to exit.
//!
//! ## Why square tiles
//!
//! The 90°/270° rotations and the transpose swap the *tile* width and
//! height. To keep every transformed tile drop-in compatible with its
//! original cell, this example uses square tiles. The tile size is
//! chosen so that the image divides evenly (256 / 4 = 64).
//!
//! ## Why `_into` rather than the allocating variant
//!
//! Each tile of the output image is a strided
//! [`fovea::image::ImageRefMut`] obtained via
//! [`fovea::image::SubViewMut::roi_mut`]. Writing directly through
//! that mutable ROI keeps the operation zero-allocation per tile and
//! exercises the [`fovea::image::RasterImageMut`]-bound output path of
//! the transform functions.

use std::fs;

use fovea::Rectangle;
use fovea::image::{Image, ImageView, RasterImage, RasterImageMut, SubView, SubViewMut};
use fovea::pixel::SrgbMono8;
use fovea::transform::{
    flip_h_into, flip_v_into, rotate_90_into, rotate_180_into, rotate_270_into, transpose_into,
};
use fovea_io::jpeg::{self, JpegImage};

use fovea_display::{DebugDisplay, Identity};

/// Which transform to apply to a given tile.
#[derive(Copy, Clone, Debug)]
enum TileOp {
    Identity,
    FlipH,
    FlipV,
    Rotate90,
    Rotate180,
    Rotate270,
    Transpose,
}

impl TileOp {
    /// Applies `self` to `src` writing into `dst`.
    ///
    /// Both views must be the same square size — this is the caller's
    /// responsibility, satisfied here by construction (we only feed
    /// `TileOp` square tiles of identical dimensions).
    fn apply<I, O>(self, src: &I, dst: &mut O)
    where
        I: fovea::image::RasterImage<Pixel = SrgbMono8>,
        O: fovea::image::RasterImageMut<Pixel = SrgbMono8>,
    {
        match self {
            // Identity: a plain row-by-row copy keeps the path free of any
            // dispatching surprises while exercising the same trait bounds
            // as the other arms.
            TileOp::Identity => {
                for y in 0..src.height() {
                    dst.row_mut(y).copy_from_slice(src.row(y));
                }
            }
            TileOp::FlipH => flip_h_into(src, dst),
            TileOp::FlipV => flip_v_into(src, dst),
            TileOp::Rotate90 => rotate_90_into(src, dst),
            TileOp::Rotate180 => rotate_180_into(src, dst),
            TileOp::Rotate270 => rotate_270_into(src, dst),
            TileOp::Transpose => transpose_into(src, dst),
        }
    }

    fn short_label(self) -> &'static str {
        match self {
            TileOp::Identity => " id  ",
            TileOp::FlipH => "flipH",
            TileOp::FlipV => "flipV",
            TileOp::Rotate90 => "rot90",
            TileOp::Rotate180 => "rot180",
            TileOp::Rotate270 => "rot270",
            TileOp::Transpose => "trans",
        }
    }
}

/// Returns the transform for grid cell `(col, row)`.
///
/// The pattern is hand-picked to cover every variant at least once and
/// to look visually distinct from a plain rotation of the whole image.
fn pattern(col: usize, row: usize) -> TileOp {
    const TABLE: [[TileOp; 4]; 4] = [
        [
            TileOp::Identity,
            TileOp::FlipH,
            TileOp::FlipV,
            TileOp::Rotate180,
        ],
        [
            TileOp::Rotate90,
            TileOp::Rotate270,
            TileOp::Transpose,
            TileOp::FlipH,
        ],
        [
            TileOp::FlipV,
            TileOp::Transpose,
            TileOp::Rotate90,
            TileOp::Rotate270,
        ],
        [
            TileOp::Rotate180,
            TileOp::Identity,
            TileOp::FlipH,
            TileOp::FlipV,
        ],
    ];
    TABLE[row][col]
}

fn main() {
    // ── 1. Load Terrace and copy a square ROI ────────────────────────────
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/data/Terrace.jpg");
    let bytes = fs::read(input).expect("failed to read Terrace.jpg");
    let decoded = jpeg::decode(&bytes).expect("failed to decode JPEG");
    let JpegImage::SrgbMono8(src_full) = decoded.image else {
        panic!("expected SrgbMono8, got a different pixel format");
    };

    let side = src_full.width().min(src_full.height());
    let roi = src_full
        .roi(Rectangle::new((0, 0), (side, side)))
        .expect("square ROI in bounds");
    let mut src = Image::<SrgbMono8>::zero(side, side);
    for y in 0..side {
        src.row_mut(y).copy_from_slice(roi.row(y));
    }

    let (w, h) = (src.width(), src.height());
    let grid: usize = 4;
    assert!(
        w % grid == 0,
        "image side ({w}) must divide evenly by grid size ({grid})"
    );
    let tile = w / grid;
    println!("Terrace crop: {w}×{h}, splitting into a {grid}×{grid} grid of {tile}×{tile} tiles");

    // ── 2. Apply per-tile transforms ─────────────────────────────────────
    // The output starts as a black image of the same size; each tile of
    // the output is written through a strided `roi_mut` view. We read
    // from the source image's `roi` (immutable) for the input.
    let mut dst = Image::<SrgbMono8>::zero(w, h);
    for row in 0..grid {
        for col in 0..grid {
            let op = pattern(col, row);
            let rect = Rectangle::new((col * tile, row * tile), (tile, tile));
            let src_tile = src.roi(rect).expect("source tile in bounds");
            let mut dst_tile = dst.roi_mut(rect).expect("dest tile in bounds");
            op.apply(&src_tile, &mut dst_tile);
            print!(" {}", op.short_label());
        }
        println!();
    }

    // ── 3. Display original and result side by side ──────────────────────
    println!(
        "\nopening 'Terrace crop (original)' and 'Terrace crop (per-tile)' — press any key to close."
    );
    DebugDisplay::run(move |ctx| {
        ctx.show("Terrace crop (original)", &src, Identity);
        ctx.show("Terrace crop (per-tile flip & rotate)", &dst, Identity);
        match ctx.wait_key() {
            Some(k) => println!("key pressed: {k:?}"),
            None => println!("all windows closed"),
        }
    });
}
