//! Recursive-descent parser with precedence climbing for expressions.
//!
//! Grammar notes:
//! * Keywords are matched case-insensitively on unquoted identifiers.
//! * Expression precedence (loosest first): `OR`, `XOR`, `AND`, `NOT`,
//!   comparisons, `IN`/string predicates/`=~`/`IS NULL`, `+ -`, `* / %`,
//!   unary `+ -`, `^`, postfix (`.`, `[]`, `:Label`).
//! * Backtracking is bounded by a global token-advance budget so that
//!   adversarial inputs terminate with an error instead of looping.

use crate::ast::*;
use crate::error::ParseError;
use crate::keywords::is_reserved;
use crate::lexer::{tokenize, Tok, Token};

type PResult<T> = Result<T, ParseError>;

/// Maximum syntactic nesting before the parser gives up (stack safety).
const MAX_DEPTH: usize = 48;
/// Maximum length of one left-associative operator/postfix chain.
const MAX_CHAIN: usize = 256;

pub(crate) fn parse_query(src: &str) -> PResult<Query> {
    let toks = tokenize(src)?;
    let budget = toks.len() * 64 + 1000;
    let mut p = Parser {
        src,
        toks,
        pos: 0,
        depth: 0,
        advances: 0,
        budget,
    };
    p.parse_statement()
}

struct Parser<'a> {
    src: &'a str,
    toks: Vec<Token>,
    pos: usize,
    depth: usize,
    advances: usize,
    budget: usize,
}

fn is_kw(t: &Tok, kw: &str) -> bool {
    matches!(t, Tok::Ident { text, quoted: false } if text.eq_ignore_ascii_case(kw))
}

/// True if the token can be used as a bare or quoted variable name.
fn is_var_tok(t: &Tok) -> bool {
    match t {
        Tok::Ident { text, quoted } => *quoted || !is_reserved(text),
        _ => false,
    }
}

fn is_ident_tok(t: &Tok) -> bool {
    matches!(t, Tok::Ident { .. })
}

// ---------------------------------------------------------------------------
// Token plumbing
// ---------------------------------------------------------------------------

impl Parser<'_> {
    fn tok(&self) -> &Tok {
        &self.toks[self.pos].kind
    }

    fn tok_at(&self, n: usize) -> &Tok {
        let i = (self.pos + n).min(self.toks.len() - 1);
        &self.toks[i].kind
    }

    fn advance(&mut self) {
        self.advances += 1;
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
    }

    fn describe(&self, idx: usize) -> String {
        let t = &self.toks[idx];
        match t.kind {
            Tok::Eof => "end of input".to_string(),
            _ => format!("`{}`", &self.src[t.start..t.end]),
        }
    }

    fn err_here(&self, msg: impl Into<String>) -> ParseError {
        let t = &self.toks[self.pos];
        ParseError::new(self.src, t.start, t.end, msg)
    }

    fn err_at_pos(&self, pos: usize, msg: impl Into<String>) -> ParseError {
        let t = &self.toks[pos.min(self.toks.len() - 1)];
        ParseError::new(self.src, t.start, t.end, msg)
    }

    fn unexpected(&self, expected: &str) -> ParseError {
        self.err_here(format!(
            "expected {expected}, found {}",
            self.describe(self.pos)
        ))
    }

    fn at(&self, t: &Tok) -> bool {
        self.tok() == t
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.at(t) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, t: &Tok, name: &str) -> PResult<()> {
        if self.eat(t) {
            Ok(())
        } else {
            Err(self.unexpected(name))
        }
    }

    fn at_kw(&self, kw: &str) -> bool {
        is_kw(self.tok(), kw)
    }

    fn at_kw_n(&self, n: usize, kw: &str) -> bool {
        is_kw(self.tok_at(n), kw)
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.at_kw(kw) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect_kw(&mut self, kw: &str) -> PResult<()> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            Err(self.unexpected(&format!("`{kw}`")))
        }
    }

    /// A variable name: a backtick identifier or a non-reserved word.
    fn parse_var(&mut self, what: &str) -> PResult<String> {
        match self.tok().clone() {
            Tok::Ident { text, quoted } => {
                if !quoted && is_reserved(&text) {
                    return Err(self.err_here(format!(
                        "expected {what}, found reserved word `{text}` (quote it with backticks to use it as a name)"
                    )));
                }
                self.advance();
                Ok(text)
            }
            _ => Err(self.unexpected(what)),
        }
    }

    /// Any identifier, reserved words included (labels, types, keys).
    fn parse_name(&mut self, what: &str) -> PResult<String> {
        match self.tok().clone() {
            Tok::Ident { text, .. } => {
                self.advance();
                Ok(text)
            }
            _ => Err(self.unexpected(what)),
        }
    }

    fn enter(&mut self) -> PResult<()> {
        if self.depth >= MAX_DEPTH {
            return Err(self.err_here("query is nested too deeply"));
        }
        if self.advances > self.budget {
            return Err(self.err_here("query is too complex to parse"));
        }
        self.depth += 1;
        Ok(())
    }

    fn check_chain(&self, n: usize) -> PResult<()> {
        if n > MAX_CHAIN {
            Err(self.err_here("expression chain is too long"))
        } else {
            Ok(())
        }
    }

    /// Runs `f`; on failure rewinds to the starting token and returns `None`.
    fn speculate<T>(&mut self, f: impl FnOnce(&mut Self) -> PResult<T>) -> Option<T> {
        let (pos, depth) = (self.pos, self.depth);
        match f(self) {
            Ok(v) => Some(v),
            Err(_) => {
                self.pos = pos;
                self.depth = depth;
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

fn bin(op: BinaryOp, lhs: Expr, rhs: Expr) -> Expr {
    Expr::Binary {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    }
}

impl Parser<'_> {
    fn parse_expr(&mut self) -> PResult<Expr> {
        self.enter()?;
        let r = self.parse_or();
        self.depth -= 1;
        r
    }

    fn parse_or(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_xor()?;
        let mut n = 0;
        while self.at_kw("OR") {
            self.advance();
            n += 1;
            self.check_chain(n)?;
            let rhs = self.parse_xor()?;
            lhs = bin(BinaryOp::Or, lhs, rhs);
        }
        Ok(lhs)
    }

    fn parse_xor(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_and()?;
        let mut n = 0;
        while self.at_kw("XOR") {
            self.advance();
            n += 1;
            self.check_chain(n)?;
            let rhs = self.parse_and()?;
            lhs = bin(BinaryOp::Xor, lhs, rhs);
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_not()?;
        let mut n = 0;
        while self.at_kw("AND") {
            self.advance();
            n += 1;
            self.check_chain(n)?;
            let rhs = self.parse_not()?;
            lhs = bin(BinaryOp::And, lhs, rhs);
        }
        Ok(lhs)
    }

    fn parse_not(&mut self) -> PResult<Expr> {
        if self.at_kw("NOT") {
            self.enter()?;
            self.advance();
            let e = self.parse_not();
            self.depth -= 1;
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(e?),
            });
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_predicate()?;
        let mut n = 0;
        loop {
            let op = match self.tok() {
                Tok::Eq => BinaryOp::Eq,
                Tok::Ne => BinaryOp::Ne,
                Tok::Lt => BinaryOp::Lt,
                Tok::Le => BinaryOp::Le,
                Tok::Gt => BinaryOp::Gt,
                Tok::Ge => BinaryOp::Ge,
                _ => return Ok(lhs),
            };
            self.advance();
            n += 1;
            self.check_chain(n)?;
            let rhs = self.parse_predicate()?;
            lhs = bin(op, lhs, rhs);
        }
    }

    /// `IN`, `STARTS WITH`, `ENDS WITH`, `CONTAINS`, `=~`, `IS [NOT] NULL`.
    fn parse_predicate(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_additive()?;
        let mut n = 0;
        loop {
            n += 1;
            self.check_chain(n)?;
            if self.at_kw("IN") {
                self.advance();
                let rhs = self.parse_additive()?;
                lhs = bin(BinaryOp::In, lhs, rhs);
            } else if self.at_kw("STARTS") && self.at_kw_n(1, "WITH") {
                self.advance();
                self.advance();
                let rhs = self.parse_additive()?;
                lhs = bin(BinaryOp::StartsWith, lhs, rhs);
            } else if self.at_kw("ENDS") && self.at_kw_n(1, "WITH") {
                self.advance();
                self.advance();
                let rhs = self.parse_additive()?;
                lhs = bin(BinaryOp::EndsWith, lhs, rhs);
            } else if self.at_kw("CONTAINS") {
                self.advance();
                let rhs = self.parse_additive()?;
                lhs = bin(BinaryOp::Contains, lhs, rhs);
            } else if self.at(&Tok::RegexMatch) {
                self.advance();
                let rhs = self.parse_additive()?;
                lhs = bin(BinaryOp::RegexMatch, lhs, rhs);
            } else if self.at_kw("IS") {
                self.advance();
                let negated = self.eat_kw("NOT");
                if !self.eat_kw("NULL") {
                    return Err(self.unexpected("`NULL` after `IS`"));
                }
                lhs = Expr::IsNull {
                    expr: Box::new(lhs),
                    negated,
                };
            } else {
                return Ok(lhs);
            }
        }
    }

    fn parse_additive(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_multiplicative()?;
        let mut n = 0;
        loop {
            let op = match self.tok() {
                Tok::Plus => BinaryOp::Add,
                Tok::Minus => BinaryOp::Sub,
                _ => return Ok(lhs),
            };
            self.advance();
            n += 1;
            self.check_chain(n)?;
            let rhs = self.parse_multiplicative()?;
            lhs = bin(op, lhs, rhs);
        }
    }

    fn parse_multiplicative(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_unary()?;
        let mut n = 0;
        loop {
            let op = match self.tok() {
                Tok::Star => BinaryOp::Mul,
                Tok::Slash => BinaryOp::Div,
                Tok::Percent => BinaryOp::Mod,
                _ => return Ok(lhs),
            };
            self.advance();
            n += 1;
            self.check_chain(n)?;
            let rhs = self.parse_unary()?;
            lhs = bin(op, lhs, rhs);
        }
    }

    fn parse_unary(&mut self) -> PResult<Expr> {
        let op = match self.tok() {
            Tok::Minus => UnaryOp::Neg,
            Tok::Plus => UnaryOp::Pos,
            _ => return self.parse_power(),
        };
        // `-9223372036854775808` is the one literal that only fits negated.
        if op == UnaryOp::Neg
            && *self.tok_at(1) == Tok::Integer(1u64 << 63)
            && !matches!(self.tok_at(2), Tok::Caret | Tok::Dot | Tok::LBracket)
        {
            self.advance();
            self.advance();
            return Ok(Expr::Literal(Literal::Integer(i64::MIN)));
        }
        self.enter()?;
        self.advance();
        let e = self.parse_unary();
        self.depth -= 1;
        Ok(Expr::Unary {
            op,
            expr: Box::new(e?),
        })
    }

    fn parse_power(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_labeled()?;
        let mut n = 0;
        while self.at(&Tok::Caret) {
            self.advance();
            n += 1;
            self.check_chain(n)?;
            let rhs = self.parse_power_operand()?;
            lhs = bin(BinaryOp::Pow, lhs, rhs);
        }
        Ok(lhs)
    }

    /// Right operand of `^`: allows a sign (`2 ^ -1`).
    fn parse_power_operand(&mut self) -> PResult<Expr> {
        let op = match self.tok() {
            Tok::Minus => UnaryOp::Neg,
            Tok::Plus => UnaryOp::Pos,
            _ => return self.parse_labeled(),
        };
        self.enter()?;
        self.advance();
        let e = self.parse_power_operand();
        self.depth -= 1;
        Ok(Expr::Unary {
            op,
            expr: Box::new(e?),
        })
    }

    /// Postfix expression optionally followed by `:Label:Label`.
    fn parse_labeled(&mut self) -> PResult<Expr> {
        let e = self.parse_postfix()?;
        if self.at(&Tok::Colon) && is_ident_tok(self.tok_at(1)) {
            let mut labels = Vec::new();
            while self.at(&Tok::Colon) {
                self.advance();
                labels.push(self.parse_name("label name")?);
            }
            return Ok(Expr::HasLabels {
                expr: Box::new(e),
                labels,
            });
        }
        Ok(e)
    }

    fn parse_postfix(&mut self) -> PResult<Expr> {
        let mut e = self.parse_atom()?;
        let mut n = 0;
        loop {
            match self.tok() {
                Tok::Dot => {
                    self.advance();
                    let key = self.parse_name("property name")?;
                    e = Expr::Property {
                        base: Box::new(e),
                        key,
                    };
                }
                Tok::LBracket => {
                    self.advance();
                    e = self.parse_subscript(e)?;
                }
                _ => return Ok(e),
            }
            n += 1;
            self.check_chain(n)?;
        }
    }

    /// After `[`: index or slice; consumes the closing `]`.
    fn parse_subscript(&mut self, base: Expr) -> PResult<Expr> {
        let base = Box::new(base);
        if self.eat(&Tok::DotDot) {
            let to = if self.at(&Tok::RBracket) {
                None
            } else {
                Some(Box::new(self.parse_expr()?))
            };
            self.expect(&Tok::RBracket, "`]`")?;
            return Ok(Expr::Slice {
                base,
                from: None,
                to,
            });
        }
        let first = self.parse_expr()?;
        if self.eat(&Tok::DotDot) {
            let to = if self.at(&Tok::RBracket) {
                None
            } else {
                Some(Box::new(self.parse_expr()?))
            };
            self.expect(&Tok::RBracket, "`]`")?;
            return Ok(Expr::Slice {
                base,
                from: Some(Box::new(first)),
                to,
            });
        }
        self.expect(&Tok::RBracket, "`]`")?;
        Ok(Expr::Index {
            base,
            index: Box::new(first),
        })
    }

    fn parse_atom(&mut self) -> PResult<Expr> {
        match self.tok().clone() {
            Tok::Integer(v) => {
                if v > i64::MAX as u64 {
                    return Err(self.err_here("integer literal out of range"));
                }
                self.advance();
                Ok(Expr::Literal(Literal::Integer(v as i64)))
            }
            Tok::Float(x) => {
                self.advance();
                Ok(Expr::Literal(Literal::Float(x)))
            }
            Tok::Str(s) => {
                self.advance();
                Ok(Expr::Literal(Literal::String(s)))
            }
            Tok::Param(p) => {
                self.advance();
                Ok(Expr::Parameter(p))
            }
            Tok::LParen => self.parse_paren_or_pattern(),
            Tok::LBracket => self.parse_bracket(),
            Tok::LBrace => self.parse_map_literal(),
            Tok::Ident { text, quoted } => self.parse_ident_atom(&text, quoted),
            _ => Err(self.unexpected("expression")),
        }
    }

    fn parse_ident_atom(&mut self, text: &str, quoted: bool) -> PResult<Expr> {
        if !quoted {
            let is = |kw: &str| text.eq_ignore_ascii_case(kw);
            if is("true") || is("false") {
                self.advance();
                return Ok(Expr::Literal(Literal::Bool(is("true"))));
            }
            if is("null") {
                self.advance();
                return Ok(Expr::Literal(Literal::Null));
            }
            if is("case") {
                return self.parse_case();
            }
            if is("exists") && *self.tok_at(1) == Tok::LBrace {
                return self.parse_exists();
            }
            if is("count")
                && *self.tok_at(1) == Tok::LParen
                && *self.tok_at(2) == Tok::Star
                && *self.tok_at(3) == Tok::RParen
            {
                for _ in 0..4 {
                    self.advance();
                }
                return Ok(Expr::CountStar);
            }
            let quant = if is("all") {
                Some(Quantifier::All)
            } else if is("any") {
                Some(Quantifier::Any)
            } else if is("none") {
                Some(Quantifier::None)
            } else if is("single") {
                Some(Quantifier::Single)
            } else {
                None
            };
            if let Some(kind) = quant {
                if *self.tok_at(1) == Tok::LParen
                    && is_var_tok(self.tok_at(2))
                    && self.at_kw_n(3, "IN")
                {
                    return self.parse_quantifier(kind);
                }
            }
            if is("reduce")
                && *self.tok_at(1) == Tok::LParen
                && is_var_tok(self.tok_at(2))
                && *self.tok_at(3) == Tok::Eq
            {
                return self.parse_reduce();
            }
            if is_reserved(text) {
                return Err(self.unexpected("expression"));
            }
        }
        if self.is_function_start() {
            return self.parse_function_call(quoted);
        }
        self.advance();
        Ok(Expr::Variable(text.to_string()))
    }

    /// `name(`, `ns.name(`, `a.b.c(`.
    fn is_function_start(&self) -> bool {
        let mut i = 0;
        loop {
            if !is_ident_tok(self.tok_at(i)) {
                return false;
            }
            if *self.tok_at(i + 1) == Tok::Dot && is_ident_tok(self.tok_at(i + 2)) {
                i += 2;
            } else {
                return *self.tok_at(i + 1) == Tok::LParen;
            }
        }
    }

    fn parse_function_call(&mut self, _first_quoted: bool) -> PResult<Expr> {
        let mut names = vec![self.parse_name("function name")?];
        while self.at(&Tok::Dot) {
            self.advance();
            names.push(self.parse_name("function name")?);
        }
        let name = names.pop().unwrap_or_default();
        self.expect(&Tok::LParen, "`(`")?;
        let distinct = self.eat_kw("DISTINCT");
        let mut args = Vec::new();
        if !self.at(&Tok::RParen) {
            loop {
                args.push(self.parse_expr()?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
        }
        self.expect(&Tok::RParen, "`,` or `)`")?;
        Ok(Expr::FunctionCall {
            namespace: names,
            name,
            distinct,
            args,
        })
    }

    fn parse_case(&mut self) -> PResult<Expr> {
        self.advance();
        let scrutinee = if !self.at_kw("WHEN") && !self.at_kw("END") {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        if !self.at_kw("WHEN") {
            return Err(self.unexpected("`WHEN`"));
        }
        let mut whens = Vec::new();
        while self.eat_kw("WHEN") {
            let w = self.parse_expr()?;
            self.expect_kw("THEN")?;
            let t = self.parse_expr()?;
            whens.push((w, t));
        }
        let else_expr = if self.eat_kw("ELSE") {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        self.expect_kw("END")?;
        Ok(Expr::Case {
            scrutinee,
            whens,
            else_expr,
        })
    }

    fn parse_quantifier(&mut self, kind: Quantifier) -> PResult<Expr> {
        self.advance();
        self.advance();
        let variable = self.parse_var("variable name")?;
        self.expect_kw("IN")?;
        let list = Box::new(self.parse_expr()?);
        let predicate = if self.eat_kw("WHERE") {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        self.expect(&Tok::RParen, "`)`")?;
        Ok(Expr::ListPredicate {
            kind,
            variable,
            list,
            predicate,
        })
    }

    fn parse_reduce(&mut self) -> PResult<Expr> {
        self.advance();
        self.advance();
        let accumulator = self.parse_var("accumulator name")?;
        self.expect(&Tok::Eq, "`=`")?;
        let init = Box::new(self.parse_expr()?);
        self.expect(&Tok::Comma, "`,`")?;
        let variable = self.parse_var("variable name")?;
        self.expect_kw("IN")?;
        let list = Box::new(self.parse_expr()?);
        self.expect(&Tok::Pipe, "`|`")?;
        let expr = Box::new(self.parse_expr()?);
        self.expect(&Tok::RParen, "`)`")?;
        Ok(Expr::Reduce {
            accumulator,
            init,
            variable,
            list,
            expr,
        })
    }

    fn parse_exists(&mut self) -> PResult<Expr> {
        self.advance();
        self.advance();
        let starts_query = ["MATCH", "OPTIONAL", "WITH", "UNWIND", "CALL", "RETURN"]
            .iter()
            .any(|k| self.at_kw(k));
        let body = if starts_query {
            ExistsBody::Subquery(self.parse_regular_query(false)?)
        } else {
            let pattern = self.parse_pattern()?;
            let where_clause = if self.eat_kw("WHERE") {
                Some(Box::new(self.parse_expr()?))
            } else {
                None
            };
            ExistsBody::Pattern {
                pattern,
                where_clause,
            }
        };
        self.expect(&Tok::RBrace, "`}`")?;
        Ok(Expr::Exists(Box::new(body)))
    }

    /// `{k: v, ...}` as an expression.
    fn parse_map_literal(&mut self) -> PResult<Expr> {
        self.expect(&Tok::LBrace, "`{`")?;
        let mut entries = Vec::new();
        if self.eat(&Tok::RBrace) {
            return Ok(Expr::Map(entries));
        }
        loop {
            let key = self.parse_name("map key")?;
            self.expect(&Tok::Colon, "`:`")?;
            let value = self.parse_expr()?;
            entries.push((key, value));
            if self.eat(&Tok::Comma) {
                continue;
            }
            self.expect(&Tok::RBrace, "`,` or `}`")?;
            return Ok(Expr::Map(entries));
        }
    }

    /// At `(`: pattern predicate or parenthesised expression.
    fn parse_paren_or_pattern(&mut self) -> PResult<Expr> {
        if self.looks_like_pattern() {
            let el = self.speculate(|p| {
                let el = p.parse_pattern_element()?;
                if el.steps.is_empty() {
                    Err(p.err_here("not a pattern"))
                } else {
                    Ok(el)
                }
            });
            if let Some(el) = el {
                return Ok(Expr::PatternPredicate(Box::new(el)));
            }
        }
        self.advance();
        let e = self.parse_expr()?;
        self.expect(&Tok::RParen, "`)`")?;
        Ok(e)
    }

    /// Cheap token scan: does the balanced `( ... )` at the cursor continue
    /// with a relationship arrow?
    fn looks_like_pattern(&self) -> bool {
        let mut depth = 0usize;
        let mut i = 0usize;
        loop {
            match self.tok_at(i) {
                Tok::LParen => depth += 1,
                Tok::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                Tok::Eof => return false,
                _ => {}
            }
            i += 1;
        }
        let after = self.pos + i + 1;
        self.arrow_at(after)
    }

    /// Does a relationship start at absolute token index `idx`?
    fn arrow_at(&self, idx: usize) -> bool {
        let t = |k: usize| &self.toks[(idx + k).min(self.toks.len() - 1)].kind;
        let det = |k: usize| matches!(t(k), Tok::LBracket | Tok::Minus);
        match t(0) {
            Tok::Minus => det(1),
            Tok::Lt => *t(1) == Tok::Minus && det(2),
            _ => false,
        }
    }

    /// At `[`: list literal, list comprehension or pattern comprehension.
    fn parse_bracket(&mut self) -> PResult<Expr> {
        if *self.tok_at(1) == Tok::RBracket {
            self.advance();
            self.advance();
            return Ok(Expr::List(Vec::new()));
        }
        if is_var_tok(self.tok_at(1)) && self.at_kw_n(2, "IN") {
            return self.parse_list_comprehension();
        }
        let pattern_start = *self.tok_at(1) == Tok::LParen
            || (is_var_tok(self.tok_at(1))
                && *self.tok_at(2) == Tok::Eq
                && *self.tok_at(3) == Tok::LParen);
        if pattern_start {
            if let Some(e) = self.speculate(|p| p.parse_pattern_comprehension()) {
                return Ok(e);
            }
        }
        self.advance();
        let mut items = Vec::new();
        loop {
            items.push(self.parse_expr()?);
            if self.eat(&Tok::Comma) {
                continue;
            }
            self.expect(&Tok::RBracket, "`,` or `]`")?;
            return Ok(Expr::List(items));
        }
    }

    fn parse_list_comprehension(&mut self) -> PResult<Expr> {
        self.advance();
        let variable = self.parse_var("variable name")?;
        self.expect_kw("IN")?;
        let list = Box::new(self.parse_expr()?);
        let predicate = if self.eat_kw("WHERE") {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        let projection = if self.eat(&Tok::Pipe) {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        self.expect(&Tok::RBracket, "`]`")?;
        Ok(Expr::ListComprehension {
            variable,
            list,
            predicate,
            projection,
        })
    }

    fn parse_pattern_comprehension(&mut self) -> PResult<Expr> {
        self.advance();
        let mut path_variable = None;
        if is_var_tok(self.tok()) && *self.tok_at(1) == Tok::Eq {
            path_variable = Some(self.parse_var("path variable")?);
            self.advance();
        }
        let pattern = self.parse_pattern_element()?;
        if pattern.steps.is_empty() {
            return Err(self.err_here("pattern comprehension needs a relationship"));
        }
        let predicate = if self.eat_kw("WHERE") {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        self.expect(&Tok::Pipe, "`|`")?;
        let projection = Box::new(self.parse_expr()?);
        self.expect(&Tok::RBracket, "`]`")?;
        Ok(Expr::PatternComprehension {
            path_variable,
            pattern: Box::new(pattern),
            predicate,
            projection,
        })
    }
}

// ---------------------------------------------------------------------------
// Patterns
// ---------------------------------------------------------------------------

impl Parser<'_> {
    fn parse_pattern(&mut self) -> PResult<Pattern> {
        let mut parts = vec![self.parse_pattern_part()?];
        while self.eat(&Tok::Comma) {
            parts.push(self.parse_pattern_part()?);
        }
        Ok(Pattern { parts })
    }

    fn parse_pattern_part(&mut self) -> PResult<PatternPart> {
        let mut variable = None;
        if is_var_tok(self.tok()) && *self.tok_at(1) == Tok::Eq {
            variable = Some(self.parse_var("variable name")?);
            self.advance();
        }
        let kind = if *self.tok_at(1) == Tok::LParen && self.at_kw("shortestPath") {
            PathKind::Shortest
        } else if *self.tok_at(1) == Tok::LParen && self.at_kw("allShortestPaths") {
            PathKind::AllShortest
        } else {
            PathKind::Plain
        };
        if kind == PathKind::Plain {
            let element = self.parse_pattern_element()?;
            return Ok(PatternPart {
                variable,
                kind,
                element,
            });
        }
        self.advance();
        self.advance();
        let element = self.parse_pattern_element()?;
        self.expect(&Tok::RParen, "`)`")?;
        Ok(PatternPart {
            variable,
            kind,
            element,
        })
    }

    fn parse_pattern_element(&mut self) -> PResult<PatternElement> {
        self.enter()?;
        let r = self.parse_pattern_element_inner();
        self.depth -= 1;
        r
    }

    fn parse_pattern_element_inner(&mut self) -> PResult<PatternElement> {
        let start = self.parse_node_pattern()?;
        let mut steps = Vec::new();
        while self.arrow_at(self.pos) {
            let rel = self.parse_rel_pattern()?;
            let node = self.parse_node_pattern()?;
            steps.push(PatternStep { rel, node });
        }
        Ok(PatternElement { start, steps })
    }

    fn parse_labels(&mut self) -> PResult<Vec<String>> {
        let mut labels = Vec::new();
        while self.eat(&Tok::Colon) {
            labels.push(self.parse_name("label name")?);
        }
        Ok(labels)
    }

    /// `{map}` or `$param` if present.
    fn parse_pattern_props(&mut self) -> PResult<Option<Expr>> {
        match self.tok().clone() {
            Tok::LBrace => Ok(Some(self.parse_map_literal()?)),
            Tok::Param(p) => {
                self.advance();
                Ok(Some(Expr::Parameter(p)))
            }
            _ => Ok(None),
        }
    }

    fn parse_node_pattern(&mut self) -> PResult<NodePattern> {
        self.expect(&Tok::LParen, "`(`")?;
        let variable = if matches!(self.tok(), Tok::Ident { .. }) {
            Some(self.parse_var("variable name")?)
        } else {
            None
        };
        let labels = self.parse_labels()?;
        let properties = self.parse_pattern_props()?;
        self.expect(&Tok::RParen, "`)`")?;
        Ok(NodePattern {
            variable,
            labels,
            properties,
        })
    }

    fn parse_rel_pattern(&mut self) -> PResult<RelPattern> {
        let left = self.eat(&Tok::Lt);
        self.expect(&Tok::Minus, "`-`")?;
        let mut variable = None;
        let mut types = Vec::new();
        let mut length = None;
        let mut properties = None;
        if self.eat(&Tok::LBracket) {
            if matches!(self.tok(), Tok::Ident { .. }) {
                variable = Some(self.parse_var("variable name")?);
            }
            if self.eat(&Tok::Colon) {
                types.push(self.parse_name("relationship type")?);
                while self.eat(&Tok::Pipe) {
                    self.eat(&Tok::Colon);
                    types.push(self.parse_name("relationship type")?);
                }
            }
            if self.eat(&Tok::Star) {
                length = Some(self.parse_length_range()?);
            }
            properties = self.parse_pattern_props()?;
            self.expect(&Tok::RBracket, "`]`")?;
        }
        self.expect(&Tok::Minus, "`-`")?;
        let right = self.eat(&Tok::Gt);
        let direction = match (left, right) {
            (true, false) => Direction::Left,
            (false, true) => Direction::Right,
            _ => Direction::Both,
        };
        Ok(RelPattern {
            variable,
            types,
            length,
            properties,
            direction,
        })
    }

    /// After `*`: `n`, `n..m`, `n..`, `..m`, `..` or nothing.
    fn parse_length_range(&mut self) -> PResult<LengthRange> {
        let min = self.parse_opt_hops()?;
        if self.eat(&Tok::DotDot) {
            let max = self.parse_opt_hops()?;
            return Ok(LengthRange { min, max });
        }
        Ok(LengthRange { min, max: min })
    }

    fn parse_opt_hops(&mut self) -> PResult<Option<u64>> {
        if let Tok::Integer(v) = *self.tok() {
            self.advance();
            Ok(Some(v))
        } else {
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// Clauses and queries
// ---------------------------------------------------------------------------

const CLAUSE_KEYWORDS: &[&str] = &[
    "MATCH", "OPTIONAL", "UNWIND", "CREATE", "MERGE", "SET", "REMOVE", "DELETE", "DETACH", "WITH",
    "RETURN", "CALL",
];

fn clause_name(c: &Clause) -> &'static str {
    match c {
        Clause::Match(_) => "MATCH",
        Clause::Unwind(_) => "UNWIND",
        Clause::Create(_) => "CREATE",
        Clause::Merge(_) => "MERGE",
        Clause::Set(_) => "SET",
        Clause::Remove(_) => "REMOVE",
        Clause::Delete(_) => "DELETE",
        Clause::With(_) => "WITH",
        Clause::Return(_) => "RETURN",
        Clause::Call(_) | Clause::Subquery(_) => "CALL",
    }
}

impl Parser<'_> {
    fn at_clause_start(&self) -> bool {
        CLAUSE_KEYWORDS.iter().any(|k| self.at_kw(k))
    }

    /// `single_query (UNION [ALL] single_query)*`. With `validate`, each
    /// part must end in `RETURN` or an updating clause.
    fn parse_regular_query(&mut self, validate: bool) -> PResult<RegularQuery> {
        self.enter()?;
        let r = self.parse_regular_query_inner(validate);
        self.depth -= 1;
        r
    }

    fn parse_regular_query_inner(&mut self, validate: bool) -> PResult<RegularQuery> {
        let first = self.parse_single_query(validate)?;
        let mut unions = Vec::new();
        while self.at_kw("UNION") {
            self.advance();
            let all = self.eat_kw("ALL");
            let query = self.parse_single_query(validate)?;
            unions.push(UnionPart { all, query });
        }
        Ok(RegularQuery { first, unions })
    }

    fn parse_single_query(&mut self, validate: bool) -> PResult<SingleQuery> {
        if !self.at_clause_start() {
            return Err(self.unexpected("a clause (`MATCH`, `WITH`, `RETURN`, `CREATE`, ...)"));
        }
        let mut clauses: Vec<Clause> = Vec::new();
        let mut last_start = self.pos;
        while self.at_clause_start() {
            if matches!(clauses.last(), Some(Clause::Return(_))) {
                return Err(
                    self.err_at_pos(last_start, "RETURN must be the last clause of a query")
                );
            }
            let start = self.pos;
            let clause = self.parse_clause()?;
            last_start = start;
            clauses.push(clause);
        }
        // Only validate when a legitimate terminator follows; otherwise the
        // caller reports the unexpected token, which is the more useful error.
        let terminated =
            matches!(self.tok(), Tok::Eof | Tok::Semicolon | Tok::RBrace) || self.at_kw("UNION");
        if validate && terminated {
            if let Some(last) = clauses.last() {
                let ok = matches!(
                    last,
                    Clause::Return(_)
                        | Clause::Create(_)
                        | Clause::Merge(_)
                        | Clause::Set(_)
                        | Clause::Remove(_)
                        | Clause::Delete(_)
                        | Clause::Call(_)
                        | Clause::Subquery(_)
                );
                if !ok {
                    return Err(self.err_at_pos(
                        last_start,
                        format!(
                            "query cannot end with {}; expected RETURN or an updating clause",
                            clause_name(last)
                        ),
                    ));
                }
            }
        }
        Ok(SingleQuery { clauses })
    }

    fn parse_clause(&mut self) -> PResult<Clause> {
        if self.at_kw("OPTIONAL") && !self.at_kw_n(1, "MATCH") {
            self.advance();
            return Err(self.unexpected("`MATCH` after `OPTIONAL`"));
        }
        if self.at_kw("MATCH") || self.at_kw("OPTIONAL") {
            let optional = self.eat_kw("OPTIONAL");
            self.expect_kw("MATCH")?;
            let pattern = self.parse_pattern()?;
            let where_clause = if self.eat_kw("WHERE") {
                Some(self.parse_expr()?)
            } else {
                None
            };
            return Ok(Clause::Match(MatchClause {
                optional,
                pattern,
                where_clause,
            }));
        }
        if self.eat_kw("UNWIND") {
            let expr = self.parse_expr()?;
            self.expect_kw("AS")?;
            let variable = self.parse_var("variable name")?;
            return Ok(Clause::Unwind(UnwindClause { expr, variable }));
        }
        if self.eat_kw("CREATE") {
            let pattern = self.parse_pattern()?;
            return Ok(Clause::Create(CreateClause { pattern }));
        }
        if self.eat_kw("MERGE") {
            return self.parse_merge();
        }
        if self.eat_kw("SET") {
            return Ok(Clause::Set(SetClause {
                items: self.parse_set_items()?,
            }));
        }
        if self.eat_kw("REMOVE") {
            return self.parse_remove();
        }
        if self.at_kw("DELETE") || self.at_kw("DETACH") {
            let detach = self.eat_kw("DETACH");
            self.expect_kw("DELETE")?;
            let mut exprs = vec![self.parse_expr()?];
            while self.eat(&Tok::Comma) {
                exprs.push(self.parse_expr()?);
            }
            return Ok(Clause::Delete(DeleteClause { detach, exprs }));
        }
        if self.eat_kw("WITH") {
            let body = self.parse_projection_body()?;
            let where_clause = if self.eat_kw("WHERE") {
                Some(self.parse_expr()?)
            } else {
                None
            };
            return Ok(Clause::With(WithClause { body, where_clause }));
        }
        if self.eat_kw("RETURN") {
            let body = self.parse_projection_body()?;
            return Ok(Clause::Return(ReturnClause { body }));
        }
        if self.eat_kw("CALL") {
            return self.parse_call();
        }
        Err(self.unexpected("a clause"))
    }

    fn parse_merge(&mut self) -> PResult<Clause> {
        let pattern = self.parse_pattern_part()?;
        let mut actions = Vec::new();
        while self.eat_kw("ON") {
            let event = if self.eat_kw("CREATE") {
                MergeEvent::Create
            } else if self.eat_kw("MATCH") {
                MergeEvent::Match
            } else {
                return Err(self.unexpected("`CREATE` or `MATCH` after `ON`"));
            };
            self.expect_kw("SET")?;
            let items = self.parse_set_items()?;
            actions.push(MergeAction { event, items });
        }
        Ok(Clause::Merge(MergeClause { pattern, actions }))
    }

    fn parse_set_items(&mut self) -> PResult<Vec<SetItem>> {
        let mut items = vec![self.parse_set_item()?];
        while self.eat(&Tok::Comma) {
            items.push(self.parse_set_item()?);
        }
        Ok(items)
    }

    fn parse_set_item(&mut self) -> PResult<SetItem> {
        let target = self.parse_postfix()?;
        match target {
            Expr::Variable(variable) => {
                if self.eat(&Tok::Eq) {
                    let value = self.parse_expr()?;
                    Ok(SetItem::Replace { variable, value })
                } else if self.eat(&Tok::PlusEq) {
                    let value = self.parse_expr()?;
                    Ok(SetItem::Merge { variable, value })
                } else if self.at(&Tok::Colon) {
                    let labels = self.parse_labels()?;
                    Ok(SetItem::Labels { variable, labels })
                } else {
                    Err(self.unexpected("`=`, `+=` or a label"))
                }
            }
            target @ Expr::Property { .. } => {
                self.expect(&Tok::Eq, "`=`")?;
                let value = self.parse_expr()?;
                Ok(SetItem::Property { target, value })
            }
            _ => Err(self.err_here("expected a variable or property to SET")),
        }
    }

    fn parse_remove(&mut self) -> PResult<Clause> {
        let mut items = Vec::new();
        loop {
            let target = self.parse_postfix()?;
            match target {
                Expr::Variable(variable) if self.at(&Tok::Colon) => {
                    let labels = self.parse_labels()?;
                    items.push(RemoveItem::Labels { variable, labels });
                }
                target @ Expr::Property { .. } => items.push(RemoveItem::Property(target)),
                _ => return Err(self.unexpected("a property or `variable:Label` to remove")),
            }
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        Ok(Clause::Remove(RemoveClause { items }))
    }

    fn parse_projection_body(&mut self) -> PResult<ProjectionBody> {
        let distinct = self.eat_kw("DISTINCT");
        let include_existing = self.eat(&Tok::Star);
        let mut items = Vec::new();
        if !include_existing || self.eat(&Tok::Comma) {
            loop {
                let expr = self.parse_expr()?;
                let alias = if self.eat_kw("AS") {
                    Some(self.parse_var("alias")?)
                } else {
                    None
                };
                items.push(ProjectionItem { expr, alias });
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
        }
        let mut order_by = Vec::new();
        if self.at_kw("ORDER") {
            self.advance();
            self.expect_kw("BY")?;
            loop {
                let expr = self.parse_expr()?;
                let descending = if self.eat_kw("DESC") || self.eat_kw("DESCENDING") {
                    true
                } else {
                    let _ = self.eat_kw("ASC") || self.eat_kw("ASCENDING");
                    false
                };
                order_by.push(SortItem { expr, descending });
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
        }
        let skip = if self.eat_kw("SKIP") {
            Some(self.parse_expr()?)
        } else {
            None
        };
        let limit = if self.eat_kw("LIMIT") {
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(ProjectionBody {
            distinct,
            include_existing,
            items,
            order_by,
            skip,
            limit,
        })
    }

    /// After `CALL`: subquery or procedure call.
    fn parse_call(&mut self) -> PResult<Clause> {
        if self.at(&Tok::LBrace) || self.at(&Tok::LParen) {
            let mut scope = None;
            if self.eat(&Tok::LParen) {
                let mut vars = Vec::new();
                if !self.at(&Tok::RParen) {
                    loop {
                        vars.push(self.parse_var("variable name")?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                }
                self.expect(&Tok::RParen, "`)`")?;
                scope = Some(vars);
            }
            self.expect(&Tok::LBrace, "`{`")?;
            let query = self.parse_regular_query(true)?;
            self.expect(&Tok::RBrace, "`}`")?;
            return Ok(Clause::Subquery(SubqueryClause { scope, query }));
        }
        let mut names = vec![self.parse_name("procedure name")?];
        while self.eat(&Tok::Dot) {
            names.push(self.parse_name("procedure name")?);
        }
        let name = names.pop().unwrap_or_default();
        let mut args = Vec::new();
        if self.eat(&Tok::LParen) {
            if !self.at(&Tok::RParen) {
                loop {
                    args.push(self.parse_expr()?);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
            }
            self.expect(&Tok::RParen, "`,` or `)`")?;
        }
        let yield_clause = if self.eat_kw("YIELD") {
            Some(self.parse_yield()?)
        } else {
            None
        };
        Ok(Clause::Call(CallClause {
            namespace: names,
            name,
            args,
            yield_clause,
        }))
    }

    fn parse_yield(&mut self) -> PResult<YieldClause> {
        let star = self.eat(&Tok::Star);
        let mut items = Vec::new();
        if !star {
            loop {
                let name = self.parse_name("result column")?;
                let alias = if self.eat_kw("AS") {
                    Some(self.parse_var("alias")?)
                } else {
                    None
                };
                items.push(YieldItem { name, alias });
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
        }
        let where_clause = if self.eat_kw("WHERE") {
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(YieldClause {
            star,
            items,
            where_clause,
        })
    }

    fn parse_statement(&mut self) -> PResult<Query> {
        let mode = if self.eat_kw("EXPLAIN") {
            Some(QueryMode::Explain)
        } else if self.eat_kw("PROFILE") {
            Some(QueryMode::Profile)
        } else {
            None
        };
        let body = if self.at_schema_start() {
            QueryBody::Schema(self.parse_schema()?)
        } else {
            QueryBody::Regular(self.parse_regular_query(true)?)
        };
        self.eat(&Tok::Semicolon);
        if !self.at(&Tok::Eof) {
            return Err(self.err_here(format!(
                "unexpected {}, expected a clause, `UNION` or end of input",
                self.describe(self.pos)
            )));
        }
        Ok(Query { mode, body })
    }
}

// ---------------------------------------------------------------------------
// Schema commands
// ---------------------------------------------------------------------------

impl Parser<'_> {
    fn at_schema_start(&self) -> bool {
        let next = |kws: &[&str]| kws.iter().any(|k| self.at_kw_n(1, k));
        (self.at_kw("CREATE") && next(&["INDEX", "RANGE", "TEXT", "POINT", "CONSTRAINT"]))
            || (self.at_kw("DROP") && next(&["INDEX", "CONSTRAINT"]))
            || (self.at_kw("SHOW") && next(&["INDEX", "INDEXES", "CONSTRAINT", "CONSTRAINTS"]))
    }

    fn parse_schema(&mut self) -> PResult<SchemaCommand> {
        if self.eat_kw("SHOW") {
            if self.eat_kw("INDEX") || self.eat_kw("INDEXES") {
                return Ok(SchemaCommand::ShowIndexes);
            }
            self.advance();
            return Ok(SchemaCommand::ShowConstraints);
        }
        if self.eat_kw("DROP") {
            let is_index = self.at_kw("INDEX");
            self.advance();
            let name = self.parse_name("name")?;
            let if_exists = self.at_kw("IF") && self.at_kw_n(1, "EXISTS");
            if if_exists {
                self.advance();
                self.advance();
            }
            return Ok(if is_index {
                SchemaCommand::DropIndex { name, if_exists }
            } else {
                SchemaCommand::DropConstraint { name, if_exists }
            });
        }
        self.expect_kw("CREATE")?;
        let kind = if self.eat_kw("RANGE") {
            Some(IndexKind::Range)
        } else if self.eat_kw("TEXT") {
            Some(IndexKind::Text)
        } else if self.eat_kw("POINT") {
            Some(IndexKind::Point)
        } else {
            None
        };
        if self.at_kw("INDEX") || kind.is_some() {
            self.expect_kw("INDEX")?;
            return self.parse_create_index(kind);
        }
        self.expect_kw("CONSTRAINT")?;
        self.parse_create_constraint()
    }

    /// Optional schema object name before `IF NOT EXISTS` / `FOR` / `ON`.
    fn parse_opt_schema_name(&mut self) -> PResult<Option<String>> {
        let is_name = match self.tok() {
            Tok::Ident { quoted: true, .. } => true,
            Tok::Ident { .. } => {
                !(self.at_kw("FOR")
                    || self.at_kw("ON")
                    || (self.at_kw("IF") && self.at_kw_n(1, "NOT")))
            }
            _ => false,
        };
        if is_name {
            Ok(Some(self.parse_name("name")?))
        } else {
            Ok(None)
        }
    }

    fn parse_if_not_exists(&mut self) -> bool {
        let yes = self.at_kw("IF") && self.at_kw_n(1, "NOT") && self.at_kw_n(2, "EXISTS");
        if yes {
            for _ in 0..3 {
                self.advance();
            }
        }
        yes
    }

    fn parse_create_index(&mut self, kind: Option<IndexKind>) -> PResult<SchemaCommand> {
        let name = self.parse_opt_schema_name()?;
        let if_not_exists = self.parse_if_not_exists();
        let (target, properties) = if self.eat_kw("FOR") {
            let target = self.parse_schema_target()?;
            self.expect_kw("ON")?;
            let props = self.parse_schema_props(schema_var(&target), false)?;
            (target, props)
        } else if self.eat_kw("ON") {
            // Legacy `ON :Label(prop, ...)`.
            self.expect(&Tok::Colon, "`:`")?;
            let label = self.parse_name("label name")?;
            self.expect(&Tok::LParen, "`(`")?;
            let mut props = vec![self.parse_name("property name")?];
            while self.eat(&Tok::Comma) {
                props.push(self.parse_name("property name")?);
            }
            self.expect(&Tok::RParen, "`)`")?;
            (
                SchemaTarget::Node {
                    variable: "n".to_string(),
                    label,
                },
                props,
            )
        } else {
            return Err(self.unexpected("`FOR` or `ON`"));
        };
        Ok(SchemaCommand::CreateIndex(CreateIndex {
            name,
            kind,
            if_not_exists,
            target,
            properties,
        }))
    }

    fn parse_create_constraint(&mut self) -> PResult<SchemaCommand> {
        let name = self.parse_opt_schema_name()?;
        let if_not_exists = self.parse_if_not_exists();
        let target;
        let properties;
        let mut kind = None;
        if self.eat_kw("FOR") {
            target = self.parse_schema_target()?;
            self.expect_kw("REQUIRE")?;
            properties = self.parse_schema_props(schema_var(&target), true)?;
        } else if self.eat_kw("ON") {
            target = self.parse_schema_target()?;
            self.expect_kw("ASSERT")?;
            if self.at_kw("EXISTS") && *self.tok_at(1) == Tok::LParen {
                self.advance();
                self.advance();
                properties = vec![self.parse_schema_prop_ref(schema_var(&target))?];
                self.expect(&Tok::RParen, "`)`")?;
                kind = Some(ConstraintKind::NotNull);
            } else {
                properties = self.parse_schema_props(schema_var(&target), true)?;
            }
        } else {
            return Err(self.unexpected("`FOR` or `ON`"));
        }
        let kind = match kind {
            Some(k) => k,
            None => {
                self.expect_kw("IS")?;
                if self.eat_kw("UNIQUE") {
                    ConstraintKind::Unique
                } else if self.eat_kw("NOT") {
                    self.expect_kw("NULL")?;
                    ConstraintKind::NotNull
                } else if self.eat_kw("NODE") {
                    self.expect_kw("KEY")?;
                    ConstraintKind::NodeKey
                } else {
                    return Err(self.unexpected("`UNIQUE`, `NOT NULL` or `NODE KEY`"));
                }
            }
        };
        Ok(SchemaCommand::CreateConstraint(CreateConstraint {
            name,
            if_not_exists,
            target,
            properties,
            kind,
        }))
    }

    /// `(var:Label)` or `()-[var:TYPE]-()`.
    fn parse_schema_target(&mut self) -> PResult<SchemaTarget> {
        self.expect(&Tok::LParen, "`(`")?;
        if self.eat(&Tok::RParen) {
            self.expect(&Tok::Minus, "`-`")?;
            self.expect(&Tok::LBracket, "`[`")?;
            let variable = self.parse_var("variable name")?;
            self.expect(&Tok::Colon, "`:`")?;
            let rel_type = self.parse_name("relationship type")?;
            self.expect(&Tok::RBracket, "`]`")?;
            self.expect(&Tok::Minus, "`-`")?;
            self.eat(&Tok::Gt);
            self.expect(&Tok::LParen, "`(`")?;
            self.expect(&Tok::RParen, "`)`")?;
            return Ok(SchemaTarget::Relationship { variable, rel_type });
        }
        let variable = self.parse_var("variable name")?;
        self.expect(&Tok::Colon, "`:`")?;
        let label = self.parse_name("label name")?;
        self.expect(&Tok::RParen, "`)`")?;
        Ok(SchemaTarget::Node { variable, label })
    }

    /// `var.prop`, checking that `var` is the target's variable.
    fn parse_schema_prop_ref(&mut self, var: &str) -> PResult<String> {
        let at = self.pos;
        let found = self.parse_var("variable name")?;
        if found != var {
            return Err(self.err_at_pos(at, format!("expected variable `{var}`, found `{found}`")));
        }
        self.expect(&Tok::Dot, "`.`")?;
        self.parse_name("property name")
    }

    /// `(v.a, v.b)`, or (when `allow_bare`) a single `v.a`.
    fn parse_schema_props(&mut self, var: &str, allow_bare: bool) -> PResult<Vec<String>> {
        if allow_bare && !self.at(&Tok::LParen) {
            return Ok(vec![self.parse_schema_prop_ref(var)?]);
        }
        self.expect(&Tok::LParen, "`(`")?;
        let mut props = vec![self.parse_schema_prop_ref(var)?];
        while self.eat(&Tok::Comma) {
            props.push(self.parse_schema_prop_ref(var)?);
        }
        self.expect(&Tok::RParen, "`)`")?;
        Ok(props)
    }
}

fn schema_var(t: &SchemaTarget) -> &str {
    match t {
        SchemaTarget::Node { variable, .. } | SchemaTarget::Relationship { variable, .. } => {
            variable
        }
    }
}
