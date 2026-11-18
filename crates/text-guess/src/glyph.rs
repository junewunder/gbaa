//! Drawing a ROM's glyphs for an agent to read: 4bpp tiles (the GBA's
//! own tile format, 32 bytes an 8x8 tile, low nibble the left pixel) as a
//! grey PNG, each glyph in a cell of a grid with a line between cells, so
//! a glyph's code is its place: row by row, `per_row` a row.

/// How a code's glyph sits in the ROM: `base + code * stride` is its top
/// left tile; it is `wide` tiles across and `tall` down, a row of tiles
/// `row` bytes after the one above.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub base: u32,
    pub stride: u32,
    pub wide: u32,
    pub tall: u32,
    pub row: u32,
}

/// The 8x8 4bpp tile at ROM offset `at` as colour indices, row by row; 0s
/// past the ROM's end.
fn tile(rom: &[u8], at: usize) -> [[u8; 8]; 8] {
    let mut t = [[0; 8]; 8];
    for (y, row) in t.iter_mut().enumerate() {
        for (x, px) in row.iter_mut().enumerate() {
            let b = rom.get(at + y * 4 + x / 2).copied().unwrap_or(0);
            *px = if x % 2 == 0 { b & 15 } else { b >> 4 };
        }
    }
    t
}

/// Grey pixels of `codes`' glyphs, a cell each, `per_row` cells a row,
/// each pixel `scale` square; a mid-grey line between cells. Returns the
/// width, height and one byte a pixel.
pub fn sheet(
    rom: &[u8],
    l: Layout,
    codes: &[u32],
    per_row: u32,
    scale: u32,
) -> (u32, u32, Vec<u8>) {
    let (gw, gh) = (l.wide * 8, l.tall * 8);
    let (cw, ch) = (gw * scale + 2, gh * scale + 2);
    let rows = (codes.len() as u32).div_ceil(per_row).max(1);
    let (w, h) = (per_row * cw, rows * ch);
    let mut px = vec![128u8; (w * h) as usize];
    for (i, &code) in codes.iter().enumerate() {
        let (cx, cy) = ((i as u32 % per_row) * cw + 1, (i as u32 / per_row) * ch + 1);
        let top = l.base.wrapping_add(code.wrapping_mul(l.stride)) & 0x01FF_FFFF;
        for ty in 0..l.tall {
            for tx in 0..l.wide {
                let t = tile(rom, (top + ty * l.row + tx * 32) as usize);
                for (y, row) in t.iter().enumerate() {
                    for (x, &c) in row.iter().enumerate() {
                        // Index 0 is the background: white; ink dark.
                        let v = if c == 0 { 255 } else { 255 - 17 * c.min(15) };
                        for sy in 0..scale {
                            for sx in 0..scale {
                                let px_x = cx + (tx * 8 + x as u32) * scale + sx;
                                let px_y = cy + (ty * 8 + y as u32) * scale + sy;
                                px[(px_y * w + px_x) as usize] = v;
                            }
                        }
                    }
                }
            }
        }
    }
    (w, h, px)
}

/// Writes grey pixels as a PNG.
pub fn write_png(path: &std::path::Path, w: u32, h: u32, px: &[u8]) -> Result<(), String> {
    let f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    enc.set_color(png::ColorType::Grayscale);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(|e| e.to_string())?;
    wr.write_image_data(px).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glyph_lands_in_its_cell() {
        // Two 1-tile glyphs: code 0 blank, code 1 with its top-left pixel
        // ink (colour 1, low nibble).
        let mut rom = vec![0u8; 64];
        rom[32] = 1;
        let l = Layout {
            base: 0,
            stride: 32,
            wide: 1,
            tall: 1,
            row: 0,
        };
        let (w, h, px) = sheet(&rom, l, &[0, 1], 2, 1);
        assert_eq!((w, h), (20, 10));
        assert_eq!(px[0], 128); // the line
        assert_eq!(px[(w + 1) as usize], 255); // code 0's first pixel
        assert_eq!(px[(w + 11) as usize], 238); // code 1's
    }
}
