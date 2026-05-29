//! # show_linear — display a linear RgbF32 image with LinearToDisplay
//!
//! Creates a 640×480 linear-light `RgbF32` image with smooth colour
//! gradients and displays it using the `LinearToDisplay` strategy, which
//! applies sRGB gamma encoding before display.
//!
//! This demonstrates why `LinearToDisplay` is necessary for linear data:
//! without sRGB encoding, the image would appear too dark on a standard
//! monitor.
//!
//! ```text
//! cargo run --bin show_linear
//! ```
//!
//! Press any key or close the window to exit.

use fovea::image::Image;
use fovea::pixel::RgbF32;

use fovea_display::{LinearToDisplay, show};

fn main() {
    let width = 640;
    let height = 480;

    // Create a linear-light gradient image:
    //   - Red increases left → right
    //   - Green increases top → bottom
    //   - Blue is the inverse of red (right → left)
    //
    // All values are in linear light [0.0, 1.0]. The `LinearToDisplay`
    // strategy will apply sRGB gamma encoding so the gradient looks
    // perceptually uniform on screen.
    let img: Image<RgbF32> = Image::generate(width, height, |x, y| {
        let r = x as f32 / (width - 1) as f32;
        let g = y as f32 / (height - 1) as f32;
        let b = 1.0 - r;
        RgbF32 { r, g, b }
    });

    println!(
        "Displaying {width}×{height} linear RgbF32 image with sRGB gamma encoding — press any key to close"
    );

    show("show_linear (LinearToDisplay)", &img, LinearToDisplay);
}
