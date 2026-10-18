use crate::Error;

pub const MAX_DEPTH: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn parse(text: &str) -> Result<Value, Error> {
        let mut p = Parser {
            s: text,
            b: text.as_bytes(),
            i: 0,
        };
        p.ws();
        let v = p.value(0)?;
        p.ws();
        if p.i != p.b.len() {
            return p.fail("trailing characters");
        }
        Ok(v)
    }

    pub fn to_json(&self) -> String {
        let mut out = String::new();
        write_value(self, &mut out);
        out
    }

    pub(crate) fn is_scalar(&self) -> bool {
        !matches!(self, Value::Array(_) | Value::Object(_))
    }
}

pub(crate) fn is_number(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if b.get(i) == Some(&b'-') {
        i += 1;
    }
    match b.get(i) {
        Some(b'0') => i += 1,
        Some(b'1'..=b'9') => {
            while matches!(b.get(i), Some(b'0'..=b'9')) {
                i += 1;
            }
        }
        _ => return false,
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        let from = i;
        while matches!(b.get(i), Some(b'0'..=b'9')) {
            i += 1;
        }
        if i == from {
            return false;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let from = i;
        while matches!(b.get(i), Some(b'0'..=b'9')) {
            i += 1;
        }
        if i == from {
            return false;
        }
    }
    i == b.len()
}

pub(crate) fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str("\\u00");
                out.push(hex_digit((c as u32 >> 4) as u8));
                out.push(hex_digit((c as u32 & 15) as u8));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn hex_digit(n: u8) -> char {
    char::from(b"0123456789abcdef"[usize::from(n & 15)])
}

fn write_value(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => out.push_str(n),
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (k, item) in items.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Object(fields) => {
            out.push('{');
            for (k, (key, val)) in fields.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                write_string(out, key);
                out.push(':');
                write_value(val, out);
            }
            out.push('}');
        }
    }
}

pub(crate) fn parse_string_at(s: &str, at: usize) -> Result<(String, usize), Error> {
    let mut p = Parser {
        s,
        b: s.as_bytes(),
        i: at,
    };
    let v = p.string()?;
    Ok((v, p.i))
}

struct Parser<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn fail<T>(&self, what: &'static str) -> Result<T, Error> {
        Err(Error::Syntax { at: self.i, what })
    }

    fn ws(&mut self) {
        while matches!(self.b.get(self.i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, Error> {
        match self.b.get(self.i) {
            None => self.fail("unexpected end"),
            Some(b'{') => self.object(depth + 1),
            Some(b'[') => self.array(depth + 1),
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => self.fail("unexpected character"),
        }
    }

    fn literal(&mut self, word: &'static str, v: Value) -> Result<Value, Error> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            self.fail("bad literal")
        }
    }

    fn number(&mut self) -> Result<Value, Error> {
        let start = self.i;
        let b = self.b;
        let mut i = self.i;
        if b.get(i) == Some(&b'-') {
            i += 1;
        }
        match b.get(i) {
            Some(b'0') => i += 1,
            Some(b'1'..=b'9') => {
                while matches!(b.get(i), Some(b'0'..=b'9')) {
                    i += 1;
                }
            }
            _ => {
                self.i = i;
                return self.fail("bad number");
            }
        }
        if b.get(i) == Some(&b'.') {
            i += 1;
            let from = i;
            while matches!(b.get(i), Some(b'0'..=b'9')) {
                i += 1;
            }
            if i == from {
                self.i = i;
                return self.fail("bad fraction");
            }
        }
        if matches!(b.get(i), Some(b'e' | b'E')) {
            i += 1;
            if matches!(b.get(i), Some(b'+' | b'-')) {
                i += 1;
            }
            let from = i;
            while matches!(b.get(i), Some(b'0'..=b'9')) {
                i += 1;
            }
            if i == from {
                self.i = i;
                return self.fail("bad exponent");
            }
        }
        self.i = i;
        Ok(Value::Number(self.s[start..i].to_string()))
    }

    fn hex4(&mut self) -> Result<u32, Error> {
        let mut n = 0u32;
        for _ in 0..4 {
            let d = match self.b.get(self.i) {
                Some(c @ b'0'..=b'9') => u32::from(c - b'0'),
                Some(c @ b'a'..=b'f') => u32::from(c - b'a' + 10),
                Some(c @ b'A'..=b'F') => u32::from(c - b'A' + 10),
                _ => return self.fail("bad unicode escape"),
            };
            n = n * 16 + d;
            self.i += 1;
        }
        Ok(n)
    }

    fn string(&mut self) -> Result<String, Error> {
        if self.b.get(self.i) != Some(&b'"') {
            return self.fail("expected string");
        }
        self.i += 1;
        let mut out = String::new();
        let mut chunk = self.i;
        loop {
            match self.b.get(self.i) {
                None => return self.fail("unterminated string"),
                Some(b'"') => {
                    out.push_str(&self.s[chunk..self.i]);
                    self.i += 1;
                    return Ok(out);
                }
                Some(c) if *c < 0x20 => return self.fail("control character in string"),
                Some(b'\\') => {
                    out.push_str(&self.s[chunk..self.i]);
                    self.i += 1;
                    let esc = match self.b.get(self.i) {
                        Some(e) => *e,
                        None => return self.fail("unterminated escape"),
                    };
                    self.i += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) {
                                if self.b.get(self.i) != Some(&b'\\')
                                    || self.b.get(self.i + 1) != Some(&b'u')
                                {
                                    return self.fail("lone surrogate");
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return self.fail("lone surrogate");
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else if (0xDC00..0xE000).contains(&hi) {
                                return self.fail("lone surrogate");
                            } else {
                                hi
                            };
                            match char::from_u32(code) {
                                Some(c) => out.push(c),
                                None => return self.fail("bad code point"),
                            }
                        }
                        _ => return self.fail("bad escape"),
                    }
                    chunk = self.i;
                }
                Some(_) => self.i += 1,
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, Error> {
        if depth > MAX_DEPTH {
            return Err(Error::TooDeep);
        }
        self.i += 1;
        let mut items = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b']') {
            self.i += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.ws();
            items.push(self.value(depth)?);
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Value::Array(items));
                }
                _ => return self.fail("expected , or ]"),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, Error> {
        if depth > MAX_DEPTH {
            return Err(Error::TooDeep);
        }
        self.i += 1;
        let mut fields = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b'}') {
            self.i += 1;
            return Ok(Value::Object(fields));
        }
        loop {
            self.ws();
            let key = self.string()?;
            self.ws();
            if self.b.get(self.i) != Some(&b':') {
                return self.fail("expected :");
            }
            self.i += 1;
            self.ws();
            let val = self.value(depth)?;
            fields.push((key, val));
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Value::Object(fields));
                }
                _ => return self.fail("expected , or }"),
            }
        }
    }
}
