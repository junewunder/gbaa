//! The per-game list of spin loops the rule misses (`spins.toml`,
//! docs/research/busy-wait-loops.md, option (c) as a fallback): back
//! edges `--timing native` counts as spin loops beside the ones
//! `armv4t::idle::is_spin_edge` finds ([`crate::Machine::spin_list`]).
//! Built in here, so the code generator, every front end's interpreter
//! and the native lifter read the same list.

/// The list, as written.
pub const LIST: &str = include_str!("../spins.toml");

/// Each spin edge [`LIST`] gives the ROM whose SHA-1 is `sha1`.
pub fn listed(sha1: &[u8; 20], each: impl FnMut(u32)) {
    listed_in(LIST, sha1, each)
}

/// [`listed`] over the text `list`.
fn listed_in(list: &str, sha1: &[u8; 20], mut each: impl FnMut(u32)) {
    let mut hex = [0u8; 40];
    for (i, b) in sha1.iter().enumerate() {
        let d = b"0123456789abcdef";
        hex[2 * i] = d[(b >> 4) as usize];
        hex[2 * i + 1] = d[(b & 15) as usize];
    }
    let want = core::str::from_utf8(&hex).unwrap_or("");
    let (mut ours, mut in_edges) = (false, false);
    for line in list.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if !in_edges && line.starts_with('[') {
            let name = line.trim_start_matches("[games.").trim_end_matches(']');
            ours = name.trim_matches('"') == want;
            continue;
        }
        let mut rest = line;
        if !in_edges {
            let Some(v) = line
                .strip_prefix("edges")
                .and_then(|v| v.trim_start().strip_prefix('='))
            else {
                continue;
            };
            let v = v.trim_start();
            rest = v.strip_prefix('[').unwrap_or(v);
        }
        let (items, done) = match rest.split_once(']') {
            Some((items, _)) => (items, true),
            None => (rest, false),
        };
        if ours {
            items.split(',').filter_map(number).for_each(&mut each);
        }
        in_edges = !done;
    }
}

/// A number as `spins.toml` writes it: hex (`0x0800_1234`) or decimal,
/// underscores allowed.
fn number(s: &str) -> Option<u32> {
    let s = s.trim();
    let (digits, radix) = match s.strip_prefix("0x") {
        Some(h) => (h, 16),
        None => (s, 10),
    };
    let mut v: u32 = 0;
    let mut any = false;
    for c in digits.chars().filter(|&c| c != '_') {
        v = v.checked_mul(radix)?.checked_add(c.to_digit(radix)?)?;
        any = true;
    }
    any.then_some(v)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    #[test]
    fn a_game_is_found_by_its_sha1() {
        let sha = [0xAB; 20];
        let list = "[games.\"abababababababababababababababababababab\"]\n\
                    name = \"x\"\nedges = [0x0800_1234,\n  4096 ] # two\n\
                    [games.\"00\"]\nedges = [1]\n";
        let mut v = Vec::new();
        listed_in(list, &sha, |e| v.push(e));
        assert_eq!(v, [0x0800_1234, 4096]);
        let mut none = Vec::new();
        listed(&[0; 20], |e| none.push(e));
        assert!(none.is_empty());
    }
}
