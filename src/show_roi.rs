//! # show_roi — display a sub-region (ROI) of an image
//!
//! Demonstrates that `show()` accepts any `ImageView`, not just `Image`.
//! Creates a colourful 640×480 sRGB image, extracts a 200×150 ROI from
//! the centre, and displays both the full image and the ROI.
//!
//! Because `show()` converts to a framebuffer on the calling thread before
//! enters the event loop, there are no lifetime issues with borrowed ROIs.
//!
//! ```text
//! cargo run --bin show_roi
//! ```
//!
//! Press any key or close the window to exit (shown sequentially).

use fovea::image::{Image, ImageView, SubView};
use fovea::pixel::Srgba8;
use fovea::{Rectangle, Size};

use fovea_display::{Identity, show};

fn main() {
    let width = 640;
    let height = 480;

    // Create a colourful test pattern: red increases left→right,
    // green increases top→bottom, blue is fixed at 128.
    let img = Image::generate(width, height, |x, y| {
        let r = (x * 255 / (width - 1)) as u8;
        let g = (y * 255 / (height - 1)) as u8;
        Srgba8::new(r, g, 128, 255)
    });

    // Extract a 200×150 ROI from the centre of the image.
    let roi_w = 200;
    let roi_h = 150;
    let roi_x = (width - roi_w) / 2;
    let roi_y = (height - roi_h) / 2;
    let roi = img
        .roi(Rectangle::new((roi_x, roi_y), (roi_w, roi_h)))
        .unwrap();

    println!(
        "Full image: {}×{}, ROI: {}×{} at ({}, {})",
        width,
        height,
        roi.width(),
        roi.height(),
        roi_x,
        roi_y,
    );

    // Display the full image first. show() converts the image to a
    // framebuffer on the calling thread, so the borrow is released
    // before the event loop starts. Press any key to proceed.
    //println!("Showing full image — press any key to continue...");
    //show("Full Image", &img, Identity);

    // Now display just the ROI. This demonstrates that show() works
    // with any ImageView — the ROI is an ImageRef, not an Image.
    //println!("Showing ROI — press any key to exit...");
    //show("ROI (centre)", &roi, Identity);

    // Now split the image into tiles and show each one
    let tile_w = width / 2;
    let tile_h = height / 2;
    let cols = width.div_ceil(tile_w);
    for (i, tile) in img.tiles(Size::new(tile_w, tile_h)).enumerate() {
        let col = i % cols;
        let row = i / cols;
        println!("Showing tile at ({}, {})", col * tile_w, row * tile_h,);
        show("Tile", &tile, Identity);
    }
}
