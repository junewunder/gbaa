use super::*;

/// A made-up font of armwrestler's 84 glyphs: glyph `i` is `i` in binary
/// along its top row, so glyph 0 is blank.
fn font() -> Font {
    let mut bytes = vec![0u8; 64 * 84];
    for i in 1..84 {
        for x in 0..8 {
            bytes[i * 64 + x] = (i >> x) as u8 & 1;
        }
    }
    Font::load(&bytes)
}

fn draw(font: &Font, ink: &mut [bool], s: &str, x: usize, y: usize) {
    for (n, c) in s.bytes().enumerate() {
        if c == b' ' {
            continue;
        }
        let g = &font.0[(c - FIRST) as usize];
        for (i, &p) in g.iter().enumerate() {
            ink[(y + i / 8) * W + x + n * 8 + i % 8] = p;
        }
    }
}

#[test]
fn reads_lines_as_drawn() {
    let f = font();
    let mut ink = vec![false; W * H];
    // Titles needn't be on the 8-pixel grid; results are.
    draw(&f, &mut ink, "ALU", 59, 1);
    draw(&f, &mut ink, "ADC  OK", 16, 16);
    draw(&f, &mut ink, "LDR &\\] BAD R^", 16, 24);
    assert!(!blank(&ink));
    assert_eq!(
        read_lines(&f, &ink),
        [
            format!("{}ALU", " ".repeat(7)),
            "  ADC  OK".to_string(),
            "  LDR +#] BAD Rd".to_string(),
        ]
    );
}

#[test]
fn scribbled_cells_read_as_unknown() {
    let f = font();
    let mut ink = vec![false; W * H];
    draw(&f, &mut ink, "LDR  BAD Mem", 16, 40);
    // A pixel no glyph has, in the last cell.
    ink[(40 + 7) * W + 16 + 11 * 8] = true;
    assert_eq!(read_lines(&f, &ink), ["  LDR  BAD Me?"]);
    assert!(blank(&vec![false; W * H]));
}
