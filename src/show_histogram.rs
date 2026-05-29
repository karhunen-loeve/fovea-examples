//! # show_histogram — display the Terrace image and a layered histogram
//!
//! Loads the Terrace sample image (8-bit grayscale JPEG) and opens two debug
//! windows:
//!
//! 1. The Terrace image itself (converted to `Srgb8` for display).
//! 2. A single histogram window with **two translucent layers**
//!    overlaid in the same plot:
//!      - linear-scale counts in blue
//!      - log-scale counts in red
//!
//!    Same data, two transforms — the overlay makes it easy to see
//!    where the dominant bins are while still reading the long tail.
//!
//! This is also the pattern you would use for an RGB image's three
//! channels: build one [`HistogramLayer`] per channel and hand them
//! to [`debug_histogram_layers`] in one call.
//!
//! ```text
//! cargo run --bin show_histogram
//! ```
//!
//! Press any key in any window (or close them) to exit.

use std::fs;

use fovea::analyze::histogram::{Histogram, NaturalBins, histogram};
use fovea::image::{Image, ImageView};
use fovea::pixel::{Srgb8, Srgba8};
use fovea::transform::{Broadcast, ConvertPixelExt, SrgbGamma, convert_image};
use fovea_io::jpeg::{self, JpegImage};

use fovea_display::{DebugDisplay, HistogramLayer, HistogramPlotOptions, Identity};

fn main() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/data/Terrace.jpg");
    let bytes = fs::read(path).unwrap_or_else(|e| {
        eprintln!("Failed to read {path}: {e}");
        eprintln!("Run this example from the fovea-examples repository root.");
        std::process::exit(1);
    });

    let decoded = jpeg::decode(&bytes).expect("failed to decode JPEG");

    // Terrace is an 8-bit grayscale JPEG (SrgbMono8). Its single channel
    // is `Saturating<u8>`, which `NaturalBins` supports natively — no
    // quantisation, one bin per intensity value.
    let JpegImage::SrgbMono8(mono) = decoded.image else {
        panic!("expected SrgbMono8, got a different pixel format");
    };

    println!(
        "Computing histogram of Terrace ({}×{}, SrgbMono8)…",
        mono.width(),
        mono.height(),
    );

    // ── Histogram ─────────────────────────────────────────────────────────────
    // Single-channel pixel → request a single `Histogram<_, _>` output.
    let h: Histogram<NaturalBins, _> =
        histogram(&mono, &NaturalBins).expect("histogram() rejected NaturalBins");

    println!(
        "  bins: {}   total: {}   nan: {}   under: {}   over: {}",
        h.bins().len(),
        h.total_count,
        h.nan_count,
        h.underflow_count,
        h.overflow_count,
    );

    // ── Display copy of Terrace ──────────────────────────────────────────────
    // SrgbMono8 → MonoF32 (linearise) → RgbF32 (broadcast) → Srgb8 (re-encode)
    // gives us a colour-space-correct displayable image for `Identity`.
    let srgb: Image<Srgb8> = convert_image(
        &mono,
        SrgbGamma
            .then::<fovea::pixel::MonoF32, _>(Broadcast)
            .then::<fovea::pixel::RgbF32, _>(SrgbGamma),
    );

    // ── Combined-histogram plot: linear + log in one window ──────────────────
    //
    // `log_scale` is a plot-level switch shared by every layer, so to
    // overlay linear and log views in the *same* plot we pre-transform
    // the log layer's bins ourselves and keep `log_scale = false`. The
    // `* 1000.0` factor just keeps the log heights in a comfortable
    // integer range relative to the raw counts so both layers are
    // legible against the shared y-scale.
    let log_bins: Vec<u64> = h
        .bins()
        .iter()
        .map(|&c| ((1.0 + c as f64).ln() * 1000.0).round() as u64)
        .collect();

    let plot_opts = HistogramPlotOptions {
        width: 768,
        height: 320,
        log_scale: false,
        background: Srgba8::new(32, 32, 32, 0),
        ..HistogramPlotOptions::default()
    };

    // Own the bin slices so the `move` closure below can borrow them.
    let layers_owned = (h.bins().to_vec(), log_bins);

    println!("Press any key in a window to exit.");

    DebugDisplay::run(move |ctx| {
        ctx.show("1 — Terrace", &srgb, Identity);

        let layers = [
            HistogramLayer::new(&layers_owned.1, Srgba8::new(255, 100, 100, 150)),
            HistogramLayer::new(&layers_owned.0, Srgba8::new(100, 100, 255, 150)),
        ];
        ctx.show_histogram_layers(
            "2 — Histogram (linear: blue, log: red)",
            &layers,
            &plot_opts,
        );

        let _ = ctx.wait_key();
    });
}
