#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Fence,
    InlineCode,
    Url,
    Path,
    Identifier,
    Hash,
    Version,
    Number,
    Quoted,
    Latex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

const LONG_WINDOW: usize = 1024;
const SHORT_WINDOW: usize = 300;
const TOKEN_CAP: usize = 256;

const UNITS: [&str; 23] = [
    "ms", "us", "ns", "sec", "min", "hr", "KB", "MB", "GB", "TB", "KiB", "MiB", "GiB", "TiB",
    "Hz", "kHz", "MHz", "GHz", "px", "tokens", "tok", "fps", "ops",
];

const FILE_EXTENSIONS: [&str; 44] = [
    "rs", "md", "toml", "json", "py", "js", "ts", "tsx", "jsx", "yml", "yaml", "txt", "c", "h",
    "cpp", "hpp", "go", "java", "sh", "ps1", "lock", "cfg", "ini", "html", "css", "sql", "csv",
    "log", "pdf", "png", "jpg", "svg", "zig", "kt", "rb", "php", "swift", "lua", "xml", "wasm",
    "bin", "exe", "dll", "so",
];

enum Hit {
    Span(usize, Kind),
    Skip(usize),
    Nothing,
}

pub fn scan(text: &str) -> Vec<Span> {
    let b = text.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match at(b, i) {
            Hit::Span(end, kind) if end > i => {
                spans.push(Span {
                    start: i,
                    end,
                    kind,
                });
                i = end;
            }
            Hit::Skip(n) => i += n.max(1),
            _ => i += 1,
        }
        while i < b.len() && !text.is_char_boundary(i) {
            i += 1;
        }
    }
    spans
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn is_seg(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-' | b'~' | b'+' | b'@' | b'%')
}

fn at(b: &[u8], i: usize) -> Hit {
    let c = b[i];
    match c {
        b'`' | b'~' => {
            if let Some(end) = fence(b, i) {
                return Hit::Span(end, Kind::Fence);
            }
            if c == b'`' {
                inline_code(b, i)
            } else {
                path_hit(b, i)
            }
        }
        b'$' => dollar(b, i),
        b'\\' => backslash(b, i),
        b'"' => double_quote(b, i),
        b'\'' => single_quote(b, i),
        0xE2 => curly_quote(b, i),
        b'-' => flag(b, i),
        b'/' | b'.' => path_hit(b, i),
        _ if is_word(c) && (i == 0 || !is_word(b[i - 1])) => word(b, i),
        _ => Hit::Nothing,
    }
}

fn line_end(b: &[u8], from: usize) -> usize {
    b[from..]
        .iter()
        .position(|&c| c == b'\n')
        .map_or(b.len(), |p| from + p)
}

fn fence(b: &[u8], i: usize) -> Option<usize> {
    let ch = b[i];
    let mut j = i;
    let mut spaces = 0;
    while j > 0 && b[j - 1] == b' ' {
        j -= 1;
        spaces += 1;
    }
    if spaces > 3 || (j > 0 && b[j - 1] != b'\n') {
        return None;
    }
    let n = b[i..].iter().take_while(|&&c| c == ch).count();
    if n < 3 {
        return None;
    }
    let first_end = line_end(b, i + n);
    if ch == b'`' && b[i + n..first_end].contains(&b'`') {
        return None;
    }
    let mut p = first_end;
    while p < b.len() {
        let start = p + 1;
        let end = line_end(b, start);
        let mut k = start;
        while k < end && b[k] == b' ' && k - start < 3 {
            k += 1;
        }
        let m = b[k..end].iter().take_while(|&&c| c == ch).count();
        if m >= n && b[k + m..end].iter().all(|c| matches!(c, b' ' | b'\t' | b'\r')) {
            let cut = if end > start && b[end - 1] == b'\r' {
                end - 1
            } else {
                end
            };
            return Some(cut);
        }
        p = end;
    }
    Some(b.len())
}

fn inline_code(b: &[u8], i: usize) -> Hit {
    let n = b[i..].iter().take_while(|&&c| c == b'`').count();
    let limit = b.len().min(i + n + LONG_WINDOW);
    let mut p = i + n;
    while p < limit && b[p] != b'\n' {
        if b[p] == b'`' {
            let m = b[p..].iter().take_while(|&&c| c == b'`').count();
            if m == n {
                return Hit::Span(p + m, Kind::InlineCode);
            }
            p += m;
        } else {
            p += 1;
        }
    }
    Hit::Skip(n)
}

fn find_seq(b: &[u8], from: usize, limit: usize, needle: &[u8]) -> Option<usize> {
    let end = b.len().min(limit);
    if from >= end {
        return None;
    }
    b[from..end]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| from + p)
}

fn dollar(b: &[u8], i: usize) -> Hit {
    if b.get(i + 1) == Some(&b'$') {
        return match find_seq(b, i + 2, i + 2 + LONG_WINDOW, b"$$") {
            Some(p) => Hit::Span(p + 2, Kind::Latex),
            None => Hit::Skip(2),
        };
    }
    match b.get(i + 1) {
        None => Hit::Nothing,
        Some(&n) if n.is_ascii_whitespace() => Hit::Nothing,
        Some(&n) => {
            let limit = b.len().min(i + 1 + SHORT_WINDOW);
            let mut q = i + 2;
            while q < limit && b[q] != b'\n' {
                if b[q] == b'$'
                    && !b[q - 1].is_ascii_whitespace()
                    && !b.get(q + 1).is_some_and(u8::is_ascii_digit)
                {
                    return Hit::Span(q + 1, Kind::Latex);
                }
                q += 1;
            }
            if n.is_ascii_alphabetic() || n == b'_' {
                let end = i + 1 + b[i + 1..].iter().take_while(|&&c| is_word(c)).count();
                return Hit::Span(end, Kind::Identifier);
            }
            Hit::Nothing
        }
    }
}

fn backslash(b: &[u8], i: usize) -> Hit {
    match b.get(i + 1) {
        Some(b'(') => match find_seq(b, i + 2, i + 2 + LONG_WINDOW, b"\\)") {
            Some(p) => Hit::Span(p + 2, Kind::Latex),
            None => Hit::Nothing,
        },
        Some(b'[') => match find_seq(b, i + 2, i + 2 + LONG_WINDOW, b"\\]") {
            Some(p) => Hit::Span(p + 2, Kind::Latex),
            None => Hit::Nothing,
        },
        Some(c) if c.is_ascii_alphabetic() => {
            let mut j = i + 1 + b[i + 1..].iter().take_while(|c| c.is_ascii_alphabetic()).count();
            while b.get(j) == Some(&b'{') {
                match brace_end(b, j) {
                    Some(end) => j = end,
                    None => break,
                }
            }
            Hit::Span(j, Kind::Latex)
        }
        _ => Hit::Nothing,
    }
}

fn brace_end(b: &[u8], open: usize) -> Option<usize> {
    let limit = b.len().min(open + SHORT_WINDOW);
    let mut depth = 0usize;
    for (k, &c) in b[open..limit].iter().enumerate() {
        match c {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + k + 1);
                }
            }
            b'\n' => return None,
            _ => {}
        }
    }
    None
}

fn double_quote(b: &[u8], i: usize) -> Hit {
    if i > 0 && is_word(b[i - 1]) {
        return Hit::Nothing;
    }
    let limit = b.len().min(i + 1 + SHORT_WINDOW);
    let mut q = i + 1;
    while q < limit && b[q] != b'\n' {
        if b[q] == b'"' {
            return if q > i + 1 {
                Hit::Span(q + 1, Kind::Quoted)
            } else {
                Hit::Nothing
            };
        }
        q += 1;
    }
    Hit::Nothing
}

fn curly_quote(b: &[u8], i: usize) -> Hit {
    if b[i..].starts_with(&[0xE2, 0x80, 0x9C]) {
        let limit = b.len().min(i + 3 + SHORT_WINDOW);
        let mut q = i + 3;
        while q < limit && b[q] != b'\n' {
            if b[q..].starts_with(&[0xE2, 0x80, 0x9D]) {
                return if q > i + 3 {
                    Hit::Span(q + 3, Kind::Quoted)
                } else {
                    Hit::Nothing
                };
            }
            q += 1;
        }
    }
    Hit::Nothing
}

fn single_quote(b: &[u8], i: usize) -> Hit {
    if i > 0 && is_word(b[i - 1]) {
        return Hit::Nothing;
    }
    match b.get(i + 1) {
        None => return Hit::Nothing,
        Some(c) if c.is_ascii_whitespace() => return Hit::Nothing,
        _ => {}
    }
    let limit = b.len().min(i + 1 + SHORT_WINDOW);
    let mut q = i + 2;
    while q < limit && b[q] != b'\n' {
        if b[q] == b'\'' && !b[q - 1].is_ascii_whitespace() && !b.get(q + 1).is_some_and(|&c| is_word(c))
        {
            return Hit::Span(q + 1, Kind::Quoted);
        }
        q += 1;
    }
    Hit::Nothing
}

fn flag(b: &[u8], i: usize) -> Hit {
    if (i > 0 && is_word(b[i - 1])) || b.get(i + 1) != Some(&b'-') {
        return Hit::Nothing;
    }
    if !b.get(i + 2).is_some_and(u8::is_ascii_alphanumeric) {
        return Hit::Nothing;
    }
    let end = i
        + 2
        + b[i + 2..]
            .iter()
            .take_while(|&&c| is_word(c) || c == b'-')
            .count();
    Hit::Span(end, Kind::Identifier)
}

fn path_hit(b: &[u8], i: usize) -> Hit {
    if i > 0 && (is_word(b[i - 1]) || matches!(b[i - 1], b'/' | b'\\' | b'.' | b'-')) {
        return Hit::Nothing;
    }
    match path(b, i) {
        Some(end) => Hit::Span(end, Kind::Path),
        None => Hit::Nothing,
    }
}

fn path(b: &[u8], i: usize) -> Option<usize> {
    let limit = b.len().min(i + TOKEN_CAP);
    let mut j = i;
    if b[j] == b'/' {
        if !b.get(j + 1).is_some_and(|&c| is_seg(c)) {
            return None;
        }
        j += 1;
    } else if b[j].is_ascii_alphabetic()
        && b.get(j + 1) == Some(&b':')
        && matches!(b.get(j + 2), Some(b'\\' | b'/'))
        && b.get(j + 3).is_some_and(|&c| is_seg(c))
    {
        j += 3;
    }
    loop {
        let s = j;
        while j < limit && is_seg(b[j]) {
            j += 1;
        }
        if j == s {
            break;
        }
        if j + 1 < limit && matches!(b[j], b'/' | b'\\') && is_seg(b[j + 1]) {
            j += 1;
        } else {
            break;
        }
    }
    while j > i && matches!(b[j - 1], b'.' | b'-' | b'+') {
        j -= 1;
    }
    if j <= i {
        return None;
    }
    let slice = &b[i..j];
    let seps = slice
        .iter()
        .enumerate()
        .filter(|(k, &c)| matches!(c, b'/' | b'\\') && *k > 0)
        .count();
    let last = slice
        .rsplit(|&c| matches!(c, b'/' | b'\\'))
        .next()
        .unwrap_or(&[]);
    if last.is_empty() {
        return None;
    }
    let ext = last
        .iter()
        .rposition(|&c| c == b'.')
        .filter(|&p| p > 0 && p + 1 < last.len())
        .map(|p| &last[p + 1..]);
    let anchored = slice.starts_with(b"~/")
        || slice.starts_with(b"./")
        || slice.starts_with(b"../")
        || slice[0] == b'/'
        || (slice.len() > 2 && slice[1] == b':');
    let known = ext.is_some_and(|e| {
        FILE_EXTENSIONS
            .iter()
            .any(|k| k.as_bytes().eq_ignore_ascii_case(e))
    });
    let ok = if seps == 0 {
        known
    } else {
        anchored || seps >= 2 || ext.is_some()
    };
    ok.then_some(j)
}

fn url(b: &[u8], i: usize) -> Option<usize> {
    let lower_www = b.len() >= i + 4 && b[i..i + 4].eq_ignore_ascii_case(b"www.");
    let mut scheme = false;
    if b[i].is_ascii_alphabetic() {
        let k = i + b[i..]
            .iter()
            .take_while(|&&c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'.' | b'-'))
            .count();
        scheme = b[k..].starts_with(b"://") && b.get(k + 3).is_some_and(|c| !c.is_ascii_whitespace());
    }
    if !scheme && !(lower_www && b.get(i + 4).is_some_and(u8::is_ascii_alphanumeric)) {
        return None;
    }
    let mut end = i;
    while end < b.len()
        && !b[end].is_ascii_whitespace()
        && !matches!(b[end], b'<' | b'>' | b'"' | b'`')
    {
        end += 1;
    }
    loop {
        let slice = &b[i..end];
        let last = *slice.last()?;
        let opens = |o: u8| slice.iter().filter(|&&c| c == o).count();
        let trim = matches!(last, b'.' | b',' | b';' | b':' | b'!' | b'?' | b'\'')
            || (last == b')' && opens(b')') > opens(b'('))
            || (last == b']' && opens(b']') > opens(b'['))
            || (last == b'}' && opens(b'}') > opens(b'{'));
        if !trim {
            break;
        }
        end -= 1;
    }
    (end > i + 4).then_some(end)
}

fn word(b: &[u8], i: usize) -> Hit {
    if let Some(end) = url(b, i) {
        return Hit::Span(end, Kind::Url);
    }
    if let Some(end) = path(b, i) {
        return Hit::Span(end, Kind::Path);
    }
    let (end, kind) = token(b, i);
    match kind {
        Some(k) => Hit::Span(end, k),
        None => Hit::Skip(end - i),
    }
}

fn word_run(b: &[u8], from: usize) -> usize {
    from + b[from..].iter().take_while(|&&c| is_word(c)).count()
}

fn token(b: &[u8], i: usize) -> (usize, Option<Kind>) {
    let mut j = word_run(b, i);
    let run = &b[i..j];
    if run.iter().any(u8::is_ascii_digit) {
        let hex = run.len() >= 7
            && run.iter().all(u8::is_ascii_hexdigit)
            && run.iter().any(|c| c.is_ascii_alphabetic());
        if hex {
            return (j, Some(Kind::Hash));
        }
        let mut dots = 0;
        while j + 1 < b.len()
            && matches!(b[j], b'.' | b',')
            && b[j + 1].is_ascii_digit()
            && b[j - 1].is_ascii_digit()
        {
            if b[j] == b'.' {
                dots += 1;
            }
            j = word_run(b, j + 1);
        }
        let mut pre = false;
        if dots > 0
            && j + 1 < b.len()
            && matches!(b[j], b'-' | b'+')
            && b[j + 1].is_ascii_alphanumeric()
        {
            pre = true;
            j += 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'.' | b'-' | b'+'))
            {
                j += 1;
            }
            while matches!(b[j - 1], b'.' | b'-' | b'+') {
                j -= 1;
            }
        }
        if run[0].is_ascii_digit() {
            if b.get(j) == Some(&b'%') {
                j += 1;
            } else if b.get(j) == Some(&b' ') {
                let e = word_run(b, j + 1);
                let unit = &b[j + 1..e];
                if UNITS.iter().any(|u| u.as_bytes() == unit) {
                    j = e;
                }
            }
        }
        let versioned = dots > 0 && (matches!(run[0], b'v' | b'V') || dots >= 2 || pre);
        let kind = if versioned {
            Kind::Version
        } else if run[0].is_ascii_digit() {
            Kind::Number
        } else if dots > 0 && matches!(run[0], b'v' | b'V') {
            Kind::Version
        } else {
            Kind::Identifier
        };
        return (j, Some(kind));
    }
    let snake = run.contains(&b'_');
    let camel = run
        .windows(2)
        .any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase());
    let colons = b[j..].starts_with(b"::")
        && b.get(j + 2).is_some_and(|&c| c.is_ascii_alphabetic() || c == b'_');
    let call = b[j..].starts_with(b"()");
    if !(snake || camel || colons || call) {
        return (j, None);
    }
    loop {
        let next = b.get(j + if b[j..].starts_with(b"::") { 2 } else { 1 });
        let ident_next = next.is_some_and(|&c| c.is_ascii_alphabetic() || c == b'_');
        if b[j..].starts_with(b"::") && ident_next {
            j = word_run(b, j + 2);
        } else if b.get(j) == Some(&b'.') && ident_next {
            j = word_run(b, j + 1);
        } else {
            break;
        }
    }
    if b[j..].starts_with(b"()") {
        j += 2;
    }
    (j, Some(Kind::Identifier))
}
