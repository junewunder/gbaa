//! `aw-check FONT PAGE.png...`: reads armwrestler's result pages (#35)
//! from `gbaa-desktop --dump` frames and prints each page's title, how many
//! lines say OK, and every line that says BAD.
//!
//! armwrestler draws its text in mode 4 with an 8x8 font (`font8x8.pat`,
//! 64 bytes a glyph, a nonzero byte an inked pixel), glyph `c - 0x25` for
//! character `c`. So each line is read back by matching 8x8 cells against
//! the font, and printed as drawn: the source's `LDR &\]` is `LDR +#]`.
//!
//! Exits non-zero if a page has a BAD line or no results (it didn't draw),
//! or, with `--expect N`, if the pages don't have N result lines in all (a
//! page cut short).
//!
//! `aw-check --blank PAGE.png` exits 0 only if the frame is all black.

use aw_check::{Font, blank, load_png, read_lines};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, page] = &args[..]
        && flag == "--blank"
    {
        return match load_png(page) {
            Ok(ink) if blank(&ink) => ExitCode::SUCCESS,
            Ok(_) => ExitCode::FAILURE,
            Err(e) => {
                eprintln!("aw-check: {e}");
                ExitCode::from(2)
            }
        };
    }
    let mut expect = None;
    if let Some(i) = args.iter().position(|a| a == "--expect") {
        expect = args.get(i + 1).and_then(|n| n.parse::<usize>().ok());
        if expect.is_none() {
            eprintln!("aw-check: --expect needs a count");
            return ExitCode::from(2);
        }
        args.drain(i..i + 2);
    }
    let [font, pages @ ..] = &args[..] else {
        eprintln!("usage: aw-check FONT PAGE.png... [--expect N] | aw-check --blank PAGE.png");
        return ExitCode::from(2);
    };
    let font = match std::fs::read(font) {
        Ok(b) => Font::load(&b),
        Err(e) => {
            eprintln!("aw-check: {font}: {e}");
            return ExitCode::from(2);
        }
    };
    let mut ok = true;
    let mut results = 0;
    for page in pages {
        let ink = match load_png(page) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("aw-check: {e}");
                return ExitCode::from(2);
            }
        };
        let lines = read_lines(&font, &ink);
        if std::env::var_os("AW_DEBUG").is_some() {
            eprintln!("{lines:#?}");
        }
        let title = lines.first().map_or("(blank)", |l| l.trim());
        let has = |l: &&String, w: &str| l.split_whitespace().any(|t| t == w);
        let oks = lines.iter().filter(|l| has(l, "OK")).count();
        let bads: Vec<_> = lines.iter().filter(|l| has(l, "BAD")).collect();
        let verdict = match (oks + bads.len(), bads.len()) {
            (0, _) => "no results",
            (_, 0) => "pass",
            _ => "FAIL",
        };
        println!(
            "{title:<24} {verdict:<10} {oks:>2} OK  {:>2} BAD",
            bads.len()
        );
        for l in &bads {
            println!("    {}", l.trim());
        }
        ok &= oks > 0 && bads.is_empty();
        results += oks + bads.len();
    }
    if let Some(n) = expect
        && n != results
    {
        println!("expected {n} results, found {results}");
        ok = false;
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
