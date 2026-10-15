//! Hand-written lexer. Keywords are not distinguished here: they are
//! identifiers that the parser matches case-insensitively, which lets every
//! keyword double as a property name, label or relationship type.

use crate::error::ParseError;

/// Token kinds.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Tok {
    /// Identifier or keyword; `quoted` is true for backtick identifiers.
    Ident {
        text: String,
        quoted: bool,
    },
    /// Integer literal (up to 2^63, so that `-9223372036854775808` works).
    Integer(u64),
    Float(f64),
    Str(String),
    /// `$name` parameter.
    Param(String),
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Semicolon,
    Dot,
    DotDot,
    Pipe,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    PlusEq,
    RegexMatch,
    Eof,
}

/// A token with its byte span.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Token {
    pub kind: Tok,
    pub start: usize,
    pub end: usize,
}

const INT_LIMIT: u64 = 1 << 63;

/// Splits `src` into tokens, ending with [`Tok::Eof`].
pub(crate) fn tokenize(src: &str) -> Result<Vec<Token>, ParseError> {
    let mut lx = Lexer { src, pos: 0 };
    let mut out = Vec::new();
    loop {
        lx.skip_trivia()?;
        let start = lx.pos;
        let Some(c) = lx.peek() else {
            out.push(Token {
                kind: Tok::Eof,
                start,
                end: start,
            });
            return Ok(out);
        };
        let kind = lx.next_token(c)?;
        out.push(Token {
            kind,
            start,
            end: lx.pos,
        });
    }
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_part(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Lexer<'_> {
    fn err(&self, start: usize, end: usize, msg: impl Into<String>) -> ParseError {
        ParseError::new(self.src, start, end, msg)
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek2(&self) -> Option<char> {
        let mut it = self.src[self.pos..].chars();
        it.next();
        it.next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += c.len_utf8();
            true
        } else {
            false
        }
    }

    fn skip_trivia(&mut self) -> Result<(), ParseError> {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.bump();
                }
                Some('/') if self.peek2() == Some('/') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                Some('/') if self.peek2() == Some('*') => {
                    let start = self.pos;
                    self.pos += 2;
                    match self.src[self.pos..].find("*/") {
                        Some(i) => self.pos += i + 2,
                        None => {
                            return Err(self.err(
                                start,
                                self.src.len(),
                                "unterminated block comment (missing `*/`)",
                            ))
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn next_token(&mut self, c: char) -> Result<Tok, ParseError> {
        let start = self.pos;
        if is_ident_start(c) {
            return Ok(self.ident());
        }
        if c.is_ascii_digit() || (c == '.' && self.peek2().is_some_and(|d| d.is_ascii_digit())) {
            return self.number();
        }
        self.bump();
        let tok = match c {
            '(' => Tok::LParen,
            ')' => Tok::RParen,
            '[' => Tok::LBracket,
            ']' => Tok::RBracket,
            '{' => Tok::LBrace,
            '}' => Tok::RBrace,
            ',' => Tok::Comma,
            ':' => Tok::Colon,
            ';' => Tok::Semicolon,
            '|' => Tok::Pipe,
            '-' => Tok::Minus,
            '*' => Tok::Star,
            '/' => Tok::Slash,
            '%' => Tok::Percent,
            '^' => Tok::Caret,
            '.' => {
                if self.eat('.') {
                    Tok::DotDot
                } else {
                    Tok::Dot
                }
            }
            '+' => {
                if self.eat('=') {
                    Tok::PlusEq
                } else {
                    Tok::Plus
                }
            }
            '=' => {
                if self.eat('~') {
                    Tok::RegexMatch
                } else {
                    Tok::Eq
                }
            }
            '<' => {
                if self.eat('>') {
                    Tok::Ne
                } else if self.eat('=') {
                    Tok::Le
                } else {
                    Tok::Lt
                }
            }
            '>' => {
                if self.eat('=') {
                    Tok::Ge
                } else {
                    Tok::Gt
                }
            }
            '\'' | '"' => return self.string(start, c),
            '`' => return self.backtick(start),
            '$' => return self.param(start),
            other => {
                return Err(self.err(start, self.pos, format!("unexpected character `{other}`")))
            }
        };
        Ok(tok)
    }

    fn ident(&mut self) -> Tok {
        let start = self.pos;
        while self.peek().is_some_and(is_ident_part) {
            self.bump();
        }
        Tok::Ident {
            text: self.src[start..self.pos].to_string(),
            quoted: false,
        }
    }

    /// Reads a backtick-quoted name; the opening backtick is already consumed.
    fn quoted_name(&mut self, start: usize) -> Result<String, ParseError> {
        let mut text = String::new();
        loop {
            match self.bump() {
                None => {
                    return Err(self.err(
                        start,
                        self.src.len(),
                        "unterminated backtick identifier (missing closing `)",
                    ))
                }
                Some('`') => {
                    if self.eat('`') {
                        text.push('`');
                    } else {
                        break;
                    }
                }
                Some(c) => text.push(c),
            }
        }
        if text.is_empty() {
            return Err(self.err(start, self.pos, "empty backtick identifier"));
        }
        Ok(text)
    }

    fn backtick(&mut self, start: usize) -> Result<Tok, ParseError> {
        let text = self.quoted_name(start)?;
        Ok(Tok::Ident { text, quoted: true })
    }

    fn param(&mut self, start: usize) -> Result<Tok, ParseError> {
        match self.peek() {
            Some('`') => {
                self.bump();
                let name = self.quoted_name(self.pos - 1)?;
                Ok(Tok::Param(name))
            }
            Some(c) if is_ident_part(c) => {
                let s = self.pos;
                while self.peek().is_some_and(is_ident_part) {
                    self.bump();
                }
                Ok(Tok::Param(self.src[s..self.pos].to_string()))
            }
            _ => Err(self.err(start, self.pos, "expected a parameter name after `$`")),
        }
    }

    fn string(&mut self, start: usize, quote: char) -> Result<Tok, ParseError> {
        let mut out = String::new();
        loop {
            let at = self.pos;
            let Some(c) = self.bump() else {
                return Err(self.err(
                    start,
                    self.src.len(),
                    format!("unterminated string literal (missing closing {quote})"),
                ));
            };
            if c == quote {
                return Ok(Tok::Str(out));
            }
            if c != '\\' {
                out.push(c);
                continue;
            }
            let Some(e) = self.bump() else {
                return Err(self.err(
                    start,
                    self.src.len(),
                    format!("unterminated string literal (missing closing {quote})"),
                ));
            };
            match e {
                '\\' => out.push('\\'),
                '\'' => out.push('\''),
                '"' => out.push('"'),
                '`' => out.push('`'),
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'u' | 'U' => {
                    let n = if e == 'u' { 4 } else { 8 };
                    let hex_start = self.pos;
                    let mut v: u32 = 0;
                    for _ in 0..n {
                        let Some(d) = self.peek().and_then(|d| d.to_digit(16)) else {
                            return Err(self.err(
                                at,
                                self.pos,
                                format!("invalid unicode escape: expected {n} hex digits"),
                            ));
                        };
                        self.bump();
                        v = v.wrapping_mul(16).wrapping_add(d);
                    }
                    match char::from_u32(v) {
                        Some(ch) => out.push(ch),
                        None => {
                            return Err(self.err(
                                at,
                                hex_start + n,
                                "invalid unicode escape: not a valid scalar value",
                            ))
                        }
                    }
                }
                other => {
                    return Err(self.err(
                        at,
                        self.pos,
                        format!("invalid escape sequence `\\{other}`"),
                    ))
                }
            }
        }
    }

    fn number(&mut self) -> Result<Tok, ParseError> {
        let start = self.pos;
        let first = self.peek();
        let second = self.peek2();
        // Hex / octal with prefix.
        if first == Some('0') && matches!(second, Some('x' | 'X' | 'o' | 'O')) {
            let radix = if matches!(second, Some('x' | 'X')) {
                16
            } else {
                8
            };
            self.pos += 2;
            let ds = self.pos;
            while self.peek().is_some_and(|c| c.is_ascii_alphanumeric()) {
                self.bump();
            }
            let digits = &self.src[ds..self.pos];
            return self.finish_int(start, digits, radix);
        }
        let mut is_float = false;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
        }
        if self.peek() == Some('.') && self.peek2().is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            self.bump();
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.bump();
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            let save = self.pos;
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                is_float = true;
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.bump();
                }
            } else {
                self.pos = save;
            }
        }
        if self.peek().is_some_and(is_ident_part) {
            let mut end = self.pos;
            while self.src[end..].chars().next().is_some_and(is_ident_part) {
                end += self.src[end..].chars().next().map_or(1, char::len_utf8);
            }
            return Err(self.err(start, end, "invalid numeric literal"));
        }
        let text = &self.src[start..self.pos];
        if is_float {
            return match text.parse::<f64>() {
                Ok(x) if x.is_finite() => Ok(Tok::Float(x)),
                _ => Err(self.err(start, self.pos, "float literal out of range")),
            };
        }
        // Leading zero => legacy octal (`017`).
        if text.len() > 1 && text.starts_with('0') {
            return self.finish_int(start, &text[1..], 8);
        }
        self.finish_int(start, text, 10)
    }

    fn finish_int(&self, start: usize, digits: &str, radix: u32) -> Result<Tok, ParseError> {
        if digits.is_empty() {
            return Err(self.err(start, self.pos, "invalid numeric literal: missing digits"));
        }
        match u64::from_str_radix(digits, radix) {
            Ok(v) if v <= INT_LIMIT => Ok(Tok::Integer(v)),
            Ok(_) => Err(self.err(start, self.pos, "integer literal out of range")),
            Err(_) => {
                let kind = match radix {
                    16 => "hexadecimal",
                    8 => "octal",
                    _ => "decimal",
                };
                Err(self.err(start, self.pos, format!("invalid {kind} integer literal")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<Tok> {
        tokenize(src).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn lex_numbers() {
        assert_eq!(kinds("42")[0], Tok::Integer(42));
        assert_eq!(kinds("0x1F")[0], Tok::Integer(31));
        assert_eq!(kinds("0o17")[0], Tok::Integer(15));
        assert_eq!(kinds("017")[0], Tok::Integer(15));
        assert_eq!(kinds("1.5e3")[0], Tok::Float(1500.0));
        assert_eq!(kinds(".5")[0], Tok::Float(0.5));
        assert_eq!(kinds("1e3")[0], Tok::Float(1000.0));
    }

    #[test]
    fn lex_range_is_not_float() {
        assert_eq!(
            kinds("1..3"),
            vec![Tok::Integer(1), Tok::DotDot, Tok::Integer(3), Tok::Eof]
        );
    }

    #[test]
    fn lex_operators() {
        assert_eq!(
            kinds("<> <= >= =~ += .."),
            vec![
                Tok::Ne,
                Tok::Le,
                Tok::Ge,
                Tok::RegexMatch,
                Tok::PlusEq,
                Tok::DotDot,
                Tok::Eof
            ]
        );
    }

    #[test]
    fn lex_comments_skipped() {
        assert_eq!(kinds("// hi\n/* x */ 1").len(), 2);
    }

    #[test]
    fn lex_string_escapes() {
        assert_eq!(kinds(r"'a\n\u0041\''")[0], Tok::Str("a\nA'".into()));
    }

    #[test]
    fn lex_backtick_and_param() {
        assert_eq!(
            kinds("`a``b` $p $`x y`"),
            vec![
                Tok::Ident {
                    text: "a`b".into(),
                    quoted: true
                },
                Tok::Param("p".into()),
                Tok::Param("x y".into()),
                Tok::Eof
            ]
        );
    }
}
