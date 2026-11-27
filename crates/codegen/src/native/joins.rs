//! Labelled blocks printed as returns and as `if`/`else` arms (unit
//! `native/lift-arm-values`; `docs/research/llm-pilot.md`, round two).
//!
//! A labelled block `'bL: { … }` is printed where a join isn't an `if`,
//! a `match` or a loop's (native/lift-join-shape). Two of its shapes are
//! plain code without one, exactly:
//!
//! - [`return_only`]: the code after the block only returns (the
//!   function's epilogue: `pc(…)`, line comments and one `return …;` that
//!   reads no memory and calls nothing). Each `break 'bL;` becomes that
//!   code, as the pilot's rewrites did (`IwramMalloc`, `TaskDestroy`).
//! - [`else_join`]: the block's breaks each end one arm of an `if` at the
//!   top of what falls to the block's end (`r3 = 0x14c; if c { r3 =
//!   0x1ac; break 'bL; } rest`): the rest of the block becomes the `if`'s
//!   other arm, so the arm that sets a value and the one that computes it
//!   join at the `if`'s end, as the rewrites' `let x = if … { … } else
//!   { … }` does.
//!
//! Both work on the printed text before [`super::indent::reindent`], so
//! a line's own indent doesn't matter, only its braces. Code only moves
//! into braces (the rest of a block into an arm), never out of them, so
//! no `let` comes into the scope of code it didn't reach before; the
//! return copied in checks that no name it reads is bound inside the
//! block.

/// The brace depth change of one printed line: `{` and `}` outside
/// string literals and line comments. A `'` starts no char literal (the
/// lift prints labels, not chars).
fn delta(line: &str) -> i32 {
    let mut d = 0;
    let mut in_str = false;
    let mut esc = false;
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
        } else {
            match c {
                b'"' => in_str = true,
                b'/' if b.get(i + 1) == Some(&b'/') => break,
                b'{' => d += 1,
                b'}' => d -= 1,
                _ => {}
            }
        }
        i += 1;
    }
    d
}

fn is_comment(t: &str) -> bool {
    t.starts_with("//")
}

/// A `pc(0x…);` marker line ([`super::pc_marks`]).
fn is_pc(t: &str) -> bool {
    t.strip_prefix("pc(0x")
        .and_then(|h| h.strip_suffix(");"))
        .is_some_and(|h| h.len() == 8 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The names `t` reads: identifiers not after `.` or `::` and not
/// before `(` or `!`.
fn names(t: &str) -> Vec<&str> {
    let b = t.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_alphabetic() || b[i] == b'_' {
            let s = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let after_path = s > 0 && (b[s - 1] == b'.' || b[s - 1] == b':');
            let called = matches!(b.get(i), Some(b'(' | b'!' | b':'));
            if !after_path && !called {
                out.push(&t[s..i]);
            }
        } else if b[i].is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Whether `body` binds `name` anywhere (a `let`, a `for`, a pattern's
/// `mut`), so that a use of it copied into `body` might read that one.
fn binds(body: &str, name: &str) -> bool {
    [
        "let ", "let mut ", "for ", "for mut ", "(mut ", ", mut ", "(", ", ",
    ]
    .iter()
    .any(|p| {
        let pat = format!("{p}{name}");
        body.match_indices(&pat).any(|(i, _)| {
            let rest = &body[i + pat.len()..];
            let ends = !rest
                .bytes()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_');
            // `(name` and `, name` bind only in a `let (…)` pattern.
            ends && (p.starts_with("let")
                || p.starts_with("for")
                || p.contains("mut")
                || body[..i]
                    .rsplit('\n')
                    .next()
                    .is_some_and(|l| l.trim_start().starts_with("let (")))
        })
    })
}

/// The code `rest` after a labelled block, if all it does is return: its
/// `pc` markers, line comments and one `return …;` that reads no memory
/// (`g`), calls nothing (`?`) and opens no block, with no name it reads
/// bound in `body` (the block's code, where it would be copied).
pub fn return_only<'a>(rest: &'a str, body: &str) -> Option<&'a str> {
    let mut ret = None;
    for l in rest.lines() {
        let t = l.trim();
        if t.is_empty() || is_comment(t) || is_pc(t) {
            continue;
        }
        let ok = ret.is_none()
            && (t.starts_with("return ") || t == "return;")
            && t.ends_with(';')
            && !t.contains(['{', '}', '?', '"'])
            && !names(t).contains(&"g");
        if !ok {
            return None;
        }
        ret = Some(t);
    }
    let r = ret?;
    if names(r).iter().any(|n| binds(body, n)) {
        return None;
    }
    Some(rest.trim_matches('\n'))
}

/// Why a labelled block's breaks were left: where the first break
/// [`else_join`] couldn't take is, among the block's top-level
/// statements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Left {
    /// Deeper in an `if`'s arm than its last statement (the arm falls on
    /// to code the break skips).
    InIf,
    /// In a `match` arm.
    InMatch,
    /// In a loop.
    InLoop,
    /// In another labelled block.
    InLabelled,
    /// Elsewhere: a break as a statement of its own before the end, an
    /// `if … else if` chain, an arm that ends in a jump away.
    Other,
    /// Its code holds a `match` arm's mark for the code after it
    /// (native/lift-join-shape's), which stays where it is.
    Marked,
}

impl Left {
    pub const ALL: [Left; 6] = [
        Left::InIf,
        Left::InMatch,
        Left::InLoop,
        Left::InLabelled,
        Left::Other,
        Left::Marked,
    ];

    /// Its metric's name.
    pub fn metric(self) -> &'static str {
        match self {
            Left::InIf => "joins_left_in_if",
            Left::InMatch => "joins_left_in_match",
            Left::InLoop => "joins_left_in_loop",
            Left::InLabelled => "joins_left_in_labelled",
            Left::Other => "joins_left_other",
            Left::Marked => "joins_left_marked",
        }
    }
}

/// A top-level statement of `lines` (a range of them) and its kind.
struct Stmt {
    start: usize,
    end: usize,
}

/// `lines` split into top-level statements (an opener and its body up
/// to its closer are one), or None if the braces don't balance.
fn stmts(lines: &[&str]) -> Option<Vec<Stmt>> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, l) in lines.iter().enumerate() {
        depth += delta(l);
        if depth < 0 {
            return None;
        }
        if depth == 0 {
            out.push(Stmt { start, end: i + 1 });
            start = i + 1;
        }
    }
    (depth == 0).then_some(out)
}

fn kind_of(head: &str) -> Left {
    let t = head.trim();
    if t.starts_with("if ") {
        Left::InIf
    } else if t.starts_with("match ") {
        Left::InMatch
    } else if t.starts_with("'l")
        || t.starts_with("loop ")
        || t.starts_with("for ")
        || t.starts_with("while ")
    {
        Left::InLoop
    } else if t.starts_with("'b") {
        Left::InLabelled
    } else {
        Left::Other
    }
}

/// An `if` statement's head, its arms' lines (each without its
/// braces' lines) and whether it had an `else`; None for an `if let`,
/// an `else if` or a one-line `if`.
/// An `if`'s head line, its `then` arm's lines and its `else` arm's.
type IfParts<'a> = (&'a str, Vec<&'a str>, Option<Vec<&'a str>>);

fn split_if<'a>(lines: &[&'a str]) -> Option<IfParts<'a>> {
    let head = lines[0];
    let t = head.trim();
    if !t.starts_with("if ") || t.starts_with("if let ") || !t.ends_with('{') || delta(head) != 1 {
        return None;
    }
    let last = lines.len() - 1;
    if lines[last].trim() != "}" {
        return None;
    }
    let mut depth = 1;
    let mut mid = None;
    for (i, l) in lines.iter().enumerate().take(last).skip(1) {
        let t = l.trim();
        if depth == 1 && t.starts_with('}') {
            if t != "} else {" || mid.is_some() {
                return None;
            }
            mid = Some(i);
        }
        depth += delta(l);
    }
    Some(match mid {
        Some(m) => (
            head,
            lines[1..m].to_vec(),
            Some(lines[m + 1..last].to_vec()),
        ),
        None => (head, lines[1..last].to_vec(), None),
    })
}

/// `arm` without the `break 'bL;` (`brk`) that ends it, if one does at
/// its top level.
fn drop_break(arm: &[&str], brk: &str) -> Option<Vec<String>> {
    let k = arm.iter().rposition(|l| {
        let t = l.trim();
        !t.is_empty() && !is_comment(t)
    })?;
    let t = arm[k].trim();
    let rest = if t == brk {
        None
    } else {
        let r = t.strip_suffix(brk)?;
        let r = r.trim_end();
        if !r.ends_with(';') && !r.ends_with('}') {
            return None;
        }
        Some(r.to_string())
    };
    // The break is at the arm's top level: the lines before it balance.
    if arm[..k].iter().map(|l| delta(l)).sum::<i32>() != 0 || delta(arm[k]) != 0 {
        return None;
    }
    let mut out: Vec<String> = arm[..k].iter().map(|s| s.to_string()).collect();
    out.extend(rest);
    out.extend(arm[k + 1..].iter().map(|s| s.to_string()));
    Some(out)
}

/// The condition `c` of an `if` negated.
fn negate(c: &str) -> String {
    let ident =
        |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    match c.strip_prefix('!') {
        Some(r) if ident(r) => r.to_string(),
        _ if ident(c) => format!("!{c}"),
        _ => format!("!({c})"),
    }
}

/// `body`, a labelled block's code, with every `break 'bL;` (`brk`)
/// gone: each ends an arm of an `if` that is a top-level statement of
/// what falls to the block's end, and the statements after that `if`
/// become its other arm (which falls on, as they did); a break that is
/// that code's last statement goes. Or why one is left.
pub fn else_join(body: &str, brk: &str) -> Result<String, Left> {
    let lines: Vec<&str> = body.lines().collect();
    let out = join_lines(&lines, brk)?;
    let mut s = out.join("\n");
    if body.ends_with('\n') {
        s.push('\n');
    }
    if s.contains(brk) {
        return Err(Left::Other);
    }
    Ok(s)
}

fn join_lines(lines: &[&str], brk: &str) -> Result<Vec<String>, Left> {
    let own = |s: &[&str]| s.iter().any(|l| l.contains(brk));
    let strs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    if !own(lines) {
        return Ok(strs(lines));
    }
    let st = stmts(lines).ok_or(Left::Other)?;
    let Some(n) = st.iter().position(|s| own(&lines[s.start..s.end])) else {
        return Err(Left::Other);
    };
    let s = &st[n];
    let here = &lines[s.start..s.end];
    let after = &lines[s.end..];
    let quiet = |v: &[&str]| {
        v.iter().all(|l| {
            let t = l.trim();
            t.is_empty() || is_comment(t)
        })
    };
    let last = quiet(after);
    let mut out = strs(&lines[..s.start]);
    // A break as the last statement: falling off goes there anyway.
    if here.len() == 1 && here[0].trim() == brk {
        if last {
            out.extend(strs(after));
            return Ok(out);
        }
        return Err(Left::Other);
    }
    let Some((head, then, els)) = split_if(here) else {
        let k = kind_of(here[0]);
        // The last statement, a labelled block or a `match`: its code
        // falls to the end as the whole does.
        if last && matches!(k, Left::InLabelled | Left::InMatch) && delta(here[0]) == 1 {
            let inner = &here[1..here.len() - 1];
            out.push(here[0].to_string());
            out.extend(match k {
                Left::InMatch => match_arms(inner, brk)?,
                _ => join_lines(inner, brk)?,
            });
            out.push(here[here.len() - 1].to_string());
            out.extend(strs(after));
            return Ok(out);
        }
        return Err(k);
    };
    let cond = head
        .trim()
        .strip_prefix("if ")
        .and_then(|c| c.strip_suffix('{'))
        .map(str::trim);
    let cond = cond.ok_or(Left::Other)?;
    let els_v: Vec<&str> = els.clone().unwrap_or_default();
    let (t2, e2) = (drop_break(&then, brk), drop_break(&els_v, brk));
    let with_rest = |arm: &[&str]| -> Result<Vec<String>, Left> {
        if !last && super::diverges(&arm.join("\n")) {
            return Err(Left::Other);
        }
        let mut v: Vec<&str> = arm.to_vec();
        v.extend_from_slice(after);
        join_lines(&v, brk)
    };
    let again = |arm: Vec<String>| -> Result<Vec<String>, Left> {
        let v: Vec<&str> = arm.iter().map(String::as_str).collect();
        join_lines(&v, brk)
    };
    let (t, e) = match (t2, e2) {
        // The `then` arm breaks: the rest goes in the `else`.
        (Some(t), None) => (again(t)?, with_rest(&els_v)?),
        // The `else` arm breaks: the rest goes at the end of `then`.
        (None, Some(e)) if els.is_some() => (with_rest(&then)?, again(e)?),
        // The last statement: each arm falls to the end as it does.
        _ if last => (join_lines(&then, brk)?, join_lines(&els_v, brk)?),
        _ => return Err(Left::InIf),
    };
    let tail = if last { strs(after) } else { Vec::new() };
    if t.is_empty() && !e.is_empty() {
        // Nothing left in `then`: the `if` is on the other arm.
        out.push(format!("if {} {{", negate(cond)));
        out.extend(e);
    } else {
        out.push(head.to_string());
        out.extend(t);
        if !e.is_empty() {
            out.push("} else {".into());
            out.extend(e);
        }
    }
    out.push("}".into());
    out.extend(tail);
    Ok(out)
}

/// A `match`'s arms (its lines between head and closer), each falling
/// to the end of the whole: a multi-line arm's code as [`join_lines`]
/// takes it, a one-line arm's `break` (`PAT => { break 'bL; }`) left
/// out.
fn match_arms(arms: &[&str], brk: &str) -> Result<Vec<String>, Left> {
    let mut out = Vec::new();
    for a in stmts(arms).ok_or(Left::InMatch)? {
        let here = &arms[a.start..a.end];
        if !here.iter().any(|l| l.contains(brk)) {
            out.extend(here.iter().map(|s| s.to_string()));
        } else if let [l] = here {
            let t = l.trim_end();
            let (pat, body) = t
                .strip_suffix('}')
                .and_then(|r| r.rsplit_once("=> {"))
                .ok_or(Left::InMatch)?;
            let inner = body.trim();
            let inner = if inner == brk {
                ""
            } else {
                let r = inner.strip_suffix(brk).map(str::trim_end);
                r.filter(|r| r.ends_with(';')).ok_or(Left::InMatch)?
            };
            out.push(match inner {
                "" => format!("{pat}=> {{}}"),
                _ => format!("{pat}=> {{ {inner} }}"),
            });
        } else if here[0].trim_end().ends_with("=> {") && here[here.len() - 1].trim() == "}" {
            out.push(here[0].to_string());
            out.extend(join_lines(&here[1..here.len() - 1], brk)?);
            out.push(here[here.len() - 1].to_string());
        } else {
            return Err(Left::InMatch);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_into_else() {
        let b = "r3 = 0x14c;\nif c {\n    r3 = 0x1ac;\n    break 'b2;\n}\nr3 = f(r3);\n";
        let s = else_join(b, "break 'b2;").unwrap();
        assert_eq!(
            s,
            "r3 = 0x14c;\nif c {\n    r3 = 0x1ac;\n} else {\nr3 = f(r3);\n}\n"
        );
    }

    #[test]
    fn bare_break_negates() {
        let b = "if !v37 {\n    break 'b18;\n}\nx = 1;\nif v {\n    hw::idle(g)?; break 'b18;\n}\ny = 2;\n";
        let s = else_join(b, "break 'b18;").unwrap();
        assert_eq!(
            s,
            "if v37 {\nx = 1;\nif v {\nhw::idle(g)?;\n} else {\ny = 2;\n}\n}\n"
        );
    }

    #[test]
    fn else_breaks() {
        let b = "if c {\n    a();\n} else {\n    b();\n    break 'b1;\n}\nz();\n";
        let s = else_join(b, "break 'b1;").unwrap();
        assert_eq!(s, "if c {\n    a();\nz();\n} else {\n    b();\n}\n");
    }

    #[test]
    fn nested_and_match_left() {
        let b = "if c {\n    if d {\n        break 'b1;\n    }\n    a();\n}\nz();\n";
        assert_eq!(else_join(b, "break 'b1;"), Err(Left::InIf));
        let m = "match x {\n    1 => { break 'b1; }\n    _ => {}\n}\nz();\n";
        assert_eq!(else_join(m, "break 'b1;"), Err(Left::InMatch));
        let l = "'l3: loop {\n    break 'b1;\n}\n";
        assert_eq!(else_join(l, "break 'b1;"), Err(Left::InLoop));
    }

    #[test]
    fn last_statement_arms() {
        let b = "x();\nif c {\n    if d {\n        break 'b1;\n    }\n    a();\n}\n// end\n";
        let s = else_join(b, "break 'b1;").unwrap();
        assert_eq!(s, "x();\nif c {\nif !d {\n    a();\n}\n}\n// end\n");
        let m = "match x {\n    1 => { f(); break 'b1; }\n    2 => {\n        if c {\n            break 'b1;\n        }\n        g();\n    }\n    _ => {}\n}\n";
        let s = else_join(m, "break 'b1;").unwrap();
        assert_eq!(
            s,
            "match x {\n    1 => { f(); }\n    2 => {\nif !c {\n        g();\n}\n    }\n    _ => {}\n}\n"
        );
        let n = "if c {\n    a();\n    if d {\n        break 'b1;\n    }\n    b();\n    break 'b1;\n}\nz();\n";
        let s = else_join(n, "break 'b1;").unwrap();
        assert_eq!(
            s,
            "if c {\n    a();\nif !d {\n    b();\n}\n} else {\nz();\n}\n"
        );
    }

    #[test]
    fn diverging_arm_left() {
        let b = "if c {\n    break 'b1;\n} else {\n    return Ok(());\n}\nz();\n";
        assert_eq!(else_join(b, "break 'b1;"), Err(Left::Other));
    }

    #[test]
    fn returns() {
        let body = "if c {\n    break 'b7;\n}\nlet v8: u32 = 1;\n";
        let rest = "    pc(0x08000220);\n    // draw.c:20\n    return Ok(());\n";
        assert_eq!(
            return_only(rest, body),
            Some("    pc(0x08000220);\n    // draw.c:20\n    return Ok(());")
        );
        assert_eq!(return_only("    return Ok(v8);\n", body), None);
        assert_eq!(
            return_only("    return Ok(r0);\n", body),
            Some("    return Ok(r0);")
        );
        assert_eq!(return_only("    return Ok(f(g)?);\n", body), None);
        assert_eq!(return_only("    x = 1;\n    return Ok(());\n", body), None);
        assert!(return_only("    return Ok(r1);\n", "for mut r1 in 0..3 {\n}\n").is_none());
        assert!(return_only("    return Ok(r4);\n", "let (mut r4, r5) = (0, 0);\n").is_none());
    }
}
