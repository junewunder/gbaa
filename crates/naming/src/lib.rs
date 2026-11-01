//! The naming loop (`docs/mod-view-design.md`, "The naming loop"): agents
//! name a sample of a ROM's functions from their readable print, with every
//! pret name hidden, and the names are scored against pret's.
//!
//! This is the part that needs no ROM: subtokens and the score, the seeded
//! sample over a call graph, bottom-up levels, the agent's answer, the
//! `claude -p` call ([`claude`], which `xmatch judge` shares) and the JSON
//! it answers in. `main.rs` prints, asks and reports.

use std::collections::{BTreeMap, BTreeSet};
use std::process::{Command, Stdio};
use std::time::Instant;
use toml::{Table, Value};

#[cfg(test)]
mod tests;

/// A function's size class, by instruction count: under 16, 16–100, over 100.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    Small,
    Mid,
    Large,
}

impl Class {
    pub const ALL: [Class; 3] = [Class::Small, Class::Mid, Class::Large];

    pub fn of(insns: usize) -> Class {
        match insns {
            0..16 => Class::Small,
            16..=100 => Class::Mid,
            _ => Class::Large,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Class::Small => "small (<16)",
            Class::Mid => "mid (16-100)",
            Class::Large => "large (>100)",
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Class::Small => "small",
            Class::Mid => "mid",
            Class::Large => "large",
        }
    }

    pub fn from_tag(s: &str) -> Option<Class> {
        Class::ALL.into_iter().find(|c| c.tag() == s)
    }
}

/// `name` split into lowercase subtokens: on `_` and anything not a letter
/// or digit, at a lower-to-upper change, before the last capital of a run
/// followed by a lowercase letter (`HPBar`: hp, bar), and between letters
/// and digits. `the` and `a` are dropped.
/// `Task_HandleChooseMonInput` is {task, handle, choose, mon, input}.
pub fn subtokens(name: &str) -> Vec<String> {
    let cs: Vec<char> = name.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    for (i, &c) in cs.iter().enumerate() {
        if !c.is_ascii_alphanumeric() {
            out.push(std::mem::take(&mut cur));
            continue;
        }
        if let Some(&p) = i.checked_sub(1).and_then(|j| cs.get(j)) {
            let next_lower = cs.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
            let split = (p.is_ascii_lowercase() && c.is_ascii_uppercase())
                || (p.is_ascii_uppercase() && c.is_ascii_uppercase() && next_lower)
                || (p.is_ascii_digit() != c.is_ascii_digit() && p.is_ascii_alphanumeric());
            if split {
                out.push(std::mem::take(&mut cur));
            }
        }
        cur.push(c.to_ascii_lowercase());
    }
    out.push(cur);
    out.retain(|t| !t.is_empty() && t != "the" && t != "a");
    out
}

/// A datum's subtokens with a scope prefix dropped: `gPlayerParty` and
/// `sPlayerParty` are both {player, party}.
pub fn data_subtokens(name: &str) -> Vec<String> {
    let b = name.as_bytes();
    let scoped = b.len() > 1 && matches!(b[0], b'g' | b's') && b[1].is_ascii_uppercase();
    subtokens(if scoped { &name[1..] } else { name })
}

/// One prediction's precision and recall against the truth, as subtoken
/// multisets: matched over predicted, matched over true. An empty side
/// scores 0.
pub fn pr(pred: &[String], truth: &[String]) -> (f64, f64) {
    let mut left: BTreeMap<&str, usize> = BTreeMap::new();
    for t in truth {
        *left.entry(t).or_default() += 1;
    }
    let mut hit = 0;
    for p in pred {
        if let Some(n) = left.get_mut(p.as_str())
            && *n > 0
        {
            *n -= 1;
            hit += 1;
        }
    }
    let div = |a: usize, b: usize| if b == 0 { 0.0 } else { a as f64 / b as f64 };
    (div(hit, pred.len()), div(hit, truth.len()))
}

/// Exact match, case-insensitive.
pub fn exact(pred: &str, truth: &str) -> bool {
    pred.eq_ignore_ascii_case(truth)
}

/// Scores summed over a set of predictions: macro precision and recall
/// (the mean per prediction), F1 their harmonic mean, and exact matches.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Score {
    pub n: usize,
    pub p: f64,
    pub r: f64,
    pub exact: usize,
}

impl Score {
    pub fn add(&mut self, pred: &[String], truth: &[String], is_exact: bool) {
        let (p, r) = pr(pred, truth);
        self.n += 1;
        self.p += p;
        self.r += r;
        self.exact += usize::from(is_exact);
    }

    pub fn precision(&self) -> f64 {
        self.p / self.n.max(1) as f64
    }

    pub fn recall(&self) -> f64 {
        self.r / self.n.max(1) as f64
    }

    pub fn f1(&self) -> f64 {
        let (p, r) = (self.precision(), self.recall());
        if p + r == 0.0 {
            0.0
        } else {
            2.0 * p * r / (p + r)
        }
    }

    pub fn exact_share(&self) -> f64 {
        self.exact as f64 / self.n.max(1) as f64
    }
}

/// One instruction's raw bits with what moves between two builds of the
/// same code masked: a Thumb BL half's offset, an ARM B or BL's. Branches
/// inside a function and pc-relative pool loads stay, as they move with it.
pub fn mask_insn(raw: u32, thumb: bool) -> u32 {
    if thumb {
        if raw & 0xf000 == 0xf000 {
            raw & 0xf800
        } else {
            raw
        }
    } else if raw & 0x0e00_0000 == 0x0a00_0000 {
        raw & 0xff00_0000
    } else {
        raw
    }
}

/// Functions of two games with the same code (the same masked bytes, each
/// fingerprint unique in its own game) whose names differ, as (key in
/// `a`, key in `b`). Names compare case-insensitively.
pub fn sibling_pairs(
    a: &BTreeMap<u32, (Vec<u8>, String)>,
    b: &BTreeMap<u32, (Vec<u8>, String)>,
) -> Vec<(u32, u32)> {
    fn unique(m: &BTreeMap<u32, (Vec<u8>, String)>) -> BTreeMap<&[u8], u32> {
        let mut seen: BTreeMap<&[u8], Option<u32>> = BTreeMap::new();
        for (&k, (fp, _)) in m {
            seen.entry(fp).and_modify(|v| *v = None).or_insert(Some(k));
        }
        seen.into_iter()
            .filter_map(|(fp, k)| Some((fp, k?)))
            .collect()
    }
    let ub = unique(b);
    unique(a)
        .into_iter()
        .filter_map(|(fp, ka)| {
            let kb = *ub.get(fp)?;
            (!a[&ka].1.eq_ignore_ascii_case(&b[&kb].1)).then_some((ka, kb))
        })
        .collect()
}

/// SplitMix64: the sample's seeded generator.
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

/// What the sample is drawn from: each candidate's instruction count and
/// the candidates it calls.
pub struct Graph {
    pub insns: BTreeMap<u32, usize>,
    pub calls: BTreeMap<u32, BTreeSet<u32>>,
}

impl Graph {
    /// `root` and every candidate it reaches by calls.
    pub fn subtree(&self, root: u32) -> BTreeSet<u32> {
        let mut seen = BTreeSet::from([root]);
        let mut todo = vec![root];
        while let Some(k) = todo.pop() {
            for &c in self.calls.get(&k).into_iter().flatten() {
                if self.insns.contains_key(&c) && seen.insert(c) {
                    todo.push(c);
                }
            }
        }
        seen
    }
}

/// How the sample is drawn.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub seed: u64,
    /// Functions per size class.
    pub per_class: usize,
    pub roots: usize,
    /// A root's subtree holds from 3 to this many functions.
    pub max_subtree: usize,
}

impl Default for Plan {
    fn default() -> Plan {
        Plan {
            seed: 20261027,
            per_class: 100,
            roots: 30,
            max_subtree: 12,
        }
    }
}

/// The sample: every key drawn, the roots whose whole subtrees are in it
/// (each with its subtree), and the rest drawn at random.
#[derive(Debug, Default, PartialEq)]
pub struct Sample {
    pub keys: BTreeSet<u32>,
    pub roots: BTreeMap<u32, BTreeSet<u32>>,
}

/// Draws `plan.roots` whole subtrees (a root and its callees down to the
/// leaves, 3 to `plan.max_subtree` functions, none pushing a class past
/// `plan.per_class`), then fills each class to `plan.per_class` at random.
pub fn draw(g: &Graph, plan: &Plan) -> Sample {
    let mut rng = Rng::new(plan.seed);
    let class = |k: &u32| Class::of(g.insns[k]);
    let mut keys: BTreeSet<u32> = BTreeSet::new();
    let mut roots = BTreeMap::new();
    let mut cands: Vec<u32> = g.insns.keys().copied().collect();
    rng.shuffle(&mut cands);
    for &r in &cands {
        if roots.len() == plan.roots {
            break;
        }
        let t = g.subtree(r);
        if !(3..=plan.max_subtree).contains(&t.len()) {
            continue;
        }
        let mut count: BTreeMap<Class, usize> = BTreeMap::new();
        for k in keys.union(&t) {
            *count.entry(class(k)).or_default() += 1;
        }
        if count.values().any(|&n| n > plan.per_class) {
            continue;
        }
        keys.extend(&t);
        roots.insert(r, t);
    }
    for c in Class::ALL {
        let mut have = keys.iter().filter(|k| class(k) == c).count();
        for &k in &cands {
            if have >= plan.per_class {
                break;
            }
            if class(&k) == c && keys.insert(k) {
                have += 1;
            }
        }
    }
    Sample { keys, roots }
}

/// The sample in bottom-up levels: a function's level is one more than its
/// sampled callees' highest (0 with none); a call back up a cycle is
/// ignored. Each level can be asked at once.
pub fn levels(keys: &BTreeSet<u32>, calls: &BTreeMap<u32, BTreeSet<u32>>) -> Vec<Vec<u32>> {
    fn visit(
        k: u32,
        keys: &BTreeSet<u32>,
        calls: &BTreeMap<u32, BTreeSet<u32>>,
        on: &mut BTreeSet<u32>,
        lv: &mut BTreeMap<u32, usize>,
    ) -> usize {
        if let Some(&l) = lv.get(&k) {
            return l;
        }
        on.insert(k);
        let mut l = 0;
        for &c in calls.get(&k).into_iter().flatten() {
            if keys.contains(&c) && c != k && !on.contains(&c) {
                l = l.max(1 + visit(c, keys, calls, on, lv));
            }
        }
        on.remove(&k);
        lv.insert(k, l);
        l
    }
    let mut lv = BTreeMap::new();
    for &k in keys {
        visit(k, keys, calls, &mut BTreeSet::new(), &mut lv);
    }
    let mut out: Vec<Vec<u32>> = Vec::new();
    for (k, l) in lv {
        if out.len() <= l {
            out.resize(l + 1, Vec::new());
        }
        out[l].push(k);
    }
    out
}

/// An agent's answer for one function.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Answer {
    pub name: String,
    pub summary: String,
    /// Names for the data it touches, by address.
    pub data: BTreeMap<u32, String>,
}

/// The answer in `text`, TOML (`name`, `summary`, `[data]` keyed by
/// `d_<hex>`), with any code fence around it dropped.
pub fn parse_answer(text: &str) -> Result<Answer, String> {
    let body = strip_fence(text);
    let t: toml::Table = body.parse().map_err(|e| format!("not TOML: {e}"))?;
    let s = |k: &str| {
        t.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let name = s("name");
    if name.is_empty() {
        return Err("no name".into());
    }
    let mut data = BTreeMap::new();
    if let Some(d) = t.get("data").and_then(|v| v.as_table()) {
        for (k, v) in d {
            let (Some(hex), Some(n)) = (k.strip_prefix("d_"), v.as_str()) else {
                continue;
            };
            if let Ok(a) = u32::from_str_radix(hex, 16) {
                data.insert(a, n.trim().to_string());
            }
        }
    }
    Ok(Answer {
        name,
        summary: s("summary"),
        data,
    })
}

/// The TOML in an answer: the last fenced block if it has any (an agent
/// that corrects itself writes a second), else all of it.
fn strip_fence(text: &str) -> &str {
    let t = text.trim();
    let parts: Vec<&str> = t.split("```").collect();
    if parts.len() < 3 {
        return t;
    }
    // Fenced blocks are the odd parts; the last whole one is before the
    // last fence that closes.
    let last = if parts.len() % 2 == 1 {
        parts.len() - 2
    } else {
        parts.len() - 3
    };
    let block = parts[last];
    block.split_once('\n').map_or("", |(lang, rest)| {
        if lang.trim().chars().all(|c| c.is_ascii_alphanumeric()) {
            rest
        } else {
            block
        }
    })
}

/// The most common name each datum was given, ties to the first in
/// sorted order.
pub fn vote<'a>(answers: impl IntoIterator<Item = &'a Answer>) -> BTreeMap<u32, String> {
    let mut count: BTreeMap<u32, BTreeMap<&str, usize>> = BTreeMap::new();
    for a in answers {
        for (&d, n) in &a.data {
            *count.entry(d).or_default().entry(n).or_default() += 1;
        }
    }
    count
        .into_iter()
        .map(|(d, ns)| {
            let best = ns.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)));
            (d, best.map(|(n, _)| n.to_string()).unwrap_or_default())
        })
        .collect()
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
    let dir = std::env::temp_dir().join(format!("gbaa-naming-{}", std::process::id()));
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
