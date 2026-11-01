//! `png-diff A.png B.png OUT.png`: prints how many pixels differ between
//! two same-sized RGB frame dumps and writes OUT, A dimmed to grey with
//! the differing pixels in red. Exits 1 if any differ. For `just mgba-diff`.

use std::fs::File;
use std::io::BufWriter;
use std::process::ExitCode;

fn read(path: &str) -> Result<(u32, u32, Vec<u8>), String> {
    let err = |e: &dyn std::fmt::Display| format!("{path}: {e}");
    let mut dec = png::Decoder::new(std::io::BufReader::new(
        File::open(path).map_err(|e| err(&e))?,
    ));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().map_err(|e| err(&e))?;
    let mut buf = vec![0; r.output_buffer_size().ok_or_else(|| err(&"too large"))?];
    let info = r.next_frame(&mut buf).map_err(|e| err(&e))?;
    if info.color_type != png::ColorType::Rgb {
        return Err(err(&"not an RGB image"));
    }
    buf.truncate(info.buffer_size());
    Ok((info.width, info.height, buf))
}

/// Pixels that differ, and the diff image's RGB bytes.
fn diff(a: &[u8], b: &[u8]) -> (usize, Vec<u8>) {
    let mut n = 0;
    let mut out = Vec::with_capacity(a.len());
    for (pa, pb) in a.chunks(3).zip(b.chunks(3)) {
        if pa == pb {
            let grey = ((pa[0] as u32 + pa[1] as u32 + pa[2] as u32) / 3 / 3 + 40) as u8;
            out.extend([grey; 3]);
        } else {
            n += 1;
            out.extend([255, 0, 0]);
        }
    }
    (n, out)
}

fn run(args: &[String]) -> Result<usize, String> {
    let [a, b, out] = args else {
        return Err("usage: png-diff A.png B.png OUT.png".into());
    };
    let (wa, ha, da) = read(a)?;
    let (wb, hb, db) = read(b)?;
    if (wa, ha) != (wb, hb) {
        return Err(format!("{a} is {wa}x{ha} but {b} is {wb}x{hb}"));
    }
    let (n, img) = diff(&da, &db);
    let file = File::create(out).map_err(|e| format!("{out}: {e}"))?;
    let mut enc = png::Encoder::new(BufWriter::new(file), wa, ha);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| format!("{out}: {e}"))?;
    w.write_image_data(&img)
        .map_err(|e| format!("{out}: {e}"))?;
    w.finish().map_err(|e| format!("{out}: {e}"))?;
    Ok(n)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(0) => {
            println!("0 pixels differ");
            ExitCode::SUCCESS
        }
        Ok(n) => {
            println!("{n} pixels differ");
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("png-diff: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn counts_pixels() {
        let a = [1, 2, 3, 4, 5, 6, 7, 8, 9];
        let b = [1, 2, 3, 4, 5, 0, 7, 8, 9];
        let (n, img) = super::diff(&a, &b);
        assert_eq!(n, 1);
        assert_eq!(&img[3..6], &[255, 0, 0]);
        assert_eq!(super::diff(&a, &a).0, 0);
    }
}
