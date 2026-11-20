//! Two measures of the printed lift's text (unit `native/lift-plain-arith`):
//! the `let`s nothing reads (`unread_lets`), and the sums still printed as
//! `prim::add_with_carry(..).0` (`adc_sums`), whose flags the printer
//! writes from their own `.1` and `.2` if anything reads them, so the sum
//! itself prints as plain arithmetic ([`super::narrow::Form`]).
//!
//! They read the text rather than the printer's plan, so a `let` any path
//! prints (a compare tree's arms, a stand-in) counts. A function is the
//! text from one `fn ` line to the next; a `let` of a plain name is unread
//! where no use of the name reaches it, by Rust's block scopes: before its
//! block's `}` or a `let` that shadows it (a `let`'s right side read as
//! after it, which the lift's numbered names never need).

/// The `let`s of `text` (Rust source) nothing after them reads.
pub fn unread_lets(text: &str) -> u64 {
    fn_bodies(text).into_iter().map(unread_in).sum()
}

/// The `prim::add_with_carry(..).0` calls of `text`.
pub fn adc_sums(text: &str) -> u64 {
    const CALL: &str = "add_with_carry(";
    let b = text.as_bytes();
    let mut n = 0;
    let mut from = 0;
    while let Some(p) = text[from..].find(CALL) {
        let open = from + p + CALL.len();
        let mut depth = 1;
        let mut j = open;
        while j < b.len() && depth > 0 {
            match b[j] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            j += 1;
        }
        // `j` is past the closing parenthesis: `.0` but not `.0x`.
        if b[j..].starts_with(b".0") && !b.get(j + 2).is_some_and(|c| c.is_ascii_alphanumeric()) {
            n += 1;
        }
        from = open;
    }
    n
}

/// The lines of `lets` (each `let NAME: T = ...;`, an expression with no
/// effect) that `body`, or a line kept after them, reads: a name `body`
/// uses before a `let` of its own binds it ([`free`]).
pub fn read_lets(lets: &str, body: &str) -> String {
    let mut seen = free(body);
    let mut kept: Vec<&str> = Vec::new();
    for line in lets.lines().rev() {
        let name = line
            .trim_start()
            .strip_prefix("let ")
            .and_then(|r| r.split(':').next())
            .map(str::trim);
        if name.is_none_or(|n| seen.contains(n)) {
            kept.push(line);
            seen.extend(free(line));
        }
    }
    kept.iter().rev().map(|l| format!("{l}\n")).collect()
}

/// The names `t` reads that no `let` of its own binds where they are
/// read (by Rust's block scopes).
fn free(t: &str) -> std::collections::HashSet<&str> {
    let mut scopes: Vec<Vec<&str>> = vec![Vec::new()];
    let mut out = std::collections::HashSet::new();
    for tok in tokens(t) {
        match tok {
            Tok::Open => scopes.push(Vec::new()),
            Tok::Close => {
                if scopes.len() > 1 {
                    scopes.pop();
                }
            }
            Tok::Word(w, true) => scopes.last_mut().expect("a scope").push(w),
            Tok::Word(w, false) => {
                if !scopes.iter().any(|s| s.contains(&w)) {
                    out.insert(w);
                }
            }
        }
    }
    out
}

/// A token of Rust source as these measures read it.
enum Tok<'a> {
    Open,
    Close,
    /// A name, and whether a `let` binds it here.
    Word(&'a str, bool),
}

/// The braces and names of `f`, strings, comments and numbers left out.
fn tokens(f: &str) -> Vec<Tok<'_>> {
    let b = f.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut after_let = false;
    while i < b.len() {
        let c = b[i];
        match c {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += 1 + usize::from(b[i] == b'\\');
                }
                i += 1;
                after_let = false;
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'{' => out.push(Tok::Open),
            b'}' => out.push(Tok::Close),
            _ => {}
        }
        if is_word(c) && (i == 0 || !is_word(b[i - 1])) {
            let s = i;
            while i < b.len() && is_word(b[i]) {
                i += 1;
            }
            let w = &f[s..i];
            // A number or a literal's suffix isn't a name.
            if b[s].is_ascii_digit() {
                continue;
            }
            match w {
                "let" => after_let = true,
                "mut" if after_let => {}
                _ => {
                    // `let NAME:` or `let NAME =`, not a pattern.
                    let rest = f[i..].trim_start();
                    let binds = after_let && (rest.starts_with(':') || rest.starts_with('='));
                    after_let = false;
                    out.push(Tok::Word(w, binds));
                }
            }
            continue;
        }
        if !c.is_ascii_whitespace() {
            after_let = false;
        }
        i += 1;
    }
    out
}

/// Each function's text: from a line whose first word (after `pub`) is
/// `fn` to the next such line.
fn fn_bodies(text: &str) -> Vec<&str> {
    let mut starts: Vec<usize> = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let t = line.trim_start();
        let t = t.strip_prefix("pub ").unwrap_or(t);
        if t.starts_with("fn ") {
            starts.push(at);
        }
        at += line.len();
    }
    starts.push(text.len());
    std::iter::once(&text[..starts[0]])
        .chain(starts.windows(2).map(|w| &text[w[0]..w[1]]))
        .filter(|s| !s.is_empty())
        .collect()
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// The unread `let`s of one function's text: by brace scope, a `let`
/// in scope until its block's `}` or a `let` of the same name in the same
/// block; a name read is the innermost `let` of it in scope.
fn unread_in(f: &str) -> u64 {
    unread_at(f).len() as u64
}

/// Where in `f` (one function's text, or part of one) the names of its
/// unread `let`s are, by byte offset, as [`unread_lets`] counts them.
pub fn unread_at(f: &str) -> Vec<usize> {
    // Each open block's `let`s: name, where, and whether read.
    let mut scopes: Vec<Vec<(&str, usize, bool)>> = vec![Vec::new()];
    let at = |w: &str| w.as_ptr() as usize - f.as_ptr() as usize;
    let mut out = Vec::new();
    let unread = |scope: Vec<(&str, usize, bool)>, out: &mut Vec<usize>| {
        out.extend(scope.into_iter().filter(|l| !l.2).map(|l| l.1));
    };
    for tok in tokens(f) {
        match tok {
            Tok::Open => scopes.push(Vec::new()),
            Tok::Close => {
                if scopes.len() > 1 {
                    unread(scopes.pop().expect("a scope"), &mut out);
                }
            }
            Tok::Word(w, true) => {
                let top = scopes.last_mut().expect("a scope");
                if let Some(k) = top.iter().position(|(x, _, _)| *x == w) {
                    let l = top.remove(k);
                    if !l.2 {
                        out.push(l.1);
                    }
                }
                // `_`-led names are meant unread.
                top.push((w, at(w), w.starts_with('_')));
            }
            Tok::Word(w, false) => {
                if let Some(l) = scopes
                    .iter_mut()
                    .rev()
                    .find_map(|sc| sc.iter_mut().rev().find(|(x, _, _)| *x == w))
                {
                    l.2 = true;
                }
            }
        }
    }
    for sc in scopes {
        unread(sc, &mut out);
    }
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_let_is_unread_until_a_use_before_its_next_let() {
        let f = "fn f() {\n    let v1: u32 = r0;\n    let v2: u32 = v1;\n    let v1: u32 = 3;\n    st8(g, v2, 0);\n}\nfn g() {\n    let mut v1: u32 = 0;\n    let (a, b) = x;\n    \"v9\";\n}\n";
        // v1's first is read (by v2), its second isn't, nor g's.
        assert_eq!(unread_lets(f), 2);
        // An inner block's `let` of a name the outer one reads after it.
        let f = "fn f() {\n    let v1: u32 = r0;\n    if c { let v1: u32 = 3; st8(g, v1, 0); }\n    r1 = v1;\n}\n";
        assert_eq!(unread_lets(f), 0);
    }

    #[test]
    fn only_a_sum_counts() {
        let t = "r0 = prim::add_with_carry(v1, (v2), true).0; cf = prim::add_with_carry(v1, !v2, true).1; x.0";
        assert_eq!(adc_sums(t), 1);
    }

    #[test]
    fn an_arm_keeps_the_lets_it_reads() {
        let lets =
            "    let v1: u32 = r0;\n    let v2: u32 = 0x8u32;\n    let v3: bool = v1 >= 5;\n";
        assert_eq!(
            read_lets(lets, "cf = v3;"),
            "    let v1: u32 = r0;\n    let v3: bool = v1 >= 5;\n"
        );
        assert_eq!(read_lets(lets, "break;"), "");
        // A nested block's own `let` of the name: not this one's.
        assert_eq!(read_lets(lets, "let v3: bool = true; cf = v3;"), "");
    }
}
