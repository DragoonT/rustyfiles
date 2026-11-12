//! A small JSON reader and writer, just enough for `shell-state.json`.
//!
//! Why not serde_json: the guard stays tiny (std and the windows crate only). Why not a fixed
//! struct: the host and later versions write more keys than the guard knows; the guard must keep
//! them when it rewrites the file, so it keeps the whole document as a tree and edits only its
//! own keys. Numbers stay as the text they came with, so a 64-bit value is never rounded.

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// The number exactly as written.
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    /// Keys keep their order; a repeated key keeps the first one's place and the last value.
    Obj(Vec<(String, Json)>),
}

const MAX_DEPTH: usize = 24;

impl Json {
    pub fn parse(text: &str) -> Result<Json, String> {
        let mut p = Parser { s: text.as_bytes(), i: 0 };
        p.ws();
        let v = p.value(0)?;
        p.ws();
        if p.i != p.s.len() {
            return Err(format!("trailing characters at byte {}", p.i));
        }
        Ok(v)
    }

    pub fn num(n: u64) -> Json {
        Json::Num(n.to_string())
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(kv) => kv.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Sets a key of an object (replacing the value, keeping its place). Does nothing on a non-object.
    pub fn set(&mut self, key: &str, value: Json) {
        if let Json::Obj(kv) = self {
            // A repeated key (the parser keeps both) is collapsed to one entry.
            let mut found = false;
            kv.retain_mut(|(k, v)| {
                if k != key {
                    return true;
                }
                if found {
                    return false;
                }
                found = true;
                *v = value.clone();
                true
            });
            if !found {
                kv.push((key.to_string(), value));
            }
        }
    }

    pub fn remove(&mut self, key: &str) {
        if let Json::Obj(kv) = self {
            kv.retain(|(k, _)| k != key);
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Json::Num(n) => n.parse().ok(),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Num(n) => n.parse().ok(),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    pub fn is_obj(&self) -> bool {
        matches!(self, Json::Obj(_))
    }

    /// Two-space indented text with a final newline.
    pub fn to_pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out.push('\n');
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => out.push_str(n),
            Json::Str(s) => write_str(out, s),
            Json::Arr(a) if a.is_empty() => out.push_str("[]"),
            Json::Arr(a) => {
                // Short arrays of scalars stay on one line (crash lists, work-area rectangles).
                if a.iter().all(|v| matches!(v, Json::Num(_) | Json::Bool(_) | Json::Null)) && a.len() <= 8 {
                    out.push('[');
                    for (i, v) in a.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        v.write(out, depth + 1);
                    }
                    out.push(']');
                    return;
                }
                out.push_str("[\n");
                for (i, v) in a.iter().enumerate() {
                    indent(out, depth + 1);
                    v.write(out, depth + 1);
                    if i + 1 < a.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                indent(out, depth);
                out.push(']');
            }
            Json::Obj(kv) if kv.is_empty() => out.push_str("{}"),
            Json::Obj(kv) => {
                out.push_str("{\n");
                for (i, (k, v)) in kv.iter().enumerate() {
                    indent(out, depth + 1);
                    write_str(out, k);
                    out.push_str(": ");
                    v.write(out, depth + 1);
                    if i + 1 < kv.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                indent(out, depth);
                out.push('}');
            }
        }
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > MAX_DEPTH {
            return Err("nested too deep".into());
        }
        match self.peek() {
            None => Err("unexpected end".into()),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.word("true", Json::Bool(true)),
            Some(b'f') => self.word("false", Json::Bool(false)),
            Some(b'n') => self.word("null", Json::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            Some(c) => Err(format!("unexpected byte {c:#04x} at {}", self.i)),
        }
    }

    fn word(&mut self, w: &str, v: Json) -> Result<Json, String> {
        if self.s[self.i..].starts_with(w.as_bytes()) {
            self.i += w.len();
            Ok(v)
        } else {
            Err(format!("bad literal at {}", self.i))
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        let digits = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        if self.i == digits {
            return Err(format!("bad number at {start}"));
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.i += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.i += 1;
            }
        }
        // The slice is ASCII digits and signs only.
        Ok(Json::Num(String::from_utf8_lossy(&self.s[start..self.i]).into_owned()))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let mut v = 0u32;
        for _ in 0..4 {
            let c = self.peek().ok_or("short \\u escape")?;
            let d = (c as char).to_digit(16).ok_or("bad \\u escape")?;
            v = v * 16 + d;
            self.i += 1;
        }
        Ok(v)
    }

    fn string(&mut self) -> Result<String, String> {
        self.i += 1; // opening quote
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = self.peek().ok_or("unterminated string")?;
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = self.peek().ok_or("unterminated escape")?;
                    self.i += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            if (0xD800..0xDC00).contains(&hi) {
                                if self.s[self.i..].starts_with(b"\\u") {
                                    self.i += 2;
                                    let lo = self.hex4()?;
                                    if !(0xDC00..0xE000).contains(&lo) {
                                        return Err("bad surrogate pair".into());
                                    }
                                    char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)).ok_or("bad code point")?
                                } else {
                                    return Err("lone surrogate".into());
                                }
                            } else {
                                char::from_u32(hi).ok_or("lone surrogate")?
                            }
                        }
                        _ => return Err("bad escape".into()),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                c if c < 0x20 => return Err("control character in string".into()),
                c => out.push(c),
            }
        }
        String::from_utf8(out).map_err(|_| "invalid UTF-8 in string".to_string())
    }

    fn array(&mut self, depth: usize) -> Result<Json, String> {
        self.i += 1;
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            self.ws();
            items.push(self.value(depth + 1)?);
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(format!("expected , or ] at {}", self.i)),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, String> {
        self.i += 1;
        let mut kv: Vec<(String, Json)> = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Json::Obj(kv));
        }
        loop {
            self.ws();
            if self.peek() != Some(b'"') {
                return Err(format!("expected a key at {}", self.i));
            }
            let k = self.string()?;
            self.ws();
            if self.peek() != Some(b':') {
                return Err(format!("expected : at {}", self.i));
            }
            self.i += 1;
            self.ws();
            let v = self.value(depth + 1)?;
            kv.push((k, v));
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Json::Obj(kv));
                }
                _ => return Err(format!("expected , or }} at {}", self.i)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_unknown_keys_and_big_numbers() {
        let src = r#"{"version": 1, "future": {"a": [1, 2, {"b": null}]}, "big": 18446744073709551615, "s": "a\"b\u00e9\ud83d\ude00"}"#;
        let v = Json::parse(src).unwrap();
        assert_eq!(v.get("big").unwrap().as_u64(), Some(u64::MAX));
        assert_eq!(v.get("s").unwrap().as_str(), Some("a\"bé😀"));
        let again = Json::parse(&v.to_pretty()).unwrap();
        assert_eq!(v, again);
    }

    #[test]
    fn rejects_garbage() {
        for bad in ["", "{", "{\"a\":}", "[1,]", "{\"a\" 1}", "nul", "\"abc", "{\"a\":1} x", "\u{0}", "[\"\\ud800\"]", "-"] {
            assert!(Json::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn deep_nesting_is_refused_not_overflowed() {
        let deep = "[".repeat(5000) + &"]".repeat(5000);
        assert!(Json::parse(&deep).is_err());
    }

    #[test]
    fn set_replaces_in_place_and_collapses_repeats() {
        let mut v = Json::parse(r#"{"a":1,"b":2,"a":3}"#).unwrap();
        assert_eq!(v.get("a").unwrap().as_u64(), Some(3));
        v.set("a", Json::num(9));
        assert_eq!(v, Json::parse(r#"{"a":9,"b":2}"#).unwrap());
        v.set("c", Json::Bool(true));
        v.remove("b");
        assert_eq!(v, Json::parse(r#"{"a":9,"c":true}"#).unwrap());
    }
}
