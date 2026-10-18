use std::collections::HashMap;

use super::rules::{classify, Scalar, KEY_STOP};
use crate::json::{parse_string_at, MAX_DEPTH};
use crate::{Error, Value};

struct Line<'a> {
    indent: usize,
    text: &'a str,
    off: usize,
}

type Cols = Vec<(String, Option<Cell>)>;

struct Cell {
    text: String,
    quoted: bool,
}

pub fn decode(text: &str) -> Result<Value, Error> {
    if let Some(rest) = text.strip_prefix('!') {
        return Value::parse(rest);
    }
    let body = text.strip_suffix('\n').unwrap_or(text);
    let mut lines = Vec::new();
    let mut off = 0;
    for raw in body.split('\n') {
        let t = raw.trim_start_matches(' ');
        lines.push(Line {
            indent: raw.len() - t.len(),
            text: t,
            off,
        });
        off += raw.len() + 1;
    }
    let mut d = Decoder {
        lines,
        pos: 0,
        dict: HashMap::new(),
    };
    d.dictionary()?;
    let value = match d.lines.get(d.pos) {
        None => return d.fail("empty document"),
        Some(l) if l.indent != 0 => return d.fail("unexpected indent"),
        Some(l) if l.text.starts_with('[') => {
            let rest = l.text;
            d.array(0, 0, rest)?
        }
        Some(_) => d.object(0, 1)?,
    };
    if d.pos != d.lines.len() {
        return d.fail("trailing lines");
    }
    Ok(value)
}

struct Decoder<'a> {
    lines: Vec<Line<'a>>,
    pos: usize,
    dict: HashMap<String, String>,
}

fn enter(level: usize) -> Result<usize, Error> {
    if level >= MAX_DEPTH {
        Err(Error::TooDeep)
    } else {
        Ok(level + 1)
    }
}

impl<'a> Decoder<'a> {
    fn fail<T>(&self, what: &'static str) -> Result<T, Error> {
        let at = self.lines.get(self.pos).map_or(0, |l| l.off);
        Err(Error::Syntax { at, what })
    }

    fn line(&self) -> Result<&Line<'a>, Error> {
        match self.lines.get(self.pos) {
            Some(l) if !l.text.is_empty() => Ok(l),
            Some(_) => self.fail("blank line"),
            None => self.fail("unexpected end"),
        }
    }

    fn dictionary(&mut self) -> Result<(), Error> {
        while let Some(l) = self.lines.get(self.pos) {
            if l.indent != 0 || !l.text.starts_with('$') {
                break;
            }
            let id_len = 1 + l.text[1..]
                .find(|c: char| !c.is_ascii_alphanumeric())
                .unwrap_or(l.text.len() - 1);
            if !l.text[id_len..].starts_with('=') {
                break;
            }
            let (id, token) = (&l.text[..id_len], &l.text[id_len + 1..]);
            let (key, rest) = self.key_from(token, false)?;
            if !rest.is_empty() || self.dict.insert(id.to_string(), key).is_some() {
                return self.fail("bad dictionary line");
            }
            self.pos += 1;
        }
        Ok(())
    }

    fn key_from<'t>(&self, text: &'t str, refs: bool) -> Result<(String, &'t str), Error> {
        if text.starts_with('"') {
            let (s, end) = parse_string_at(text, 0)?;
            return Ok((s, &text[end..]));
        }
        let end = text.find(KEY_STOP).unwrap_or(text.len());
        let word = &text[..end];
        if word.is_empty() {
            return self.fail("empty key");
        }
        if word.starts_with('$') {
            if !refs {
                return Ok((word.to_string(), &text[end..]));
            }
            return match self.dict.get(word) {
                Some(k) => Ok((k.clone(), &text[end..])),
                None => self.fail("unknown dictionary reference"),
            };
        }
        Ok((word.to_string(), &text[end..]))
    }

    fn object(&mut self, ind: usize, level: usize) -> Result<Value, Error> {
        let mut fields = Vec::new();
        while let Some(l) = self.lines.get(self.pos) {
            if l.indent != ind {
                break;
            }
            let text = self.line()?.text;
            let (key, rest) = self.key_from(text, true)?;
            let value = self.after_key(ind, level, rest)?;
            fields.push((key, value));
        }
        if self.lines.get(self.pos).is_some_and(|l| l.indent > ind) {
            return self.fail("unexpected indent");
        }
        Ok(Value::Object(fields))
    }

    fn after_key(&mut self, ind: usize, level: usize, rest: &str) -> Result<Value, Error> {
        if rest.starts_with('[') {
            return self.array(ind, level, rest);
        }
        let Some(r) = rest.strip_prefix(':') else {
            return self.fail("expected :");
        };
        if r.is_empty() {
            let child = enter(level)?;
            self.pos += 1;
            return match self.lines.get(self.pos) {
                Some(l) if l.indent == ind + 1 => self.object(ind + 1, child),
                _ => self.fail("missing block"),
            };
        }
        let Some(val) = r.strip_prefix(' ') else {
            return self.fail("expected space after :");
        };
        if val == "{}" {
            enter(level)?;
            self.pos += 1;
            return Ok(Value::Object(Vec::new()));
        }
        self.pos += 1;
        self.scalar_field(val)
    }

    fn scalar_field(&self, val: &str) -> Result<Value, Error> {
        if val.starts_with('"') {
            let (s, end) = parse_string_at(val, 0)?;
            if end != val.len() {
                return self.fail("text after string");
            }
            return Ok(Value::String(s));
        }
        Ok(bare_value(val))
    }

    fn array(&mut self, ind: usize, level: usize, rest: &str) -> Result<Value, Error> {
        let child = enter(level)?;
        let mut cur = &rest[1..];
        let digits = cur.bytes().take_while(u8::is_ascii_digit).count();
        let Ok(n) = cur[..digits].parse::<usize>() else {
            return self.fail("bad array length");
        };
        cur = &cur[digits..];
        let Some(after) = cur.strip_prefix(']') else {
            return self.fail("expected ]");
        };
        cur = after;
        let mut cols: Option<Cols> = None;
        if let Some(inner) = cur.strip_prefix('{') {
            let (parsed, remainder) = self.columns(inner)?;
            cols = Some(parsed);
            cur = remainder;
        }
        let Some(tail) = cur.strip_prefix(':') else {
            return self.fail("expected :");
        };
        if let Some(cols) = cols {
            if !tail.is_empty() {
                return self.fail("text after table header");
            }
            return self.rows(ind, child, n, &cols);
        }
        if tail.is_empty() {
            self.pos += 1;
            let mut items = Vec::new();
            for _ in 0..n {
                items.push(self.item(ind + 1, child)?);
            }
            return Ok(Value::Array(items));
        }
        let Some(cells_text) = tail.strip_prefix(' ') else {
            return self.fail("expected space after :");
        };
        let cells = self.split_cells(cells_text)?;
        if cells.len() != n {
            return self.fail("array length mismatch");
        }
        self.pos += 1;
        Ok(Value::Array(
            cells.into_iter().map(|c| cell_value(&c)).collect(),
        ))
    }

    fn columns<'t>(&self, text: &'t str) -> Result<(Cols, &'t str), Error> {
        let mut cols = Vec::new();
        let mut cur = text;
        loop {
            let (name, rest) = self.key_from(cur, true)?;
            cur = rest;
            let mut prefix = None;
            if let Some(after) = cur.strip_prefix('^') {
                let (cell, rest) = self.cell(after)?;
                prefix = Some(cell);
                cur = rest;
            }
            cols.push((name, prefix));
            if let Some(after) = cur.strip_prefix(',') {
                cur = after;
            } else if let Some(after) = cur.strip_prefix('}') {
                return Ok((cols, after));
            } else {
                return self.fail("bad table header");
            }
        }
    }

    fn cell<'t>(&self, text: &'t str) -> Result<(Cell, &'t str), Error> {
        if text.starts_with('"') {
            let (s, end) = parse_string_at(text, 0)?;
            return Ok((
                Cell {
                    text: s,
                    quoted: true,
                },
                &text[end..],
            ));
        }
        let end = text.find([',', '}']).unwrap_or(text.len());
        if end == 0 {
            return self.fail("empty cell");
        }
        Ok((
            Cell {
                text: text[..end].to_string(),
                quoted: false,
            },
            &text[end..],
        ))
    }

    fn split_cells(&self, text: &str) -> Result<Vec<Cell>, Error> {
        let mut cells = Vec::new();
        let mut cur = text;
        loop {
            let (cell, rest) = if cur.starts_with('"') {
                self.cell(cur)?
            } else {
                let end = cur.find(',').unwrap_or(cur.len());
                if end == 0 {
                    return self.fail("empty cell");
                }
                (
                    Cell {
                        text: cur[..end].to_string(),
                        quoted: false,
                    },
                    &cur[end..],
                )
            };
            cells.push(cell);
            match rest.strip_prefix(',') {
                Some(more) => cur = more,
                None if rest.is_empty() => return Ok(cells),
                None => return self.fail("text after cell"),
            }
        }
    }

    fn rows(
        &mut self,
        ind: usize,
        child: usize,
        n: usize,
        cols: &[(String, Option<Cell>)],
    ) -> Result<Value, Error> {
        enter(child)?;
        self.pos += 1;
        let mut items = Vec::new();
        for _ in 0..n {
            let l = self.line()?;
            if l.indent != ind + 1 {
                return self.fail("bad row indent");
            }
            let cells = self.split_cells(l.text)?;
            if cells.len() != cols.len() {
                return self.fail("row width mismatch");
            }
            let fields = cols
                .iter()
                .zip(cells)
                .map(|((name, prefix), cell)| {
                    let value = match prefix {
                        Some(p) => Value::String(format!("{}{}", p.text, cell.text)),
                        None => cell_value(&cell),
                    };
                    (name.clone(), value)
                })
                .collect();
            items.push(Value::Object(fields));
            self.pos += 1;
        }
        Ok(Value::Array(items))
    }

    fn item(&mut self, ind: usize, level: usize) -> Result<Value, Error> {
        let l = self.line()?;
        if l.indent != ind {
            return self.fail("bad item indent");
        }
        let Some(rest) = l.text.strip_prefix("- ") else {
            return self.fail("expected list item");
        };
        let off = l.off;
        if rest == "{}" {
            enter(level)?;
            self.pos += 1;
            return Ok(Value::Object(Vec::new()));
        }
        if rest.starts_with('[') {
            self.lines[self.pos] = Line {
                indent: ind + 2,
                text: rest,
                off,
            };
            return self.array(ind + 2, level, rest);
        }
        let is_key = if rest.starts_with('"') {
            let (_, end) = parse_string_at(rest, 0)?;
            end != rest.len()
        } else {
            match rest.find(KEY_STOP) {
                None => false,
                Some(at) => matches!(rest.as_bytes()[at], b':' | b'['),
            }
        };
        if is_key {
            self.lines[self.pos] = Line {
                indent: ind + 2,
                text: rest,
                off,
            };
            return self.object(ind + 2, enter(level)?);
        }
        self.pos += 1;
        if rest.starts_with('"') {
            let (s, _) = parse_string_at(rest, 0)?;
            return Ok(Value::String(s));
        }
        if rest.starts_with('$') || rest.contains(KEY_STOP) {
            return self.fail("bad item");
        }
        Ok(bare_value(rest))
    }
}

fn bare_value(tok: &str) -> Value {
    match classify(tok) {
        Scalar::Null => Value::Null,
        Scalar::Bool(b) => Value::Bool(b),
        Scalar::Number(n) => Value::Number(n.to_string()),
        Scalar::Str(s) => Value::String(s.to_string()),
    }
}

fn cell_value(c: &Cell) -> Value {
    if c.quoted {
        Value::String(c.text.clone())
    } else {
        bare_value(&c.text)
    }
}
