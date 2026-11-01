//! mGBA 0.11-dev's `storage` (`src/script/storage.c` at fd9dc5c):
//! `storage:getBucket(name)` gives a bucket, a table kept as
//! `<dir>/<name>.json` (mGBA's dir: its config folder's `storage/`), with
//! `reload`, `flush` and `enableAutoFlush`; `storage:flushAll()` flushes
//! every bucket that autoflushes.
//!
//! As in mGBA: a bucket's name is `[0-9A-Za-z_][0-9A-Za-z_.]*`, at most 64
//! long, else `getBucket` gives nil; setting a field copies the value in
//! (numbers, strings, booleans, and tables of those keyed by names or
//! 1..n, which become JSON arrays); reading one gives the stored table
//! itself, so changing it in place changes the bucket. mGBA flushes every
//! autoflushing bucket on a 5 ms timer; the host does at every
//! [`FLUSH_EVERY`]th frame, writing a file only when its text changed, and
//! at the end ([`Storage::shutdown`]), when, as mGBA's bucket teardown
//! does, a bucket set since its last flush is written even with autoflush
//! off.
//!
//! Unlike mGBA, a value it can't store (a function, a table holding
//! itself, a table keyed by anything else) is refused when it's set, with
//! an error, rather than making every later flush fail.

use mlua::{Lua, MetaMethod, Table, UserData, UserDataMethods, UserDataRef, Value, Variadic};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

/// Frames between the host's flushes of autoflushing buckets.
pub const FLUSH_EVERY: u64 = 30;
/// `STORAGE_LEN_MAX`.
const NAME_MAX: usize = 64;

/// The `storage` global's state: the folder and the buckets got so far.
#[derive(Default)]
pub struct Storage {
    dir: RefCell<Option<PathBuf>>,
    buckets: RefCell<BTreeMap<String, Rc<RefCell<Bucket>>>>,
}

struct Bucket {
    name: String,
    root: Table,
    autoflush: bool,
    /// Set since the last flush or reload.
    dirty: bool,
    /// The text last read or written, so an unchanged bucket isn't
    /// rewritten.
    written: Option<Vec<u8>>,
}

impl Storage {
    /// Keep buckets in `dir` (made at the first flush); with none, buckets
    /// live only in memory and `flush` and `reload` give false.
    pub fn set_dir(&self, dir: Option<PathBuf>) {
        *self.dir.borrow_mut() = dir;
    }

    fn path(&self, name: &str) -> Option<PathBuf> {
        Some(self.dir.borrow().as_ref()?.join(format!("{name}.json")))
    }

    /// `mScriptStorageGetBucket`.
    fn bucket(&self, lua: &Lua, name: &str) -> mlua::Result<Option<Rc<RefCell<Bucket>>>> {
        let ok = name.len() <= NAME_MAX
            && name
                .bytes()
                .enumerate()
                .all(|(i, c)| c.is_ascii_alphanumeric() || c == b'_' || (i > 0 && c == b'.'));
        if !ok {
            return Ok(None);
        }
        if let Some(b) = self.buckets.borrow().get(name) {
            return Ok(Some(b.clone()));
        }
        let b = Rc::new(RefCell::new(Bucket {
            name: name.to_owned(),
            root: lua.create_table()?,
            autoflush: true,
            dirty: false,
            written: None,
        }));
        self.reload(lua, &mut b.borrow_mut())?;
        self.buckets.borrow_mut().insert(name.to_owned(), b.clone());
        Ok(Some(b))
    }

    /// `reload`: the bucket as its file has it, if that reads as a JSON
    /// object.
    fn reload(&self, lua: &Lua, b: &mut Bucket) -> mlua::Result<bool> {
        let Some(text) = self.path(&b.name).and_then(|p| std::fs::read(p).ok()) else {
            return Ok(false);
        };
        let Some(j @ J::Obj(_)) = parse(&text) else {
            return Ok(false);
        };
        let Value::Table(t) = to_lua(lua, &j)? else {
            unreachable!("an object is a table");
        };
        b.root = t;
        b.dirty = false;
        b.written = Some(text);
        Ok(true)
    }

    /// `flush`; unless `force`, only if the text changed.
    fn flush(&self, b: &mut Bucket, force: bool) -> bool {
        let Ok(j) = from_lua(&Value::Table(b.root.clone()), &mut vec![]) else {
            return false;
        };
        let mut text = Vec::new();
        write(&j, 0, &mut text);
        if !force && b.written.as_ref() == Some(&text) {
            b.dirty = false;
            return true;
        }
        let Some(path) = self.path(&b.name) else {
            return false;
        };
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if std::fs::write(&path, &text).is_err() {
            return false;
        }
        b.dirty = false;
        b.written = Some(text);
        true
    }

    /// The host's timer: each autoflushing bucket whose text changed.
    pub fn flush_changed(&self) {
        for b in self.buckets.borrow().values() {
            let mut b = b.borrow_mut();
            if b.autoflush {
                self.flush(&mut b, false);
            }
        }
    }

    /// At the end: the autoflushing buckets, and any set since its last
    /// flush (mGBA's `mScriptStorageBucketDeinit`).
    pub fn shutdown(&self) {
        for b in self.buckets.borrow().values() {
            let mut b = b.borrow_mut();
            if b.autoflush || b.dirty {
                self.flush(&mut b, false);
            }
        }
    }
}

/// The `storage` global.
pub struct StorageRef(pub Rc<Storage>);

impl UserData for StorageRef {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("getBucket", |lua, s, name: Option<String>| {
            let Some(name) = name else {
                return Ok(None);
            };
            Ok(s.0.bucket(lua, &name)?.map(|b| BucketRef(s.0.clone(), b)))
        });
        m.add_method("flushAll", |_, s, ()| {
            for b in s.0.buckets.borrow().values() {
                let mut b = b.borrow_mut();
                if b.autoflush {
                    s.0.flush(&mut b, true);
                }
            }
            Ok(())
        });
    }
}

/// A bucket, as scripts hold it.
struct BucketRef(Rc<Storage>, Rc<RefCell<Bucket>>);

/// Lua's truth, for `enableAutoFlush`'s argument as mGBA coerces it.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Nil | Value::Boolean(false) => false,
        Value::Integer(n) => *n != 0,
        Value::Number(f) => *f != 0.0,
        _ => true,
    }
}

impl UserData for BucketRef {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        // Methods first, then the stored fields, as mGBA's struct lookup.
        m.add_meta_method(MetaMethod::Index, |lua, b, key: Value| {
            if let Value::String(s) = &key
                && let Ok(name) = s.to_str()
                && matches!(&*name, "reload" | "flush" | "enableAutoFlush")
            {
                return method(lua, &name);
            }
            let key = match key {
                Value::Integer(n) => n.to_string(),
                Value::Number(f) => f.to_string(),
                Value::String(s) => s.to_str()?.to_owned(),
                _ => return Ok(Value::Nil),
            };
            b.1.borrow().root.get(key)
        });
        m.add_meta_method(MetaMethod::NewIndex, |lua, b, (key, v): (String, Value)| {
            let j = from_lua(&v, &mut vec![]).map_err(|what| {
                mlua::Error::runtime(format!("storage: can't store {what} in {key:?}"))
            })?;
            let mut b = b.1.borrow_mut();
            b.root.set(key, to_lua(lua, &j)?)?;
            b.dirty = true;
            Ok(())
        });
        m.add_meta_method(MetaMethod::ToString, |_, b, _: Variadic<Value>| {
            Ok(format!("bucket {}", b.1.borrow().name))
        });
    }
}

/// A bucket's method, by name.
fn method(lua: &Lua, name: &str) -> mlua::Result<Value> {
    let f = match name {
        "reload" => lua.create_function(|lua, b: UserDataRef<BucketRef>| {
            b.0.reload(lua, &mut b.1.borrow_mut())
        })?,
        "flush" => lua.create_function(|_, b: UserDataRef<BucketRef>| {
            Ok(b.0.flush(&mut b.1.borrow_mut(), true))
        })?,
        _ => lua.create_function(|_, (b, on): (UserDataRef<BucketRef>, Value)| {
            b.1.borrow_mut().autoflush = truthy(&on);
            Ok(())
        })?,
    };
    Ok(Value::Function(f))
}

/// What a bucket holds, as JSON has it.
#[derive(Debug, PartialEq)]
enum J {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Vec<u8>),
    List(Vec<J>),
    /// By key, sorted, so a bucket's file is the same each time.
    Obj(Vec<(Vec<u8>, J)>),
}

/// `v` as a bucket holds it, or what it is that can't be held. `seen`:
/// the tables we're inside.
fn from_lua(v: &Value, seen: &mut Vec<usize>) -> Result<J, String> {
    Ok(match v {
        Value::Nil => J::Null,
        Value::Boolean(b) => J::Bool(*b),
        Value::Integer(n) => J::Int(*n),
        Value::Number(f) => J::Float(*f),
        Value::String(s) => J::Str(s.as_bytes().to_vec()),
        Value::Table(t) => {
            let p = t.to_pointer() as usize;
            if seen.contains(&p) {
                return Err("a table that contains itself".into());
            }
            seen.push(p);
            let mut ints = BTreeMap::new();
            let mut names = BTreeMap::new();
            for pair in t.pairs::<Value, Value>() {
                let (k, v) = pair.map_err(|e| e.to_string())?;
                let v = from_lua(&v, seen)?;
                match k {
                    Value::Integer(n) => {
                        ints.insert(n, v);
                    }
                    Value::String(s) => {
                        names.insert(s.as_bytes().to_vec(), v);
                    }
                    k => return Err(format!("a table keyed by a {}", k.type_name())),
                }
            }
            seen.pop();
            match (ints.is_empty(), names.is_empty()) {
                (true, _) => J::Obj(names.into_iter().collect()),
                (false, true) if ints.keys().copied().eq(1..=ints.len() as i64) => {
                    J::List(ints.into_values().collect())
                }
                _ => {
                    return Err(
                        "a table keyed by numbers other than 1..n (a list's) or by both numbers and names"
                            .into(),
                    );
                }
            }
        }
        v => return Err(format!("a {}", v.type_name())),
    })
}

/// A fresh Lua value holding `j`.
fn to_lua(lua: &Lua, j: &J) -> mlua::Result<Value> {
    Ok(match j {
        J::Null => Value::Nil,
        J::Bool(b) => Value::Boolean(*b),
        J::Int(n) => Value::Integer(*n),
        J::Float(f) => Value::Number(*f),
        J::Str(s) => Value::String(lua.create_string(s)?),
        J::List(l) => {
            let t = lua.create_table_with_capacity(l.len(), 0)?;
            for (i, v) in l.iter().enumerate() {
                t.raw_set(i + 1, to_lua(lua, v)?)?;
            }
            Value::Table(t)
        }
        J::Obj(o) => {
            let t = lua.create_table_with_capacity(0, o.len())?;
            for (k, v) in o {
                t.raw_set(lua.create_string(k)?, to_lua(lua, v)?)?;
            }
            Value::Table(t)
        }
    })
}

fn tabs(n: usize, out: &mut Vec<u8>) {
    out.extend(std::iter::repeat_n(b'\t', n));
}

fn write_str(s: &[u8], out: &mut Vec<u8>) {
    out.push(b'"');
    for &c in s {
        match c {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            0..0x20 | 0x7F => out.extend_from_slice(format!("\\u{c:04x}").as_bytes()),
            // Other bytes as they are, UTF-8 or not, as json-c writes them.
            c => out.push(c),
        }
    }
    out.push(b'"');
}

/// `j` as json-c's `JSON_C_TO_STRING_PRETTY | PRETTY_TAB` writes it.
fn write(j: &J, depth: usize, out: &mut Vec<u8>) {
    match j {
        J::Null => out.extend_from_slice(b"null"),
        J::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        J::Int(n) => out.extend_from_slice(n.to_string().as_bytes()),
        J::Float(f) if f.is_nan() => out.extend_from_slice(b"NaN"),
        J::Float(f) if f.is_infinite() => {
            out.extend_from_slice(if *f > 0.0 { b"Infinity" } else { b"-Infinity" })
        }
        // Rust's shortest form that reads back the same, with a ".0" when
        // whole, so it reads back a float.
        J::Float(f) => out.extend_from_slice(format!("{f:?}").as_bytes()),
        J::Str(s) => write_str(s, out),
        J::List(l) if l.is_empty() => out.extend_from_slice(b"[]"),
        J::Obj(o) if o.is_empty() => out.extend_from_slice(b"{}"),
        J::List(l) => {
            out.extend_from_slice(b"[\n");
            for (i, v) in l.iter().enumerate() {
                if i > 0 {
                    out.extend_from_slice(b",\n");
                }
                tabs(depth + 1, out);
                write(v, depth + 1, out);
            }
            out.push(b'\n');
            tabs(depth, out);
            out.push(b']');
        }
        J::Obj(o) => {
            out.extend_from_slice(b"{\n");
            for (i, (k, v)) in o.iter().enumerate() {
                if i > 0 {
                    out.extend_from_slice(b",\n");
                }
                tabs(depth + 1, out);
                write_str(k, out);
                out.push(b':');
                write(v, depth + 1, out);
            }
            out.push(b'\n');
            tabs(depth, out);
            out.push(b'}');
        }
    }
}

/// JSON text as a value, or None if it isn't JSON (json-c's `NaN` and
/// `Infinity` taken too).
fn parse(text: &[u8]) -> Option<J> {
    let mut p = Parser { s: text, i: 0 };
    let j = p.value(0)?;
    p.ws();
    (p.i == text.len()).then_some(j)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn eat(&mut self, word: &[u8]) -> bool {
        let ok = self.s[self.i..].starts_with(word);
        if ok {
            self.i += word.len();
        }
        ok
    }

    fn value(&mut self, depth: usize) -> Option<J> {
        if depth > 200 {
            return None;
        }
        self.ws();
        Some(match *self.s.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut o = BTreeMap::new();
                self.ws();
                if !self.eat(b"}") {
                    loop {
                        self.ws();
                        let k = self.string()?;
                        self.ws();
                        if !self.eat(b":") {
                            return None;
                        }
                        o.insert(k, self.value(depth + 1)?);
                        self.ws();
                        if self.eat(b"}") {
                            break;
                        }
                        if !self.eat(b",") {
                            return None;
                        }
                    }
                }
                J::Obj(o.into_iter().collect())
            }
            b'[' => {
                self.i += 1;
                let mut l = vec![];
                self.ws();
                if !self.eat(b"]") {
                    loop {
                        l.push(self.value(depth + 1)?);
                        self.ws();
                        if self.eat(b"]") {
                            break;
                        }
                        if !self.eat(b",") {
                            return None;
                        }
                    }
                }
                J::List(l)
            }
            b'"' => J::Str(self.string()?),
            _ if self.eat(b"null") => J::Null,
            _ if self.eat(b"true") => J::Bool(true),
            _ if self.eat(b"false") => J::Bool(false),
            _ if self.eat(b"NaN") => J::Float(f64::NAN),
            _ if self.eat(b"Infinity") => J::Float(f64::INFINITY),
            _ if self.eat(b"-Infinity") => J::Float(f64::NEG_INFINITY),
            _ => self.number()?,
        })
    }

    fn number(&mut self) -> Option<J> {
        let start = self.i;
        while self
            .s
            .get(self.i)
            .is_some_and(|c| c.is_ascii_digit() || b"+-.eE".contains(c))
        {
            self.i += 1;
        }
        let t = std::str::from_utf8(&self.s[start..self.i]).ok()?;
        if t.is_empty() {
            return None;
        }
        if !t.contains(['.', 'e', 'E'])
            && let Ok(n) = t.parse()
        {
            return Some(J::Int(n));
        }
        t.parse().ok().map(J::Float)
    }

    fn hex4(&mut self) -> Option<u32> {
        let h = std::str::from_utf8(self.s.get(self.i..self.i + 4)?).ok()?;
        self.i += 4;
        u32::from_str_radix(h, 16).ok()
    }

    fn string(&mut self) -> Option<Vec<u8>> {
        if !self.eat(b"\"") {
            return None;
        }
        let mut out = vec![];
        loop {
            let c = *self.s.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return Some(out),
                b'\\' => {
                    let e = *self.s.get(self.i)?;
                    self.i += 1;
                    match e {
                        b'"' | b'\\' | b'/' => out.push(e),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let mut u = self.hex4()?;
                            if (0xD800..0xDC00).contains(&u) && self.eat(b"\\u") {
                                let lo = self.hex4()?;
                                if (0xDC00..0xE000).contains(&lo) {
                                    u = 0x10000 + ((u - 0xD800) << 10) + (lo - 0xDC00);
                                }
                            }
                            let ch = char::from_u32(u).unwrap_or('\u{FFFD}');
                            out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                        }
                        _ => return None,
                    }
                }
                c => out.push(c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trips() {
        let j = J::Obj(vec![
            (b"a".to_vec(), J::Int(-3)),
            (b"b".to_vec(), J::Float(0.5)),
            (b"c".to_vec(), J::Float(2.0)),
            (b"d".to_vec(), J::Str(b"q\"\\\n\x01\xff".to_vec())),
            (
                b"e".to_vec(),
                J::List(vec![J::Bool(true), J::Null, J::Obj(vec![])]),
            ),
            (b"f".to_vec(), J::List(vec![])),
        ]);
        let mut text = vec![];
        write(&j, 0, &mut text);
        assert_eq!(
            String::from_utf8_lossy(&text),
            "{\n\t\"a\":-3,\n\t\"b\":0.5,\n\t\"c\":2.0,\n\t\"d\":\"q\\\"\\\\\\n\\u0001\u{FFFD}\",\n\t\"e\":[\n\t\ttrue,\n\t\tnull,\n\t\t{}\n\t],\n\t\"f\":[]\n}"
        );
        assert_eq!(parse(&text), Some(j));
        assert_eq!(
            parse(br#" {"x": "\u00e9\ud83d\ude00\/", "y": 1e2, "z": -0} "#),
            Some(J::Obj(vec![
                (b"x".to_vec(), J::Str("é😀/".as_bytes().to_vec())),
                (b"y".to_vec(), J::Float(100.0)),
                (b"z".to_vec(), J::Int(0)),
            ]))
        );
        for bad in [&b""[..], b"{", b"{\"a\":}", b"[1,]", b"{} x", b"{'a':1}"] {
            assert_eq!(parse(bad), None, "{:?}", String::from_utf8_lossy(bad));
        }
    }
}
