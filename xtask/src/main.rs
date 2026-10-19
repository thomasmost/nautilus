//! Workspace helper tasks, run as `cargo xtask <task>` (alias in
//! `.cargo/config.toml`).
//!
//! `icons`: rasterize `assets/logo.svg` into `assets/icons/`:
//! `nautilus-<size>.png` for each size in `PNG_SIZES`, `nautilus.png`
//! (1024 px), `nautilus.icns` (macOS), and `nautilus.ico` (Windows). Output is
//! deterministic for a given `Cargo.lock`, so the generated files are
//! committed and CI checks they are up to date.
//!

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use resvg::tiny_skia;
use resvg::usvg;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The size rendered from the SVG; every other size is downsampled from it.
const LARGEST_SIZE: u32 = 1024;
const PNG_SIZES: [u32; 7] = [16, 32, 64, 128, 256, 512, LARGEST_SIZE];
const ICO_SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];
/// (pixel size, icns slot): the set Apple's `iconutil` produces for a
/// standard `.iconset`. The `_2x` slots are the Retina versions.
const ICNS_TYPES: [(u32, icns::IconType); 10] = [
    (16, icns::IconType::RGBA32_16x16),
    (32, icns::IconType::RGBA32_16x16_2x),
    (32, icns::IconType::RGBA32_32x32),
    (64, icns::IconType::RGBA32_32x32_2x),
    (128, icns::IconType::RGBA32_128x128),
    (256, icns::IconType::RGBA32_128x128_2x),
    (256, icns::IconType::RGBA32_256x256),
    (512, icns::IconType::RGBA32_256x256_2x),
    (512, icns::IconType::RGBA32_512x512),
    (1024, icns::IconType::RGBA32_512x512_2x),
];

fn main() {
    let task = std::env::args().nth(1);
    let result = match task.as_deref() {
        Some("icons") => icons(&project_root()),
        _ => {
            eprintln!("usage: cargo xtask icons");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level below the workspace root")
        .to_path_buf()
}

fn icons(root: &Path) -> Result<()> {
    let assets = root.join("assets");
    let svg_path = assets.join("logo.svg");
    let svg_data =
        std::fs::read(&svg_path).map_err(|e| format!("reading {}: {e}", svg_path.display()))?;
    // The logo is pure vector: no fonts, no raster artwork.
    let tree = usvg::Tree::from_data(&svg_data, &usvg::Options::default())?;

    let out = root.join("assets").join("icons");
    std::fs::create_dir_all(&out)?;

    // Render once at the largest size and area-average the rest down from
    // it, so every size comes from the same pixels. Sizes that do not divide
    // 1024 (24, 48) weight the partly covered source pixels.
    let largest = render(&tree, LARGEST_SIZE)?;
    if !has_dark_interior(&largest) {
        return Err("the logo did not render: assets/logo.svg draws nothing dark".into());
    }
    let mut pngs: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
    for size in PNG_SIZES.iter().chain(ICO_SIZES.iter()).copied() {
        if let std::collections::btree_map::Entry::Vacant(entry) = pngs.entry(size) {
            entry.insert(downsample(&largest, size)?.encode_png()?);
        }
    }
    for size in PNG_SIZES {
        std::fs::write(out.join(format!("nautilus-{size}.png")), &pngs[&size])?;
    }
    std::fs::write(out.join("nautilus.png"), &pngs[&1024])?;

    let mut family = icns::IconFamily::new();
    for (size, icon_type) in ICNS_TYPES {
        let image = icns::Image::read_png(Cursor::new(&pngs[&size]))?;
        family.add_icon_with_type(&image, icon_type)?;
    }
    let mut icns_bytes = Vec::new();
    family.write(&mut icns_bytes)?;
    std::fs::write(out.join("nautilus.icns"), icns_bytes)?;

    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in ICO_SIZES {
        let image = ico::IconImage::read_png(Cursor::new(&pngs[&size]))?;
        dir.add_entry(ico::IconDirEntry::encode_as_png(&image)?);
    }
    let mut ico_bytes = Vec::new();
    dir.write(&mut ico_bytes)?;
    std::fs::write(out.join("nautilus.ico"), ico_bytes)?;

    println!("wrote icons to {}", out.display());
    Ok(())
}

fn render(tree: &usvg::Tree, size: u32) -> Result<tiny_skia::Pixmap> {
    let mut pixmap = tiny_skia::Pixmap::new(size, size).ok_or("icon size must be non-zero")?;
    let scale_x = size as f32 / tree.size().width();
    let scale_y = size as f32 / tree.size().height();
    resvg::render(
        tree,
        tiny_skia::Transform::from_scale(scale_x, scale_y),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}

/// Shrinks a square pixmap to `size` by area averaging: each output pixel
/// is the mean of the source pixels under it, weighted by how much of each
/// source pixel its footprint covers. Sizes that do not divide the source
/// (24, 48) work the same way. The pixels are premultiplied RGBA, so a
/// plain weighted mean is correct.
fn downsample(source: &tiny_skia::Pixmap, size: u32) -> Result<tiny_skia::Pixmap> {
    if source.width() != source.height() {
        return Err(format!(
            "downsample needs a square source, got {}x{}",
            source.width(),
            source.height()
        )
        .into());
    }
    let src = source.width() as usize;
    let n = size as usize;
    if src == n {
        return Ok(source.clone());
    }
    if src < n {
        return Err(format!("cannot downsample {src} px to {n} px").into());
    }
    let ratio = src as f64 / n as f64;
    let data = source.data();
    let mut out = tiny_skia::Pixmap::new(size, size).ok_or("icon size must be non-zero")?;
    let out_data = out.data_mut();
    // Overlap of the one-pixel-wide source cell `s` with the span [a, b).
    let overlap = |s: usize, a: f64, b: f64| (b.min(s as f64 + 1.0) - a.max(s as f64)).max(0.0);
    for y in 0..n {
        let (y0, y1) = (y as f64 * ratio, (y + 1) as f64 * ratio);
        for x in 0..n {
            let (x0, x1) = (x as f64 * ratio, (x + 1) as f64 * ratio);
            let mut sum = [0f64; 4];
            let mut weight = 0f64;
            for sy in y0.floor() as usize..(y1.ceil() as usize).min(src) {
                let wy = overlap(sy, y0, y1);
                for sx in x0.floor() as usize..(x1.ceil() as usize).min(src) {
                    let w = wy * overlap(sx, x0, x1);
                    let i = (sy * src + sx) * 4;
                    for (acc, &value) in sum.iter_mut().zip(&data[i..i + 4]) {
                        *acc += w * f64::from(value);
                    }
                    weight += w;
                }
            }
            let o = (y * n + x) * 4;
            for (dst, total) in out_data[o..o + 4].iter_mut().zip(sum) {
                *dst = (total / weight).round() as u8;
            }
        }
    }
    Ok(out)
}

/// Whether any pixel in the middle half of the pixmap (well inside the
/// tile's border) is dark: red below 128 at full alpha, in premultiplied
/// terms. The shell is drawn in dark ink, so this is what "rendered" looks
/// like.
fn has_dark_interior(pixmap: &tiny_skia::Pixmap) -> bool {
    let (w, h) = (pixmap.width() as usize, pixmap.height() as usize);
    let data = pixmap.data();
    (h / 4..h * 3 / 4).any(|y| {
        (w / 4..w * 3 / 4).any(|x| {
            let i = (y * w + x) * 4;
            data[i + 3] == 255 && data[i] < 128
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(size: u32, rgba: [u8; 4]) -> tiny_skia::Pixmap {
        let mut pixmap = tiny_skia::Pixmap::new(size, size).unwrap();
        for px in pixmap.data_mut().chunks_exact_mut(4) {
            px.copy_from_slice(&rgba);
        }
        pixmap
    }

    #[test]
    fn uniform_source_stays_uniform_at_a_size_that_does_not_divide_it() {
        let source = filled(1024, [200, 100, 50, 255]);
        let out = downsample(&source, 48).unwrap();
        assert_eq!((out.width(), out.height()), (48, 48));
        assert!(
            out.data()
                .chunks_exact(4)
                .all(|px| px == [200, 100, 50, 255])
        );
    }

    #[test]
    fn checkerboard_averages_to_the_mean() {
        // 4x4 checkerboard of opaque black and white, premultiplied.
        let mut source = tiny_skia::Pixmap::new(4, 4).unwrap();
        for (i, px) in source.data_mut().chunks_exact_mut(4).enumerate() {
            let (x, y) = (i % 4, i / 4);
            let v = if (x + y) % 2 == 0 { 0 } else { 255 };
            px.copy_from_slice(&[v, v, v, 255]);
        }
        for size in [2, 1] {
            let out = downsample(&source, size).unwrap();
            // Each output pixel covers an equal number of black and white
            // pixels, so the mean is 127.5, rounded to 128.
            assert!(
                out.data()
                    .chunks_exact(4)
                    .all(|px| px == [128, 128, 128, 255]),
                "size {size}: {:?}",
                out.data()
            );
        }
    }

    #[test]
    fn two_by_two_to_one_weights_premultiplied_alpha() {
        // Three transparent pixels and one opaque red: a quarter of the
        // coverage, so premultiplied red 64 (255 / 4 rounded) and alpha 64.
        let mut source = tiny_skia::Pixmap::new(2, 2).unwrap();
        source.data_mut()[0..4].copy_from_slice(&[255, 0, 0, 255]);
        let out = downsample(&source, 1).unwrap();
        assert_eq!(out.data(), [64, 0, 0, 64]);
    }

    #[test]
    fn same_size_is_a_copy_and_upsampling_is_an_error() {
        let source = filled(8, [1, 2, 3, 4]);
        assert_eq!(downsample(&source, 8).unwrap().data(), source.data());
        assert!(downsample(&source, 16).is_err());
        let wide = tiny_skia::Pixmap::new(8, 4).unwrap();
        assert!(downsample(&wide, 2).is_err());
    }

    #[test]
    fn dark_interior_ignores_the_border_and_needs_ink() {
        let mut tile = filled(64, [255, 255, 255, 255]);
        // A black border ring must not count.
        for px in tile.data_mut()[..64 * 4].chunks_exact_mut(4) {
            px.copy_from_slice(&[0, 0, 0, 255]);
        }
        assert!(!has_dark_interior(&tile));
        // One black pixel in the middle does.
        let centre = (32 * 64 + 32) * 4;
        tile.data_mut()[centre..centre + 4].copy_from_slice(&[0, 0, 0, 255]);
        assert!(has_dark_interior(&tile));
        // Transparent pixels never count.
        assert!(!has_dark_interior(&filled(16, [0, 0, 0, 0])));
    }
}
