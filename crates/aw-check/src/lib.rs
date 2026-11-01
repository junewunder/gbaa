//! Reading armwrestler's result pages (#35) back from frame dumps. See
//! `main.rs` for the tool.

#[cfg(test)]
mod tests;

pub const W: usize = 240;
pub const H: usize = 160;
const FIRST: u8 = 0x25;

/// armwrestler's 8x8 font (`font8x8.pat`): 64 bytes a glyph, a nonzero
/// byte an inked pixel, glyph `c - 0x25` for character `c`.
pub struct Font(Vec<[bool; 64]>);

impl Font {
    pub fn load(bytes: &[u8]) -> Font {
        let glyphs = bytes.as_chunks::<64>().0;
        Font(glyphs.iter().map(|g| g.map(|b| b != 0)).collect())
    }

    /// The character whose glyph is exactly `cell`. Glyph 0 doubles as
    /// the blank a space draws, so it reads as `' '` and isn't matched.
    fn read(&self, cell: &[bool; 64]) -> Option<char> {
        let i = self.0.iter().skip(1).position(|g| g == cell)? + 1;
        Some((FIRST + i as u8) as char)
    }
}

/// A frame as inked (non-black) pixels.
pub fn load_png(path: &str) -> Result<Vec<bool>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let mut r = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .map_err(|e| format!("{path}: {e}"))?;
    let mut buf = vec![0; r.output_buffer_size().ok_or("frame too big")?];
    let info = r.next_frame(&mut buf).map_err(|e| format!("{path}: {e}"))?;
    if (info.width, info.height, info.color_type) != (W as u32, H as u32, png::ColorType::Rgb) {
        return Err(format!("{path}: not a 240x160 RGB frame dump"));
    }
    Ok(buf[..W * H * 3]
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| *p != [0; 3])
        .collect())
}

fn cell(ink: &[bool], x: usize, y: usize) -> [bool; 64] {
    std::array::from_fn(|i| ink[(y + i / 8) * W + x + i % 8])
}

/// What the font draws for the source characters that don't look like
/// themselves (checked against the frames). `^` and `_` share `d`'s and
/// `n`'s glyphs, so `Rd` and `Rn` (the source's `R^`, `R_`) read right.
const SHOWN: [(char, char); 6] = [
    ('&', '+'),
    ('\'', '-'),
    ('\\', '#'),
    (')', '!'),
    ('^', 'd'),
    ('_', 'n'),
];

/// The text of row `y` read in 8-pixel cells from `x0`, as drawn. A cell
/// that is inked but no glyph reads `?`: a failing Mem or Rn test draws its
/// value with a mode 3 routine on this mode 4 screen, which scribbles over
/// other rows. A row needs at least two glyphs, and no more `?` than
/// glyphs, so rows between the text lines don't read.
fn read_row(font: &Font, ink: &[bool], x0: usize, y: usize) -> Option<String> {
    let (mut glyphs, mut unknown) = (0, 0);
    let s: String = (x0..=W - 8)
        .step_by(8)
        .map(|x| {
            let c = cell(ink, x, y);
            if !c.iter().any(|&p| p) {
                return ' ';
            }
            match font.read(&c) {
                Some(ch) => {
                    glyphs += 1;
                    SHOWN.iter().find(|p| p.0 == ch).map_or(ch, |p| p.1)
                }
                None => {
                    unknown += 1;
                    '?'
                }
            }
        })
        .collect();
    (glyphs >= 2 && unknown <= glyphs).then(|| s.trim_end().to_string())
}

/// The page's text lines, top to bottom. Results are drawn at multiples
/// of 8 across, titles not always, so each row tries every offset and
/// keeps the one that reads the most glyphs.
pub fn read_lines(font: &Font, ink: &[bool]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut y = 0;
    while y + 8 <= H {
        let best = (0..8)
            .filter_map(|x0| read_row(font, ink, x0, y))
            .min_by_key(|s| s.matches(['?', ' ']).count());
        match best {
            Some(s) => {
                lines.push(s);
                y += 8;
            }
            None => y += 1,
        }
    }
    lines
}

/// Whether the frame has no ink at all.
pub fn blank(ink: &[bool]) -> bool {
    !ink.contains(&true)
}
