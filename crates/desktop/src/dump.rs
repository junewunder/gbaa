//! Frame dumps: a PNG for people, a hash for the interpreter-vs-recompiled
//! comparison.

use runtime::Frame;
use runtime::platform::{HEIGHT, WIDTH};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

/// A 5-bit channel widened to 8 bits, so 31 maps to 255.
fn widen(c: u16) -> u8 {
    let c = (c & 31) as u8;
    (c << 3) | (c >> 2)
}

/// A GBA colour as 8-bit red, green, blue.
pub fn rgb(c: u16) -> [u8; 3] {
    [widen(c), widen(c >> 5), widen(c >> 10)]
}

/// Write `frame` as a 240×160 RGB PNG.
pub fn write_png(path: &Path, frame: &Frame) -> std::io::Result<()> {
    write_png_wide(path, frame)
}

/// `rgb` under `overlay`'s ARGB pixel (a script's canvas layers).
pub fn over(rgb: [u8; 3], overlay: u32) -> [u8; 3] {
    let [_, r, g, b] =
        script::canvas::mix(overlay, u32::from_be_bytes([0xFF, rgb[0], rgb[1], rgb[2]]))
            .to_be_bytes();
    [r, g, b]
}

/// [`write_png`] with the scripts' canvas layers ([`script::Host::overlay`])
/// drawn over it and its sidebars (black under them), as the window shows
/// them: as wide as the picture and its sidebars.
pub fn write_png_over(
    path: &Path,
    frame: &Frame,
    overlay: Option<&script::canvas::Overlay>,
) -> std::io::Result<()> {
    let Some(o) = overlay else {
        return write_png(path, frame);
    };
    let mut enc = png::Encoder::new(
        BufWriter::new(File::create(path)?),
        o.width as u32,
        HEIGHT as u32,
    );
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let data: Vec<u8> =
        o.px.iter()
            .enumerate()
            .flat_map(|(i, &px)| {
                let (x, y) = ((i % o.width).wrapping_sub(o.left), i / o.width);
                let under = if x < WIDTH {
                    rgb(frame[y * WIDTH + x])
                } else {
                    [0; 3]
                };
                over(under, px)
            })
            .collect();
    let mut w = enc.write_header()?;
    w.write_image_data(&data)?;
    w.finish()?;
    Ok(())
}

/// Write screens side by side ([`crate::window::side_by_side`]) as one
/// PNG as wide as they are.
pub fn write_png_wide(path: &Path, frame: &[u16]) -> std::io::Result<()> {
    debug_assert_eq!(frame.len() % (WIDTH * HEIGHT), 0);
    let mut enc = png::Encoder::new(
        BufWriter::new(File::create(path)?),
        (frame.len() / HEIGHT) as u32,
        HEIGHT as u32,
    );
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let data: Vec<u8> = frame.iter().flat_map(|&c| rgb(c)).collect();
    let mut w = enc.write_header()?;
    w.write_image_data(&data)?;
    w.finish()?;
    Ok(())
}

/// FNV-1a 64 over the frame's pixels, each as two little-endian bytes.
pub fn frame_hash(frame: &Frame) -> u64 {
    pixels_hash(frame)
}

/// [`frame_hash`] over any pixels: screens side by side.
pub fn pixels_hash(frame: &[u16]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in frame.iter().flat_map(|c| c.to_le_bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}
