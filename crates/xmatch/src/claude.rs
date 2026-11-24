//! `claude -p` for `xmatch judge` ([`claude`]), the JSON it answers in, and
//! the seeded shuffle the judge orders its groups by. Moved here from the
//! retired naming loop's crate.

use std::collections::BTreeMap;
use std::process::{Command, Stdio};
use std::time::Instant;
use toml::{Table, Value};

/// SplitMix64: a seeded, reproducible order.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Fisher–Yates.
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = (self.next_u64() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }
}

/// Asks `claude -p` `prompt` under `system`: no tools, no MCP servers,
/// safe mode, no session kept, from an empty folder in the temp dir, cut
/// off after 15 minutes. Returns its answer text and a table of the call's
/// model, effort, tokens, list cost (`cost_usd`), seconds and prompt size.
pub fn claude(
    model: &str,
    effort: Option<&str>,
    system: &str,
    prompt: &str,
) -> Result<(String, Table), String> {
    let dir = std::env::temp_dir().join(format!("gbaa-claude-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    let mut child = Command::new("timeout")
        .arg("900")
        .arg("claude")
        .args(["-p", "--tools", "", "--strict-mcp-config", "--safe-mode"])
        .args(["--no-session-persistence", "--output-format", "json"])
        .args(["--model", model, "--system-prompt", system])
        .args(effort.map(|e| ["--effort", e]).into_iter().flatten())
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("claude: {e}"))?;
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(prompt.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    let o = child.wait_with_output().map_err(|e| e.to_string())?;
    let secs = t0.elapsed().as_secs_f64();
    let text = String::from_utf8_lossy(&o.stdout);
    let j = Json::parse(text.trim()).map_err(|e| {
        format!(
            "exit {:?}, output not JSON ({e}): {} {}",
            o.status.code(),
            text.chars().take(200).collect::<String>(),
            String::from_utf8_lossy(&o.stderr)
                .chars()
                .take(200)
                .collect::<String>()
        )
    })?;
    if j.get("is_error") == Some(&Json::Bool(true)) {
        return Err(format!("claude error: {}", j.str("result").unwrap_or("")));
    }
    let result = j.str("result").unwrap_or("").to_string();
    let u = j.get("usage").cloned().unwrap_or(Json::Null);
    let mut t = Table::new();
    let int = |x: f64| Value::Integer(x as i64);
    t.insert("model".into(), Value::String(model.into()));
    if let Some(e) = effort {
        t.insert("effort".into(), Value::String(e.into()));
    }
    t.insert("input_tokens".into(), int(u.num("input_tokens")));
    t.insert(
        "cache_creation_tokens".into(),
        int(u.num("cache_creation_input_tokens")),
    );
    t.insert(
        "cache_read_tokens".into(),
        int(u.num("cache_read_input_tokens")),
    );
    t.insert("output_tokens".into(), int(u.num("output_tokens")));
    t.insert("cost_usd".into(), Value::Float(j.num("total_cost_usd")));
    t.insert("seconds".into(), Value::Float((secs * 10.0).round() / 10.0));
    t.insert("prompt_bytes".into(), int(prompt.len() as f64));
    Ok((result, t))
}

/// A JSON value: just enough to read `claude -p --output-format json`.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    pub fn parse(s: &str) -> Result<Json, String> {
        let b = s.as_bytes();
        let mut i = 0;
        let v = json_value(b, &mut i)?;
        skip_ws(b, &mut i);
        if i != b.len() {
            return Err(format!("trailing text at {i}"));
        }
        Ok(v)
    }

    pub fn get(&self, k: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.get(k),
            _ => None,
        }
    }

    pub fn num(&self, k: &str) -> f64 {
        match self.get(k) {
            Some(Json::Num(n)) => *n,
            _ => 0.0,
        }
    }

    pub fn str(&self, k: &str) -> Option<&str> {
        match self.get(k) {
            Some(Json::Str(s)) => Some(s),
            _ => None,
        }
    }
}

fn skip_ws(b: &[u8], i: &mut usize) {
    while *i < b.len() && b[*i].is_ascii_whitespace() {
        *i += 1;
    }
}

fn json_value(b: &[u8], i: &mut usize) -> Result<Json, String> {
    skip_ws(b, i);
    let lit = |i: &mut usize, w: &str, v: Json| {
        if b[*i..].starts_with(w.as_bytes()) {
            *i += w.len();
            Ok(v)
        } else {
            Err(format!("bad literal at {i}"))
        }
    };
    match b.get(*i) {
        None => Err("unexpected end".into()),
        Some(b'n') => lit(i, "null", Json::Null),
        Some(b't') => lit(i, "true", Json::Bool(true)),
        Some(b'f') => lit(i, "false", Json::Bool(false)),
        Some(b'"') => json_str(b, i).map(Json::Str),
        Some(b'[') => {
            *i += 1;
            let mut v = Vec::new();
            loop {
                skip_ws(b, i);
                if b.get(*i) == Some(&b']') {
                    *i += 1;
                    return Ok(Json::Arr(v));
                }
                v.push(json_value(b, i)?);
                skip_ws(b, i);
                if b.get(*i) == Some(&b',') {
                    *i += 1;
                }
            }
        }
        Some(b'{') => {
            *i += 1;
            let mut m = BTreeMap::new();
            loop {
                skip_ws(b, i);
                if b.get(*i) == Some(&b'}') {
                    *i += 1;
                    return Ok(Json::Obj(m));
                }
                let k = json_str(b, i)?;
                skip_ws(b, i);
                if b.get(*i) != Some(&b':') {
                    return Err(format!("expected : at {i}"));
                }
                *i += 1;
                m.insert(k, json_value(b, i)?);
                skip_ws(b, i);
                if b.get(*i) == Some(&b',') {
                    *i += 1;
                }
            }
        }
        Some(_) => {
            let s = *i;
            while *i < b.len() && matches!(b[*i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                *i += 1;
            }
            std::str::from_utf8(&b[s..*i])
                .ok()
                .and_then(|t| t.parse().ok())
                .map(Json::Num)
                .ok_or_else(|| format!("bad number at {s}"))
        }
    }
}

fn json_str(b: &[u8], i: &mut usize) -> Result<String, String> {
    if b.get(*i) != Some(&b'"') {
        return Err(format!("expected string at {i}"));
    }
    *i += 1;
    let mut out: Vec<u8> = Vec::new();
    while let Some(&c) = b.get(*i) {
        *i += 1;
        match c {
            b'"' => return String::from_utf8(out).map_err(|e| e.to_string()),
            b'\\' => {
                let e = *b.get(*i).ok_or("unexpected end")?;
                *i += 1;
                match e {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'r' => out.push(b'\r'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'u' => {
                        let hex = |i: &mut usize| {
                            let h = b.get(*i..*i + 4).ok_or("short \\u")?;
                            *i += 4;
                            u32::from_str_radix(std::str::from_utf8(h).unwrap_or("x"), 16)
                                .map_err(|e| e.to_string())
                        };
                        let mut cp = hex(i)?;
                        if (0xd800..0xdc00).contains(&cp) && b[*i..].starts_with(b"\\u") {
                            *i += 2;
                            let lo = hex(i)?;
                            cp =
                                0x10000 + ((cp - 0xd800) << 10) + (lo.wrapping_sub(0xdc00) & 0x3ff);
                        }
                        let ch = char::from_u32(cp).unwrap_or('\u{fffd}');
                        out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                    }
                    other => out.push(other),
                }
            }
            c => out.push(c),
        }
    }
    Err("unterminated string".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_reads_a_claude_result() {
        let j = Json::parse(
            r#"{"usage":{"input_tokens":2,"output_tokens":7},"result":"name = \"x\"\né","is_error":false,"a":[1,-2.5e1,null]}"#,
        )
        .unwrap();
        assert_eq!(j.str("result"), Some("name = \"x\"\né"));
        assert_eq!(j.get("usage").unwrap().num("output_tokens"), 7.0);
        assert_eq!(j.get("is_error"), Some(&Json::Bool(false)));
        assert_eq!(
            j.get("a"),
            Some(&Json::Arr(vec![
                Json::Num(1.0),
                Json::Num(-25.0),
                Json::Null
            ]))
        );
    }
}
