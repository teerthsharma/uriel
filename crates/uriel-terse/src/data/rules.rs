use crate::json::is_number;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Ctx {
    Cell,
    Field,
    Item,
}

pub(super) enum Scalar<'a> {
    Null,
    Bool(bool),
    Number(&'a str),
    Str(&'a str),
}

pub(super) fn classify(tok: &str) -> Scalar<'_> {
    match tok {
        "null" => Scalar::Null,
        "true" => Scalar::Bool(true),
        "false" => Scalar::Bool(false),
        _ if is_number(tok) => Scalar::Number(tok),
        _ => Scalar::Str(tok),
    }
}

pub(super) const KEY_STOP: &[char] = &[':', ',', '[', ']', '{', '}', '^', '"'];

fn plain_text(s: &str) -> bool {
    let mut chars = s.chars();
    let (Some(first), Some(last)) = (chars.next(), s.chars().next_back()) else {
        return false;
    };
    !first.is_whitespace() && !last.is_whitespace() && !s.chars().any(char::is_control)
}

pub(super) fn raw_ok(s: &str, ctx: Ctx) -> bool {
    if !plain_text(s) {
        return false;
    }
    if s.starts_with(['"', '{', '[', '$', '!']) {
        return false;
    }
    match ctx {
        Ctx::Field => true,
        Ctx::Cell => !s.contains([',', '"', '}']),
        Ctx::Item => !s.contains([':', '[', ']', '{', '}', ',', '"', '^']),
    }
}

pub(super) fn string_ok(s: &str, ctx: Ctx) -> bool {
    raw_ok(s, ctx) && matches!(classify(s), Scalar::Str(_))
}

pub(super) fn key_ok(k: &str) -> bool {
    !k.is_empty()
        && !k.starts_with(['$', '!'])
        && !k
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || KEY_STOP.contains(&c))
}

pub(super) fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    crate::json::write_string(&mut out, s);
    out
}

pub(super) fn key_token(k: &str) -> String {
    if key_ok(k) {
        k.to_string()
    } else {
        quote(k)
    }
}

pub(super) fn raw_token(s: &str, ctx: Ctx) -> String {
    if raw_ok(s, ctx) {
        s.to_string()
    } else {
        quote(s)
    }
}

pub(super) fn id_for(n: usize) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut n = n;
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[n % 36]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    let mut s = String::from("$");
    s.extend(out.into_iter().map(char::from));
    s
}
