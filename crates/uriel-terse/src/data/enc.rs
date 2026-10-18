use std::collections::HashMap;

use super::rules::{id_for, key_token, quote, raw_token, string_ok, Ctx};
use crate::json::{is_number, MAX_DEPTH};
use crate::{Error, Value};

pub fn encode(value: &Value) -> Result<String, Error> {
    validate(value, 1)?;
    let mini = value.to_json();
    let toon = match value {
        Value::Object(fields) if !fields.is_empty() => Some(toon(value)),
        Value::Array(_) => Some(toon(value)),
        _ => None,
    };
    Ok(match toon {
        Some(t) if t.len() <= mini.len() => t,
        _ => format!("!{mini}"),
    })
}

fn validate(v: &Value, depth: usize) -> Result<(), Error> {
    match v {
        Value::Number(n) if !is_number(n) => Err(Error::InvalidNumber),
        Value::Array(items) => {
            if depth > MAX_DEPTH {
                return Err(Error::TooDeep);
            }
            items.iter().try_for_each(|i| validate(i, depth + 1))
        }
        Value::Object(fields) => {
            if depth > MAX_DEPTH {
                return Err(Error::TooDeep);
            }
            fields.iter().try_for_each(|(_, i)| validate(i, depth + 1))
        }
        _ => Ok(()),
    }
}

fn toon(root: &Value) -> String {
    let mut first = Enc::default();
    first.root(root);
    let dict = first.pick_dict();
    let mut second = Enc {
        dict,
        ..Enc::default()
    };
    let body = second.root(root);
    let mut defs: Vec<(&String, &String)> = second.dict.iter().collect();
    defs.sort_by(|a, b| a.1.len().cmp(&b.1.len()).then_with(|| a.1.cmp(b.1)));
    let mut out = String::new();
    for (key, id) in defs {
        out.push_str(id);
        out.push('=');
        out.push_str(&key_token(key));
        out.push('\n');
    }
    out.push_str(&body);
    out
}

enum Col {
    Plain,
    Prefix(usize),
    Const,
}

#[derive(Default)]
struct Enc {
    counts: HashMap<String, usize>,
    dict: HashMap<String, String>,
}

fn scalar_token(v: &Value, ctx: Ctx) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Number(n) => n.clone(),
        Value::String(s) => {
            if string_ok(s, ctx) {
                s.clone()
            } else {
                quote(s)
            }
        }
        _ => String::new(),
    }
}

fn push_line(out: &mut String, ind: usize, text: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    for _ in 0..ind {
        out.push(' ');
    }
    out.push_str(text);
}

fn common_prefix(strings: &[&str]) -> String {
    let mut prefix: &str = strings[0];
    for s in &strings[1..] {
        let mut n = 0;
        for (a, b) in prefix.chars().zip(s.chars()) {
            if a != b {
                break;
            }
            n += a.len_utf8();
        }
        prefix = &prefix[..n];
        if prefix.is_empty() {
            break;
        }
    }
    prefix.to_string()
}

impl Enc {
    fn root(&mut self, v: &Value) -> String {
        let mut out = String::new();
        match v {
            Value::Object(fields) => self.fields(&mut out, 0, fields),
            Value::Array(items) => self.array(&mut out, 0, "", items),
            _ => {}
        }
        out
    }

    fn key(&mut self, k: &str) -> String {
        *self.counts.entry(k.to_string()).or_insert(0) += 1;
        match self.dict.get(k) {
            Some(id) => id.clone(),
            None => key_token(k),
        }
    }

    fn pick_dict(&self) -> HashMap<String, String> {
        let mut cands: Vec<(&String, usize, usize)> = self
            .counts
            .iter()
            .filter(|(_, n)| **n >= 2)
            .map(|(k, n)| (k, *n, key_token(k).chars().count()))
            .collect();
        cands.sort_by(|a, b| (b.1 * b.2).cmp(&(a.1 * a.2)).then_with(|| a.0.cmp(b.0)));
        let mut dict = HashMap::new();
        for (key, n, width) in cands {
            let id = id_for(dict.len());
            let r = id.len();
            let saved = n * width;
            let cost = n * r + r + 1 + width + 1;
            if saved > cost {
                dict.insert(key.clone(), id);
            }
        }
        dict
    }

    fn fields(&mut self, out: &mut String, ind: usize, fields: &[(String, Value)]) {
        for (k, v) in fields {
            self.field(out, ind, k, v);
        }
    }

    fn field(&mut self, out: &mut String, ind: usize, k: &str, v: &Value) {
        let key = self.key(k);
        match v {
            Value::Object(f) if f.is_empty() => push_line(out, ind, &format!("{key}: {{}}")),
            Value::Object(f) => {
                push_line(out, ind, &format!("{key}:"));
                self.fields(out, ind + 1, f);
            }
            Value::Array(items) => self.array(out, ind, &key, items),
            scalar => push_line(
                out,
                ind,
                &format!("{key}: {}", scalar_token(scalar, Ctx::Field)),
            ),
        }
    }

    fn array(&mut self, out: &mut String, ind: usize, key: &str, items: &[Value]) {
        let n = items.len();
        if n == 0 {
            push_line(out, ind, &format!("{key}[0]:"));
        } else if items.iter().all(Value::is_scalar) {
            let cells: Vec<String> = items.iter().map(|v| scalar_token(v, Ctx::Cell)).collect();
            push_line(out, ind, &format!("{key}[{n}]: {}", cells.join(",")));
        } else if let Some(cols) = tabular_columns(items) {
            self.table(out, ind, key, items, &cols);
        } else {
            push_line(out, ind, &format!("{key}[{n}]:"));
            for item in items {
                self.item(out, ind + 1, item);
            }
        }
    }

    fn table(&mut self, out: &mut String, ind: usize, key: &str, items: &[Value], cols: &[&str]) {
        let n = items.len();
        let rows: Vec<&[(String, Value)]> = items
            .iter()
            .map(|v| match v {
                Value::Object(f) => f.as_slice(),
                _ => &[],
            })
            .collect();
        let mut header = Vec::with_capacity(cols.len());
        let mut kinds: Vec<Col> = Vec::with_capacity(cols.len());
        let mut constant: Vec<bool> = (0..cols.len())
            .map(|j| rows.iter().all(|r| r[j].1 == rows[0][j].1))
            .collect();
        if constant.iter().all(|c| *c) {
            constant[0] = false;
        }
        for (j, col) in cols.iter().enumerate() {
            let mut name = self.key(col);
            let first = &rows[0][j].1;
            if constant[j] {
                name.push('=');
                name.push_str(&scalar_token(first, Ctx::Cell));
                header.push(name);
                kinds.push(Col::Const);
                continue;
            }
            let strings: Option<Vec<&str>> = rows
                .iter()
                .map(|r| match &r[j].1 {
                    Value::String(s) => Some(s.as_str()),
                    _ => None,
                })
                .collect();
            let mut kind = Col::Plain;
            if let Some(strings) = strings {
                let p = common_prefix(&strings);
                if !p.is_empty() {
                    let ptoken = raw_token(&p, Ctx::Cell);
                    let without: usize = strings
                        .iter()
                        .map(|s| scalar_token(&Value::String((*s).to_string()), Ctx::Cell).len())
                        .sum();
                    let with: usize = strings
                        .iter()
                        .map(|s| raw_token(&s[p.len()..], Ctx::Cell).len())
                        .sum::<usize>()
                        + 1
                        + ptoken.len();
                    if with < without {
                        name.push('^');
                        name.push_str(&ptoken);
                        kind = Col::Prefix(p.len());
                    }
                }
            }
            header.push(name);
            kinds.push(kind);
        }
        push_line(out, ind, &format!("{key}[{n}]{{{}}}:", header.join(",")));
        for r in &rows {
            let cells: Vec<String> = r
                .iter()
                .zip(&kinds)
                .filter_map(|((_, v), kind)| match (v, kind) {
                    (_, Col::Const) => None,
                    (Value::String(s), Col::Prefix(n)) => Some(raw_token(&s[*n..], Ctx::Cell)),
                    _ => Some(scalar_token(v, Ctx::Cell)),
                })
                .collect();
            push_line(out, ind + 1, &cells.join(","));
        }
    }

    fn item(&mut self, out: &mut String, ind: usize, v: &Value) {
        match v {
            Value::Object(f) if f.is_empty() => push_line(out, ind, "- {}"),
            Value::Object(f) => {
                let mut buf = String::new();
                self.fields(&mut buf, ind + 2, f);
                merge(out, ind, &buf);
            }
            Value::Array(items) => {
                let mut buf = String::new();
                self.array(&mut buf, ind + 2, "", items);
                merge(out, ind, &buf);
            }
            scalar => push_line(out, ind, &format!("- {}", scalar_token(scalar, Ctx::Item))),
        }
    }
}

fn merge(out: &mut String, ind: usize, buf: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    for _ in 0..ind {
        out.push(' ');
    }
    out.push_str("- ");
    out.push_str(&buf[ind + 2..]);
}

fn tabular_columns(items: &[Value]) -> Option<Vec<&str>> {
    if items.len() < 2 {
        return None;
    }
    let mut cols: Option<Vec<&str>> = None;
    for item in items {
        let Value::Object(fields) = item else {
            return None;
        };
        if fields.is_empty() || !fields.iter().all(|(_, v)| v.is_scalar()) {
            return None;
        }
        let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
        match &cols {
            None => cols = Some(keys),
            Some(c) if *c == keys => {}
            Some(_) => return None,
        }
    }
    cols
}
