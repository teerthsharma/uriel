#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

pub struct Tag {
    pub name: String,
    pub closing: bool,
    pub attrs: BTreeMap<String, String>,
    pub names_in_order: Vec<String>,
}

pub fn tags(html: &str) -> Vec<Tag> {
    let b = html.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        let closing = j < b.len() && b[j] == b'/';
        if closing {
            j += 1;
        }
        let start = j;
        while j < b.len() && (b[j].is_ascii_alphanumeric()) {
            j += 1;
        }
        let name = html[start..j].to_ascii_lowercase();
        if name.is_empty() {
            i += 1;
            continue;
        }
        let mut attrs = BTreeMap::new();
        let mut order = Vec::new();
        loop {
            while j < b.len() && (b[j] == b' ' || b[j] == b'\n' || b[j] == b'/') {
                j += 1;
            }
            if j >= b.len() || b[j] == b'>' {
                break;
            }
            let s = j;
            while j < b.len() && !matches!(b[j], b'=' | b' ' | b'>' | b'/') {
                j += 1;
            }
            let key = html[s..j].to_ascii_lowercase();
            let mut val = String::new();
            if j < b.len() && b[j] == b'=' {
                j += 1;
                if j < b.len() && b[j] == b'"' {
                    j += 1;
                    let vs = j;
                    while j < b.len() && b[j] != b'"' {
                        j += 1;
                    }
                    val = html[vs..j].to_string();
                    j += 1;
                } else {
                    let vs = j;
                    while j < b.len() && !matches!(b[j], b' ' | b'>') {
                        j += 1;
                    }
                    val = html[vs..j].to_string();
                }
            }
            order.push(key.clone());
            attrs.insert(key, val);
        }
        out.push(Tag {
            name: name.clone(),
            closing,
            attrs,
            names_in_order: order,
        });
        i = j;
        if name == "style" && !closing {
            i = html[i..].find("</style>").map_or(b.len(), |k| i + k);
        }
    }
    out
}

pub fn a11y(html: &str) -> Result<(), String> {
    if !html.starts_with("<!doctype html>") {
        return Err("missing doctype".into());
    }
    let ts = tags(html);
    let html_tag = ts.iter().find(|t| t.name == "html").ok_or("no html")?;
    if html_tag.attrs.get("lang").is_none_or(|l| l.is_empty()) {
        return Err("no lang".into());
    }
    let mut ids = BTreeSet::new();
    let mut refs = Vec::new();
    let mut last_heading = 0u32;
    let mut h1 = 0;
    let mut in_table = false;
    let mut caption_seen = false;
    let mut main = 0;
    for t in &ts {
        if t.closing {
            if t.name == "table" {
                if !caption_seen {
                    return Err("table without caption".into());
                }
                in_table = false;
            }
            continue;
        }
        for k in &t.names_in_order {
            if k.starts_with("on") {
                return Err(format!("event attribute {k}"));
            }
        }
        if let Some(id) = t.attrs.get("id") {
            if !ids.insert(id.clone()) {
                return Err(format!("duplicate id {id}"));
            }
        }
        if let Some(r) = t.attrs.get("aria-labelledby") {
            refs.push(r.clone());
        }
        if let Some(h) = t.attrs.get("href") {
            if h.to_ascii_lowercase().starts_with("javascript:") {
                return Err("javascript href".into());
            }
        }
        match t.name.as_str() {
            "script" | "iframe" | "object" | "embed" | "link" | "img" | "form" => {
                return Err(format!("forbidden tag {}", t.name))
            }
            "main" => main += 1,
            "nav" if t.attrs.get("aria-label").is_none_or(|l| l.is_empty()) => {
                return Err("nav without label".into())
            }
            "section"
                if !t.attrs.contains_key("aria-labelledby")
                    && !t.attrs.contains_key("aria-label") =>
            {
                return Err("section without name".into())
            }
            "table" => {
                in_table = true;
                caption_seen = false;
            }
            "caption" if in_table => caption_seen = true,
            "th" if !t.attrs.contains_key("scope") => return Err("th without scope".into()),
            n if n.len() == 2 && n.starts_with('h') && n.as_bytes()[1].is_ascii_digit() => {
                let l = u32::from(n.as_bytes()[1] - b'0');
                if l == 1 {
                    h1 += 1;
                }
                if l > last_heading + 1 {
                    return Err(format!("heading skip h{last_heading} to h{l}"));
                }
                last_heading = l;
            }
            _ => {}
        }
    }
    if h1 != 1 {
        return Err(format!("{h1} h1 elements"));
    }
    if main != 1 {
        return Err("need exactly one main".into());
    }
    for r in refs {
        if !ids.contains(&r) {
            return Err(format!("dangling aria-labelledby {r}"));
        }
    }
    Ok(())
}

pub fn hrefs(html: &str) -> Vec<String> {
    tags(html)
        .into_iter()
        .filter_map(|t| t.attrs.get("href").cloned())
        .collect()
}

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

pub const XSS: &[&str] = &[
    "<script>alert(1)</script>",
    "\"><script>alert(1)</script>",
    "\" onmouseover=\"alert(1)",
    "' onfocus='alert(1)' autofocus='",
    "<img src=x onerror=alert(1)>",
    "<svg/onload=alert(1)>",
    "</style><script>alert(1)</script>",
    "</title><script>alert(1)</script>",
    "javascript:alert(1)",
    "<a href=\"javascript:alert(1)\">x</a>",
    "&lt;script&gt;",
    "`onerror=alert(1)",
    "<iframe src=//evil.example></iframe>",
    "\u{0}<script>",
];
