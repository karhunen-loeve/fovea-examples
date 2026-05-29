//! # show_histogram_consumers — Otsu threshold + histogram equalization
//!
//! Loads the Terrace sample image (8-bit grayscale JPEG) and demonstrates the
//! two direct consumers of the per-channel histogram engine:
//!
//! - [`otsu_binary_mask`] picks an automatic intensity threshold and
//!   produces a [`BinaryImage`].
//! - [`equalize_image`] flattens the intensity distribution for a
//!   higher-contrast result.
//!
//! Four debug windows open simultaneously:
//!
//! 1. **Original** Terrace luminance.
//! 2. **Otsu binary mask** — window title carries the chosen threshold.
//! 3. **Equalised image**.
//! 4. **Histogram overlay** — input distribution (blue) vs. equalised
//!    distribution (green). The equalised layer should look noticeably
//!    flatter, which is the whole point of histogram equalization.
//!
//! ```text
//! cargo run --bin show_histogram_consumers
//! ```
//!
//! Press any key in any window (or close them) to exit.
//!
//! ## Design notes
//!
//! The Terrace JPEG decodes as `SrgbMono8`. Both Otsu and equalisation
//! operate purely on histogram bins, so the gamma encoding of the
//! source is immaterial — we re-tag the bytes as raw [`Mono8`] for the
//! analysis pipeline. [`otsu_binary_mask`] requires the pixel type to
//! implement `From<u8>` (so it can lift the threshold back into the
//! pixel space), which [`Mono8`] does and `SrgbMono8` does not; the
//! retagging keeps the example aligned with the API's actual bounds
//! rather than papering over them.

use std::fs;

use fovea::analyze::histogram::{
    Histogram, NaturalBins, equalize_image, histogram, otsu_binary_mask,
};
use fovea::image::{BinaryImage, Image, ImageView};
use fovea::pixel::{Mono8, SrgbMono8, Srgba8};
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
    let JpegImage::SrgbMono8(srgb_mono) = decoded.image else {
        panic!("expected SrgbMono8 Terrace image");
    };
    let (w, h) = (srgb_mono.width(), srgb_mono.height());
    println!("Terrace {w}×{h} (SrgbMono8)");

    // Treat the 8-bit intensities as raw `Mono8`. The histogram
    // consumers care about bin counts, not colour space.
    let mono: Image<Mono8> = Image::generate(w, h, |x, y| Mono8::new(srgb_mono.pixel_at(x, y).0.0));

    // ── Otsu ────────────────────────────────────────────────────────────────
    let (threshold, mask): (u8, BinaryImage) =
        otsu_binary_mask(&mono).expect("otsu_binary_mask on NaturalBins is infallible here");
    println!("Otsu threshold: {threshold}");

    // ── Equalisation ────────────────────────────────────────────────────────
    let equalised: Image<Mono8> = equalize_image(&mono).expect("equalize_image failed");

    // ── Histograms before/after ─────────────────────────────────────────────
    let h_in: Histogram<NaturalBins, _> = histogram(&mono, &NaturalBins).expect("input histogram");
    let h_eq: Histogram<NaturalBins, _> =
        histogram(&equalised, &NaturalBins).expect("equalised histogram");
    println!(
        "input histogram   — total={}, peak={}",
        h_in.total_count,
        h_in.bins().iter().copied().max().unwrap_or(0),
    );
    println!(
        "equalised histogram — total={}, peak={}",
        h_eq.total_count,
        h_eq.bins().iter().copied().max().unwrap_or(0),
    );

    // ── Promote to a displayable type (SrgbMono8 + Identity) ────────────────
    // Terrace's bytes are already sRGB-encoded greys, so we can re-tag
    // the equalised result (and the binary mask) as SrgbMono8
    // for direct display without double-gamma.
    let mask_display: Image<SrgbMono8> = Image::generate(w, h, |x, y| {
        SrgbMono8::new(if mask.pixel_at(x, y) { 255 } else { 0 })
    });
    let equalised_display: Image<SrgbMono8> = Image::generate(w, h, |x, y| {
        SrgbMono8::new(equalised.pixel_at(x, y).value())
    });
    let original_display: Image<SrgbMono8> = srgb_mono;

    // ── Histogram overlay options ───────────────────────────────────────────
    let plot_opts = HistogramPlotOptions {
        width: 768,
        height: 320,
        log_scale: false,
        background: Srgba8::new(32, 32, 32, 0),
        ..HistogramPlotOptions::default()
    };

    // Own the bin slices so the `move` closure can borrow them safely.
    let bins_in: Vec<u64> = h_in.bins().to_vec();
    let bins_eq: Vec<u64> = h_eq.bins().to_vec();

    println!("Press any key in a window to exit.");

    DebugDisplay::run(move |ctx| {
        ctx.show("1 — Terrace (original)", &original_display, Identity);
        ctx.show(
            &format!("2 — Otsu mask (threshold = {threshold})"),
            &mask_display,
            Identity,
        );
        ctx.show("3 — Equalised", &equalised_display, Identity);

        let layers = [
            HistogramLayer::new(&bins_in, Srgba8::new(100, 100, 255, 160)),
            HistogramLayer::new(&bins_eq, Srgba8::new(100, 220, 100, 160)),
        ];
        ctx.show_histogram_layers(
            "4 — Histogram (input: blue, equalised: green)",
            &layers,
            &plot_opts,
        );

        let _ = ctx.wait_key();
    });
}
