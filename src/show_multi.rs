//! # show_multi — display multiple windows with DebugDisplay::run()
//!
//! Demonstrates how to open several debug windows simultaneously using
//! `DebugDisplay::run()` and `ctx.show()` with different titles. Each
//! window shows a different solid colour.
//!
//! ```text
//! cargo run --bin show_multi
//! ```
//!
//! Press any key or close all windows to exit.

use fovea::image::Image;
use fovea::pixel::Srgba8;

use fovea_display::{DebugDisplay, Identity};

fn main() {
    let width = 320;
    let height = 240;

    // Create four solid-colour images.
    let red = Image::fill(width, height, Srgba8::new(220, 50, 50, 255));
    let green = Image::fill(width, height, Srgba8::new(50, 180, 50, 255));
    let blue = Image::fill(width, height, Srgba8::new(50, 80, 220, 255));
    let yellow = Image::fill(width, height, Srgba8::new(230, 210, 40, 255));

    println!("Opening 4 windows ({width}×{height} each) — press any key to close all.");

    DebugDisplay::run(move |ctx| {
        ctx.show("Red", &red, Identity);
        ctx.show("Green", &green, Identity);
        ctx.show("Blue", &blue, Identity);
        ctx.show("Yellow", &yellow, Identity);

        // Block until the user presses any key or closes all windows.
        match ctx.wait_key() {
            Some(key) => println!("Key pressed: {key:?}"),
            None => println!("All windows closed"),
        }
    });
}
