//! # show_mono16 — display a synthetic Mono16 gradient with AutoContrast
//!
//! Creates a 640×480 Mono16 image with a horizontal gradient from 0 to
//! 65535, scans it with `AutoContrast`, and displays the result in a
//! debug window.
//!
//! ```text
//! cargo run --bin show_mono16
//! ```
//!
//! Press any key or close the window to exit.

use fovea::image::Image;
use fovea::pixel::Mono16;

use fovea_display::{AutoContrast, show};

fn main() {
    let width = 640;
    let height = 480;

    // Create a horizontal gradient: each column maps linearly from 0 to 65535.
    let img = Image::generate(width, height, |x, _y| {
        let value = (x as u64 * 65535 / (width as u64 - 1)) as u16;
        Mono16::new(value)
    });

    // Scan the image to determine the display range, extracting the u16
    // intensity via the `.value()` accessor.
    let strategy = AutoContrast::scan_with(&img, |p| p.value() as f64);

    println!("Displaying {width}×{height} Mono16 gradient — press any key to close");

    show("show_mono16", &img, strategy);
}
