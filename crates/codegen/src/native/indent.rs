//! Lifted code indented by its nesting (unit `native/lift-indent`). The
//! printer builds a function's text from pieces that each carry a fixed
//! indent (a statement 16 spaces, a loop's opener 4, a call 20), so a
//! nested loop read flat. [`reindent`] prints each line at 4 spaces a
//! brace level instead: a statement at its block's depth, an opener with
//! the code around it and a closer with its opener, a comment (`//
//! src/file.c:31`, `// dwarf r8: i`) at the depth of the statement it
//! precedes. No rustfmt run over the lift: a game's is 850,000 lines, and
//! a cut's three-way merge wants text that only changes where the code
//! does; this pass is cheap and keeps every line the line it was.
//!
//! The depth counts `{` and `}` outside string and char literals and
//! comments; a line inside a multi-line string or block comment is kept
//! as it is. A line's leading `}`s (after any `)`, `]`, `;` or `,` among
//! them) close before it prints, so `} else {` sits with its `if`.

#[derive(Clone, Copy, PartialEq)]
enum In {
    Code,
    /// A string literal: `"…"` (escapes) or a raw one, its `#`s.
    Str(Option<usize>),
    /// A block comment, nested this deep.
    Comment(usize),
}

/// `text` with each line's leading whitespace 4 spaces a brace level
/// (starting at 0), but for lines that start inside a string literal or a
/// block comment, kept as they are. Blank lines are left empty.
pub fn reindent(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    let mut depth = 0usize;
    let mut state = In::Code;
    for l in text.split_inclusive('\n') {
        let (body, end) = match l.strip_suffix('\n') {
            Some(b) => (b, "\n"),
            None => (l, ""),
        };
        if state != In::Code {
            out.push_str(body);
        } else {
            let t = body.trim_start();
            if !t.is_empty() {
                let closes = t
                    .bytes()
                    .take_while(|b| matches!(b, b'}' | b')' | b']' | b';' | b',' | b' '))
                    .filter(|&b| b == b'}')
                    .count();
                for _ in 0..depth.saturating_sub(closes) {
                    out.push_str("    ");
                }
                out.push_str(t.trim_end());
            }
        }
        out.push_str(end);
        scan(body, &mut state, &mut depth);
    }
    out
}

/// The brace depth and literal state after `line`.
fn scan(line: &str, state: &mut In, depth: &mut usize) {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match *state {
            In::Str(None) => {
                match b[i] {
                    b'\\' => i += 1,
                    b'"' => *state = In::Code,
                    _ => {}
                }
                i += 1;
            }
            In::Str(Some(n)) => {
                if b[i] == b'"' && b[i + 1..].iter().take(n).filter(|&&c| c == b'#').count() == n {
                    *state = In::Code;
                    i += n;
                }
                i += 1;
            }
            In::Comment(d) => {
                if b[i..].starts_with(b"*/") {
                    *state = if d == 1 { In::Code } else { In::Comment(d - 1) };
                    i += 2;
                } else if b[i..].starts_with(b"/*") {
                    *state = In::Comment(d + 1);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            In::Code => {
                let ident_before = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
                match b[i] {
                    b'{' => *depth += 1,
                    b'}' => *depth = depth.saturating_sub(1),
                    b'"' => *state = In::Str(None),
                    b'/' if b.get(i + 1) == Some(&b'/') => return,
                    b'/' if b.get(i + 1) == Some(&b'*') => {
                        *state = In::Comment(1);
                        i += 1;
                    }
                    b'r' if !ident_before && matches!(b.get(i + 1), Some(b'"' | b'#')) => {
                        let n = b[i + 1..].iter().take_while(|&&c| c == b'#').count();
                        if b.get(i + 1 + n) == Some(&b'"') {
                            *state = In::Str(Some(n));
                            i += 1 + n;
                        }
                    }
                    // A char literal (`'{'`, `'\''`); else a label or lifetime.
                    b'\'' => {
                        if b.get(i + 1) == Some(&b'\\') {
                            i += 2;
                            while i < b.len() && b[i] != b'\'' {
                                i += 1;
                            }
                        } else if let Some(c) = line[i + 1..].chars().next()
                            && line[i + 1 + c.len_utf8()..].starts_with('\'')
                        {
                            i += c.len_utf8() + 1;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
        }
    }
}

/// Each nonblank line's leading spaces against 4 times its brace depth:
/// the lines (1-based) where they differ, with the text. For a test of a
/// lift's modules.
pub fn misindented(text: &str) -> Vec<(usize, String)> {
    let mut bad = Vec::new();
    let again = reindent(text);
    for (n, (a, b)) in text.lines().zip(again.lines()).enumerate() {
        if a != b {
            bad.push((n + 1, a.to_string()));
        }
    }
    bad
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_loops_indent_by_depth() {
        let flat = "pub fn f(g: &mut Game) -> () {\n    let x = 0u32;\n                // c/a.c:3\n    'l1: loop {\n                let a = 1;\n    'l2: loop {\n                if a {\n                    continue 'l2;\n                } else {\n                x = 2;\n                }\n                    break 'l2;\n    }\n                    break 'l1;\n    }\n                    return ();\n}\n";
        let want = "pub fn f(g: &mut Game) -> () {\n    let x = 0u32;\n    // c/a.c:3\n    'l1: loop {\n        let a = 1;\n        'l2: loop {\n            if a {\n                continue 'l2;\n            } else {\n                x = 2;\n            }\n            break 'l2;\n        }\n        break 'l1;\n    }\n    return ();\n}\n";
        assert_eq!(reindent(flat), want);
        assert!(misindented(want).is_empty());
        assert_eq!(misindented(flat).len(), 11);
    }

    #[test]
    fn literals_and_comments_hold_no_braces() {
        let t = "fn f() {\n        panic!(\"0x0800: {{ a trap }}\");\n  let c = '{';\n  let d = '\\'';\n  let s = r#\"}\"#; // }\n  'l1: loop { /* { */ }\n}\n";
        let want = "fn f() {\n    panic!(\"0x0800: {{ a trap }}\");\n    let c = '{';\n    let d = '\\'';\n    let s = r#\"}\"#; // }\n    'l1: loop { /* { */ }\n}\n";
        assert_eq!(reindent(t), want);
    }

    #[test]
    fn a_multi_line_string_keeps_its_lines() {
        let t = "fn f() {\nlet s = \"a {\n   b\";\n}\n";
        assert_eq!(reindent(t), "fn f() {\n    let s = \"a {\n   b\";\n}\n");
    }

    #[test]
    fn match_arms_and_one_line_items() {
        let t = "pub fn x_08000000(g: &mut Game, s: Regs) -> () { f(g) }\nfn f() {\n    match b {\n        0 => { pc(0x08000000);\n    a();\n        }\n        _ => unreachable!(),\n    }\n}\n";
        let want = "pub fn x_08000000(g: &mut Game, s: Regs) -> () { f(g) }\nfn f() {\n    match b {\n        0 => { pc(0x08000000);\n            a();\n        }\n        _ => unreachable!(),\n    }\n}\n";
        assert_eq!(reindent(t), want);
    }
}
