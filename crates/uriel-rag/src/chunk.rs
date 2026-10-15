#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Python,
    Script,
    Markdown,
    Toml,
    Json,
    Text,
}

impl Lang {
    pub fn from_path(path: &str) -> Lang {
        let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        match ext.as_deref() {
            Some("rs") => Lang::Rust,
            Some("py") => Lang::Python,
            Some("ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs") => Lang::Script,
            Some("md" | "markdown") => Lang::Markdown,
            Some("toml") => Lang::Toml,
            Some("json") => Lang::Json,
            _ => Lang::Text,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChunkOptions {
    pub window_lines: usize,
    pub overlap_lines: usize,
    pub min_lines: usize,
    pub max_lines: usize,
}

impl Default for ChunkOptions {
    fn default() -> Self {
        ChunkOptions {
            window_lines: 40,
            overlap_lines: 5,
            min_lines: 4,
            max_lines: 80,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawChunk {
    pub start_line: u32,
    pub end_line: u32,
    pub text: String,
}

fn indent_of(line: &str) -> usize {
    line.chars()
        .take_while(|c| c.is_whitespace())
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

pub(crate) fn rust_item_start(line: &str) -> bool {
    let indent = indent_of(line);
    if indent > 8 {
        return false;
    }
    let mut t = line.trim_start();
    if let Some(rest) = t.strip_prefix("pub") {
        if rest.starts_with('(') {
            match rest.find(')') {
                Some(i) => t = rest[i + 1..].trim_start(),
                None => return false,
            }
        } else if rest.starts_with(' ') {
            t = rest.trim_start();
        }
    }
    let mut saw_const = false;
    'outer: loop {
        for m in ["async ", "unsafe ", "const ", "extern ", "default "] {
            if let Some(r) = t.strip_prefix(m) {
                saw_const |= m == "const ";
                t = r.trim_start();
                if m == "extern " && t.starts_with('"') {
                    if let Some(i) = t[1..].find('"') {
                        t = t[i + 2..].trim_start();
                    }
                }
                continue 'outer;
            }
        }
        break;
    }
    for kw in [
        "fn ",
        "struct ",
        "enum ",
        "trait ",
        "impl ",
        "impl<",
        "mod ",
        "union ",
        "macro_rules!",
    ] {
        if t.starts_with(kw) {
            return true;
        }
    }
    if indent <= 4 && (t.starts_with("type ") || t.starts_with("static ")) {
        return true;
    }
    if indent <= 4 && saw_const {
        let name_end = t.find(':').unwrap_or(0);
        return name_end > 0
            && t[..name_end]
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_');
    }
    false
}

fn python_item_start(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("def ") || t.starts_with("async def ") || t.starts_with("class ")
}

fn script_item_start(line: &str) -> bool {
    let indent = indent_of(line);
    let t = line.trim_start();
    if indent == 0 {
        return [
            "function ",
            "function(",
            "async function",
            "export ",
            "class ",
            "interface ",
            "type ",
            "enum ",
            "const ",
            "let ",
            "var ",
            "abstract class",
            "declare ",
            "namespace ",
            "module ",
            "describe(",
            "test(",
            "it(",
        ]
        .iter()
        .any(|k| t.starts_with(k));
    }
    if !(2..=4).contains(&indent) || !t.trim_end().ends_with('{') || !t.contains('(') {
        return false;
    }
    let mut head = t;
    'strip: loop {
        for m in [
            "public ",
            "private ",
            "protected ",
            "static ",
            "async ",
            "get ",
            "set ",
            "readonly ",
            "override ",
        ] {
            if let Some(r) = head.strip_prefix(m) {
                head = r;
                continue 'strip;
            }
        }
        break;
    }
    let name = head.split('(').next().unwrap_or("").trim();
    let control = [
        "if", "for", "while", "switch", "catch", "function", "return", "else", "with",
    ];
    !name.is_empty()
        && !control.contains(&name)
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

fn markdown_heading(line: &str) -> bool {
    let t = line.trim_start();
    if line.len() - t.len() > 3 {
        return false;
    }
    let hashes = t.chars().take_while(|c| *c == '#').count();
    (1..=6).contains(&hashes) && t[hashes..].starts_with(' ')
}

fn toml_header(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('[') && t.ends_with(']')
}

fn json_key(line: &str) -> bool {
    let indent_ok = line.starts_with("  \"") || line.starts_with("\t\"");
    indent_ok && line.contains("\":")
}

pub(crate) fn is_signature(line: &str, lang: Lang) -> bool {
    match lang {
        Lang::Rust => rust_item_start(line),
        Lang::Python => python_item_start(line),
        Lang::Script => script_item_start(line),
        Lang::Markdown => markdown_heading(line),
        Lang::Toml => toml_header(line),
        Lang::Json => json_key(line),
        Lang::Text => false,
    }
}

pub(crate) fn is_doc_line(line: &str, lang: Lang) -> bool {
    let t = line.trim_start();
    match lang {
        Lang::Rust => t.starts_with("///") || t.starts_with("//!"),
        Lang::Script => {
            t.starts_with("/**") || t.starts_with("* ") || t.starts_with("*/") || t == "*"
        }
        Lang::Python => t.starts_with("\"\"\"") || t.starts_with("'''"),
        _ => false,
    }
}

fn is_leading(line: &str, lang: Lang) -> bool {
    let t = line.trim_start();
    if t.is_empty() {
        return false;
    }
    match lang {
        Lang::Rust => t.starts_with("//") || t.starts_with("#["),
        Lang::Python => t.starts_with('@') || t.starts_with('#'),
        Lang::Script => {
            t.starts_with("//") || t.starts_with("/*") || t.starts_with('*') || t.starts_with('@')
        }
        Lang::Toml => t.starts_with('#'),
        Lang::Markdown | Lang::Json | Lang::Text => false,
    }
}

fn start_flags(lines: &[&str], lang: Lang) -> Vec<bool> {
    let mut in_fence = false;
    lines
        .iter()
        .map(|l| {
            if lang == Lang::Markdown {
                let t = l.trim_start();
                if t.starts_with("```") || t.starts_with("~~~") {
                    in_fence = !in_fence;
                    return false;
                }
                if in_fence {
                    return false;
                }
            }
            is_signature(l, lang)
        })
        .collect()
}

fn non_blank(lines: &[&str], a: usize, b: usize) -> usize {
    lines[a..b].iter().filter(|l| !l.trim().is_empty()).count()
}

pub fn chunk_text(text: &str, lang: Lang, opts: &ChunkOptions) -> Vec<RawChunk> {
    let lines: Vec<&str> = text.lines().collect();
    let n = lines.len();
    if n == 0 {
        return Vec::new();
    }
    let window = opts.window_lines.max(2);
    let overlap = opts.overlap_lines.min(window - 1);

    let mut cuts: Vec<usize> = Vec::new();
    if lang != Lang::Text {
        for (i, is_start) in start_flags(&lines, lang).into_iter().enumerate() {
            if !is_start {
                continue;
            }
            let mut j = i;
            while j > 0 && is_leading(lines[j - 1], lang) && cuts.last() != Some(&(j - 1)) {
                j -= 1;
            }
            if j > 0 && cuts.last() != Some(&j) {
                cuts.push(j);
            }
        }
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut segs: Vec<(usize, usize)> = Vec::new();
    let mut a = 0;
    for &c in &cuts {
        if c > a {
            segs.push((a, c));
            a = c;
        }
    }
    segs.push((a, n));

    let mut merged: Vec<(usize, usize)> = Vec::new();
    let mut cur: Option<(usize, usize)> = None;
    for seg in segs {
        cur = match cur {
            None => Some(seg),
            Some((a, b)) if non_blank(&lines, a, b) < opts.min_lines => Some((a, seg.1)),
            Some(done) => {
                merged.push(done);
                Some(seg)
            }
        };
    }
    if let Some((a, b)) = cur {
        match merged.last_mut() {
            Some(last) if non_blank(&lines, a, b) < opts.min_lines => last.1 = b,
            _ => merged.push((a, b)),
        }
    }

    let limit = if lang == Lang::Text {
        window
    } else {
        opts.max_lines.max(window)
    };
    let mut out = Vec::new();
    for (mut a, mut b) in merged {
        while a < b && lines[a].trim().is_empty() {
            a += 1;
        }
        while b > a && lines[b - 1].trim().is_empty() {
            b -= 1;
        }
        if a >= b {
            continue;
        }
        let mut spans = Vec::new();
        if b - a > limit {
            let mut s = a;
            loop {
                let e = (s + window).min(b);
                spans.push((s, e));
                if e >= b {
                    break;
                }
                s = e - overlap;
            }
        } else {
            spans.push((a, b));
        }
        for (s, e) in spans {
            out.push(RawChunk {
                start_line: u32::try_from(s + 1).unwrap_or(u32::MAX),
                end_line: u32::try_from(e).unwrap_or(u32::MAX),
                text: lines[s..e].join("\n"),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks(src: &[&str], lang: Lang) -> Vec<RawChunk> {
        chunk_text(&src.join("\n"), lang, &ChunkOptions::default())
    }

    fn pad(name: &str) -> Vec<String> {
        (0..5)
            .map(|i| format!("    let {name}{i} = {i};"))
            .collect()
    }

    #[test]
    fn language_from_path() {
        assert_eq!(Lang::from_path("src/a.rs"), Lang::Rust);
        assert_eq!(Lang::from_path("a.PY"), Lang::Python);
        assert_eq!(Lang::from_path("a.tsx"), Lang::Script);
        assert_eq!(Lang::from_path("README.md"), Lang::Markdown);
        assert_eq!(Lang::from_path("Cargo.toml"), Lang::Toml);
        assert_eq!(Lang::from_path("x.json"), Lang::Json);
        assert_eq!(Lang::from_path("notes.txt"), Lang::Text);
        assert_eq!(Lang::from_path("decision:7"), Lang::Text);
    }

    #[test]
    fn rust_cuts_at_items_and_keeps_docs_and_attributes_attached() {
        let mut src = vec!["use std::fmt;".to_string(), "".into()];
        src.push("/// Adds things.".into());
        src.push("#[inline]".into());
        src.push("pub fn add() {".into());
        src.extend(pad("a"));
        src.push("}".into());
        src.push("".into());
        src.push("pub(crate) async fn sub() {".into());
        src.extend(pad("b"));
        src.push("}".into());
        let refs: Vec<&str> = src.iter().map(String::as_str).collect();
        let c = chunks(&refs, Lang::Rust);
        assert_eq!(c.len(), 2, "{c:#?}");
        assert_eq!(c[0].start_line, 1);
        assert!(c[0]
            .text
            .contains("/// Adds things.\n#[inline]\npub fn add()"));
        assert!(c[1].text.starts_with("pub(crate) async fn sub()"));
        assert_eq!(c[1].end_line as usize, src.len());
    }

    #[test]
    fn rust_methods_inside_impl_are_separate_chunks() {
        let mut src = vec!["impl Foo {".to_string()];
        for name in ["one", "two"] {
            src.push(format!("    /// Does {name}."));
            src.push(format!("    pub fn {name}(&self) {{"));
            for i in 0..5 {
                src.push(format!("        let v{i} = {i};"));
            }
            src.push("    }".into());
        }
        src.push("}".into());
        let refs: Vec<&str> = src.iter().map(String::as_str).collect();
        let c = chunks(&refs, Lang::Rust);
        assert_eq!(c.len(), 2);
        assert!(c[1].text.contains("/// Does two."));
        assert!(c[0].text.starts_with("impl Foo {"));
    }

    #[test]
    fn python_cuts_at_def_and_class_with_decorators() {
        let src = [
            "import os",
            "",
            "@decorator",
            "def first():",
            "    a = 1",
            "    b = 2",
            "    c = 3",
            "    return a + b + c",
            "",
            "class Thing:",
            "    def method(self):",
            "        x = 1",
            "        y = 2",
            "        z = 3",
            "        return x",
        ];
        let c = chunks(&src, Lang::Python);
        assert!(c.len() >= 2, "{c:#?}");
        assert!(c[0].text.contains("@decorator\ndef first():"));
        assert!(c.iter().any(|k| k.text.contains("class Thing:")));
    }

    #[test]
    fn typescript_cuts_at_exports_and_classes() {
        let src = [
            "import { a } from './a';",
            "export function one(): number {",
            "  const x = 1;",
            "  const y = 2;",
            "  const z = 3;",
            "  return x + y + z;",
            "}",
            "",
            "export class Two {",
            "  constructor(private v: number) {",
            "    this.v = v;",
            "  }",
            "  run(): number {",
            "    return this.v;",
            "  }",
            "}",
        ];
        let c = chunks(&src, Lang::Script);
        assert!(c.len() >= 2, "{c:#?}");
        assert!(c.iter().any(|k| k.text.starts_with("export class Two")));
        assert!(c.iter().any(|k| k.text.contains("export function one")));
    }

    #[test]
    fn markdown_cuts_at_headings_but_not_inside_fences() {
        let src = [
            "# Title",
            "intro one",
            "intro two",
            "intro three",
            "",
            "```sh",
            "# not a heading",
            "echo hi",
            "```",
            "",
            "## Section",
            "body one",
            "body two",
            "body three",
            "body four",
        ];
        let c = chunks(&src, Lang::Markdown);
        assert_eq!(c.len(), 2, "{c:#?}");
        assert!(c[0].text.contains("# not a heading"));
        assert!(c[1].text.starts_with("## Section"));
    }

    #[test]
    fn toml_cuts_at_tables() {
        let src = [
            "[package]",
            "name = \"x\"",
            "version = \"1\"",
            "edition = \"2021\"",
            "",
            "# deps",
            "[dependencies]",
            "a = \"1\"",
            "b = \"2\"",
            "c = \"3\"",
        ];
        let c = chunks(&src, Lang::Toml);
        assert_eq!(c.len(), 2, "{c:#?}");
        assert!(c[1].text.starts_with("# deps\n[dependencies]"));
    }

    #[test]
    fn json_cuts_at_top_level_keys() {
        let src = [
            "{",
            "  \"alpha\": {",
            "    \"a\": 1,",
            "    \"b\": 2,",
            "    \"c\": 3",
            "  },",
            "  \"beta\": [",
            "    1,",
            "    2,",
            "    3",
            "  ]",
            "}",
        ];
        let c = chunks(&src, Lang::Json);
        assert_eq!(c.len(), 2, "{c:#?}");
        assert!(c[1].text.starts_with("  \"beta\""));
    }

    #[test]
    fn plain_text_uses_overlapping_windows() {
        let src: Vec<String> = (1..=100).map(|i| format!("line {i}")).collect();
        let refs: Vec<&str> = src.iter().map(String::as_str).collect();
        let c = chunks(&refs, Lang::Text);
        assert_eq!(c[0].start_line, 1);
        assert_eq!(c[0].end_line, 40);
        assert_eq!(c[1].start_line, 36, "5 lines of overlap");
        assert_eq!(c.last().map(|k| k.end_line), Some(100));
        for w in c.windows(2) {
            assert_eq!(w[1].start_line, w[0].end_line - 4);
        }
    }

    #[test]
    fn oversize_unit_falls_back_to_windows_and_everything_is_covered() {
        let mut src = vec!["fn huge() {".to_string()];
        src.extend((0..200).map(|i| format!("    let v{i} = {i};")));
        src.push("}".into());
        let refs: Vec<&str> = src.iter().map(String::as_str).collect();
        let c = chunks(&refs, Lang::Rust);
        assert!(c.len() > 3);
        assert_eq!(c[0].start_line, 1);
        assert_eq!(c.last().map(|k| k.end_line as usize), Some(src.len()));
        for w in c.windows(2) {
            assert!(
                w[1].start_line <= w[0].end_line + 1,
                "no gap between windows"
            );
        }
    }

    #[test]
    fn empty_and_blank_inputs_yield_nothing() {
        assert!(chunk_text("", Lang::Rust, &ChunkOptions::default()).is_empty());
        assert!(chunk_text("\n\n  \n", Lang::Text, &ChunkOptions::default()).is_empty());
    }

    #[test]
    fn rust_item_start_cases() {
        for l in [
            "fn a() {}",
            "pub fn a() {}",
            "pub(crate) const fn a() {}",
            "    pub async fn a() {}",
            "unsafe impl Send for X {}",
            "impl<T> Foo<T> {",
            "pub struct A;",
            "const MAX: usize = 3;",
            "macro_rules! m {",
            "extern \"C\" fn cb() {}",
        ] {
            assert!(rust_item_start(l), "{l}");
        }
        for l in [
            "let fn_ptr = 1;",
            "        const LOCAL: u8 = 1;",
            "            fn deep() {}",
            "// fn commented()",
            "publish(fn_x);",
            "use std::fmt;",
        ] {
            assert!(!rust_item_start(l), "{l}");
        }
    }
}
