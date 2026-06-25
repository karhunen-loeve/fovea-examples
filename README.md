# fovea examples

[![CI](https://github.com/karhunen-loeve/fovea-examples/actions/workflows/ci.yml/badge.svg)](https://github.com/karhunen-loeve/fovea-examples/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

End-to-end programs for the fovea crates. This repository is not published to crates.io; it is the place to see `fovea`, `fovea-io`, and `fovea-display` working together against released crate versions.

If the crate docs show the building blocks, these examples show the whole pipeline: decode typed pixels, make conversions explicit, run transforms, display or encode the result.

## Start here

| If you want to... | Run |
|---|---|
| See the smallest decode → convert → encode pipeline | `cargo run --bin simple` |
| Resize an image with linear-light bilinear interpolation | `cargo run --bin resize -- -i data/Terrace.jpg -W 800` |
| See convolution, gradient magnitude, and overlay together | `cargo run --bin edge_overlay` |
| Segment edges with a double (hysteresis) threshold | `cargo run --bin hysteresis_threshold` |
| Detect edges with the full Canny pipeline | `cargo run --bin canny` |
| Inspect display strategies | `cargo run --bin show_srgb` and `cargo run --bin show_linear` |
| See ROI display | `cargo run --bin show_roi` |

## Building

Run commands from the `fovea-examples` repository root.

```sh
cargo build            # all examples, debug
cargo build --release  # all examples, optimised
```

## Examples

| Binary          | Description |
|-----------------|-------------|
| `simple`        | Minimal getting-started example — load, convert, manipulate, save |
| `resize`        | Colour-space-aware image resizer (PNG, JPEG, BMP) |
| `edge_overlay`  | Sobel edge detection + gradient magnitude + `LinearCombine` overlay |
| `hysteresis_threshold` | Double-threshold edge segmentation (`hysteresis_threshold`) on a gradient-magnitude image |
| `canny`         | Full Canny edge detector (`analyze::edge::canny`) with every intermediate stage displayed |
| `perona_malik`  | Perona-Malik anisotropic diffusion filter CLI — PNG, JPEG, BMP       |
| `show_srgb`     | Load a JPEG and display it with `Identity` strategy |
| `show_mono16`   | Synthetic Mono16 gradient displayed with `AutoContrast` |
| `show_roi`      | Display a sub-region (ROI) to show `ImageView` generality |
| `show_multi`    | Multiple windows using `DebugDisplay::run()` |
| `show_linear`   | Linear `RgbF32` gradient with `LinearToDisplay` (sRGB gamma) |

---

## `edge_overlay`

Demonstrates the **convolution** and **pixel-wise combinator** APIs working
together in a single pipeline on the Terrace sample image:

```
Terrace.jpg (SrgbMono8)
  → SrgbGamma          → Image<f32>  (linear light, [0, 1])
  → sobel_x / sobel_y  → Image<f32>  (signed gradients)
  → Magnitude          → Image<f32>  (√(gx² + gy²), edge map)
  → LinearCombine      → Image<f32>  (85 % original + 15 % edges)
```

All five stages are displayed simultaneously in separate windows.

### Quick start

```sh
cargo run --bin edge_overlay
```

Press any key or close any window to exit.

### What it demonstrates

| Concept | API used |
|---------|----------|
| sRGB linearisation | `convert_image(&mono, SrgbGamma)` → `Image<f32>` |
| Convolution | `sobel_x` / `sobel_y` with `Clamp` border policy |
| Binary pixel combinator | `combine_images(&gx, &gy, Magnitude)` |
| Weighted blend | `combine_images(&linear, &magnitude, LinearCombine { wa: 0.7, wb: 0.3 })` |
| Multi-window display | `DebugDisplay::run` + `ctx.show` |
| Display strategies | `LinearToDisplay` (original), `AutoContrast::scan` (gradients, magnitude, overlay) |

### Naming note

`sobel_x` computes `dI/dx` (horizontal gradient) and lights up **vertical**
edges `|`.  `sobel_y` computes `dI/dy` (vertical gradient) and lights up
**horizontal** edges `―`.  The name refers to the gradient direction, not
the orientation of the visible edge in the output image.

---

## `hysteresis_threshold`

Demonstrates `analyze::threshold::hysteresis_threshold` — the **double-threshold**
segmentation that forms the final stage of a Canny edge detector — on a real
gradient-magnitude image:

```text
Terrace.jpg (SrgbMono8)
  → SrgbGamma          → Image<f32>  (linear light, [0, 1])
  → sobel_x / sobel_y  → Image<f32>  (signed gradients)
  → Magnitude          → Image<f32>  (√(gx² + gy²), edge map)
  → hysteresis_threshold(low, high)  → BinaryImage  (kept edges)
```

A weak pixel (`value >= low`) survives only if its 8-connected component
contains a strong pixel (`value >= high`). The example derives `low` / `high`
as fractions of the peak magnitude, then shows three masks built from the
**same** function to make the trade-off visible:

- **strong only** (`low == high`): clean, but strong edges fragment.
- **low only** (`low == low`): connected, but noisy.
- **hysteresis** (`low`, `high`): connected edges without the noise.

### Quick start

```sh
cargo run --bin hysteresis_threshold
```

Press any key or close any window to exit.

---

## `canny`

Demonstrates `analyze::edge::canny` — the **complete** single-scale Canny edge
detector — and, because every stage is a public function, rebuilds the same
pipeline by hand so each intermediate can be shown:

```text
Terrace.jpg (SrgbMono8)
  → SrgbGamma                    → Image<f32>  (linear light, [0, 1])
  → gaussian_blur(sigma)         → Image<f32>  (true-σ Gaussian smoothing)
  → scharr_x / scharr_y          → Image<f32>  (signed gradients)
  → gradient_magnitude / _direction → Image<f32>  (edge strength + angle)
  → non_maximum_suppression      → Image<f32>  (thinned ridge)
  → hysteresis_threshold(low, high) → BinaryImage  (linked edges)
```

`sigma` is a true Gaussian standard deviation; `low` / `high` are absolute
gradient-magnitude thresholds whose meaning is stable across `sigma` because
the blur preserves brightness. The example asserts that the hand-built mask
equals the one-call `canny(&linear, low, high, sigma)`.

### Quick start

```sh
cargo run --bin canny
```

Press any key or close any window to exit.

---

## `perona_malik`

A CLI tool that applies the classic **Perona-Malik anisotropic diffusion**
filter to an image.  It smooths homogeneous regions while sharpening or
preserving edges — the opposite of a plain Gaussian blur.

Each iteration updates every pixel using the four-neighbour (N, S, E, W)
discretisation:

```text
I(t+1) = I(t) + λ · Σ_{n ∈ {N,S,E,W}} g(‖∇I_n‖) · (I_n − I)
```

Processing is done in **linear light**: sRGB images are decoded to linear
`f32`/`RgbF32` before the diffusion loop and re-encoded with sRGB gamma
afterwards.  For colour images the conductance is derived from an RMS RGB
gradient magnitude so all three channels share a single edge map while the
κ parameter remains comparable to grayscale inputs (vector Perona-Malik).

### Quick start

```sh
# Demo defaults: 15 iterations, κ = 30, λ = 0.15, rational conductance
cargo run --bin perona_malik -- -i data/Mandrill.jpg

# Subtler, more edge-preserving settings
cargo run --bin perona_malik -- \
    -i photo.jpg -n 15 -k 20 -l 0.1 -f exp -o photo_smooth.jpg
```

### Usage

```text
perona_malik [OPTIONS] --input <FILE>

Options:
  -i, --input <FILE>      Input image file (PNG, JPEG, or BMP)
  -o, --output <PATH>     Output file or directory (optional)
  -n, --iterations <N>    Number of diffusion iterations [default: 15]
  -k, --kappa <FLOAT>     Diffusion coefficient κ, 0–255 scale [default: 30.0]
  -l, --lambda <FLOAT>    Time-step λ per iteration, must be ≤ 0.25 [default: 0.15]
  -f, --function <FN>     Conductance function: "exp" or "rat" [default: rat]
  -h, --help              Print help
  -V, --version           Print version
```

### Parameters

| Parameter       | Meaning                                                                       |
|-----------------|-------------------------------------------------------------------------------|
| `-n` iterations | More iterations → more smoothing (and slower runtime)                         |
| `-k` κ (kappa)  | Edge threshold (0–255 scale). Low κ preserves faint edges; high κ smooths more|
| `-l` λ (lambda) | Step size per iteration. Must be ≤ 0.25 for numerical stability               |
| `-f` function   | `exp` (PM option 1, Gaussian) or `rat` (PM option 2, Lorentzian)              |

### Conductance functions

| Flag  | Formula                    | Characteristic                                |
|-------|----------------------------|-----------------------------------------------|
| `exp` | g(d) = exp(−(d/κ)²)        | Sharp fall-off at κ; favours high-contrast edges |
| `rat` | g(d) = 1 / (1 + (d/κ)²)   | Heavy-tailed; favours wide smooth regions     |

### Output rules

| `--output` value              | Behaviour                                               |
|-------------------------------|---------------------------------------------------------|
| *(omitted)*                   | Written next to the input: `photo_pm.jpg`               |
| A directory (or no extension) | File placed there with the suffix: `out/photo_pm.jpg`   |
| A file with image extension   | That exact path; format derived from the extension      |

### Supported pixel formats

Only `SrgbMono8` and `Srgb8` inputs are accepted — the most common 8-bit sRGB
formats produced by PNG, JPEG, and BMP decoders.  Other pixel types (16-bit,
alpha, indexed) are rejected with a descriptive error message; convert to
`Srgb8` or `SrgbMono8` first (e.g. with the `resize` example).

| Input format | `SrgbMono8` | `Srgb8` |
|--------------|:-----------:|:-------:|
| PNG          | ✓           | ✓       |
| JPEG         | ✓           | ✓       |
| BMP          | —           | ✓       |

### What it demonstrates

| Concept | API used |
|---------|----------|
| Iterative non-linear neighbourhood filter | `map_neighborhood_fn` + `cross_3x3` mask |
| Data-dependent conductance per pixel | Closure captures `kappa_sq` and `Conductance` |
| Linear-light processing loop | `convert_image(&img, SrgbGamma)` before and after |
| Vector PM for colour images | RMS RGB gradient magnitude in the conductance closure |
| Multi-format I/O | `fovea_io::load` + per-codec encode (`png`, `jpeg`, `bmp`) |

---

## `simple`

A minimal first-contact example that shows the core fovea workflow in a
single `main` function: **decode → convert → manipulate → encode**.

It loads the Terrace sample image (an 8-bit sRGB grayscale JPEG), converts it
to linear RGB, boosts the red channel to give it a colour tint, converts back
to sRGB, and writes the result as a new PNG.

### Quick start

```sh
cargo run --bin simple
```

This reads `data/Terrace.jpg` and writes
`data/terrace_tinted.png`.

### What it demonstrates

| Concept | API used |
|---------|----------|
| JPEG decoding | `jpeg::decode` → `JpegImage::SrgbMono8` |
| Colour-space conversion | `SrgbGamma` (sRGB ↔ linear) |
| Channel broadcasting | `Broadcast` (mono → RGB) |
| Strategy chaining | `SrgbGamma.then::<f32, _>(Broadcast)` |
| Pixel-level manipulation | Iterating `as_mut_slice()` to modify `RgbF32.r` |
| PNG encoding | `png::encode` with `PngEncodeOptions::default()` |

---

## `resize`

A small CLI tool that resizes images while **preserving the pixel format and
colour space**.  sRGB-encoded pixels are linearised before bilinear
interpolation and re-encoded afterwards so that the resize is perceptually
correct.

### Quick start

```sh
# Scale a PNG to half size
cargo run --bin resize -- -i photo.png -s 0.5

# Resize to a specific width, keeping the aspect ratio
cargo run --bin resize -- -i photo.jpg -W 800

# Resize to exact dimensions and write to a specific file
cargo run --bin resize -- -i photo.png -W 320 -H 240 -o thumb.png

# Resize and save into a different directory
cargo run --bin resize -- -i photo.bmp -s 2.0 -o output/
```

### Usage

```text
resize [OPTIONS] --input <FILE>

Options:
  -i, --input <FILE>            Input image file
  -o, --output <PATH>           Output file or directory (optional)
  -W, --width <PX>              Target width in pixels
  -H, --height <PX>             Target height in pixels
  -s, --scaling-factor <FLOAT>  Uniform scaling factor (e.g. 0.5, 2.0)
  -h, --help                    Print help
  -V, --version                 Print version
```

### Size rules

| Arguments                         | Behaviour                                            |
|-----------------------------------|------------------------------------------------------|
| `--width` **and** `--height`      | Exact target size                                    |
| Only `--width`                    | Height computed to preserve aspect ratio             |
| Only `--height`                   | Width computed to preserve aspect ratio              |
| `--scaling-factor`                | Both dimensions scaled uniformly                     |
| `--width`/`--height` **+** `-s`   | **Error** — mutually exclusive                       |
| *(none)*                          | **Error** — at least one sizing parameter is required |

### Output rules

| `--output` value                  | Behaviour                                                      |
|-----------------------------------|----------------------------------------------------------------|
| *(omitted)*                       | Written next to the input: `photo_320x240.png`                 |
| A directory (or no extension)     | File placed there with the same suffix: `out/photo_320x240.png`|
| A file name with image extension  | That exact path; output format derived from the extension       |

When the output format differs from the input format, the tool checks
whether the pixel type is encodable in the target format.  Incompatible
combinations are rejected with a descriptive error message.  For example:

- **Alpha → JPEG**: *"JPEG does not support alpha channels."*
- **Linear → JPEG**: *"JPEG requires sRGB-encoded data."*
- **16-bit → BMP**: *"BMP only supports 8-bit depth."*

### Colour-space handling

The resize algorithm depends on the pixel type detected during decoding:

| Pixel category | Examples                           | Resize strategy                                         |
|----------------|------------------------------------|---------------------------------------------------------|
| **sRGB**       | `Srgb8`, `Srgba8`, `SrgbMono8`, … | Decode to linear float → bilinear resize → re-encode sRGB |
| **Linear**     | `Rgb8`, `Rgba16`, `Mono8`, …       | Bilinear resize directly (already linear)               |
| **Indexed**    | `Indexed8`                         | Nearest-neighbour resize (palette preserved)            |

This ensures that sRGB images are resized in linear light, avoiding the
brightness shifts that naïve interpolation in gamma-encoded space produces.

### Supported formats (resize)

| Format | Decode | Encode | Pixel types                                                  |
|--------|--------|--------|--------------------------------------------------------------|
| PNG    | ✓      | ✓      | All 17 variants (sRGB, linear, 8/16-bit, mono, colour, alpha, indexed) |
| JPEG   | ✓      | ✓      | `Srgb8`, `SrgbMono8` (8-bit sRGB only)                      |
| BMP    | ✓      | ✓      | `Srgb8`, `Srgba8`, `Indexed8`                               |

---

## Display examples

These examples demonstrate `fovea-display` with the `debug-window` feature.
Each one opens a window showing an image — press any key or close the window
to exit.

### `show_srgb`

Loads the Terrace sample image (grayscale JPEG), converts it to `Srgb8`,
and displays it using `Identity`.

```sh
cargo run --bin show_srgb
```

### `show_mono16`

Creates a synthetic 640×480 `Mono16` horizontal gradient (0–65535) and
displays it with `AutoContrast::scan_with()`, which maps the full range
to visible grey levels.

```sh
cargo run --bin show_mono16
```

### `show_roi`

Creates a colourful test pattern, extracts a 200×150 ROI from the centre,
and displays both the full image and the ROI sequentially. Demonstrates
that `show()` works with any `ImageView`, not just `Image`.

```sh
cargo run --bin show_roi
```

### `show_multi`

Opens four windows simultaneously (Red, Green, Blue, Yellow) using
`DebugDisplay::run()` and `ctx.show()` with different titles.

```sh
cargo run --bin show_multi
```

### `show_linear`

Creates a 640×480 linear-light `RgbF32` gradient and displays it with
`LinearToDisplay`, which applies sRGB gamma encoding. Without the gamma
encoding the image would appear too dark on a standard monitor.

```sh
cargo run --bin show_linear
```

## License

Licensed under the [MIT License](LICENSE).
