use super::lex::{Tok, K};
use super::rules::{
    is_neg, Index, ABBREVIATIONS, AFTER_A_STOP, AFTER_COPULA_STOP, BEFORE_COPULA_STOP, CLOSERS,
    FILLERS, HEDGE_PREFIXES, LEADINS,
};
use super::Level;

fn match_words(t: &[Tok], i: usize, pat: &str) -> Option<usize> {
    let mut idx = i;
    for (n, w) in pat.split(' ').enumerate() {
        if n > 0 {
            if t.get(idx)?.k != K::Space {
                return None;
            }
            idx += 1;
        }
        if !t.get(idx)?.is_word(w) {
            return None;
        }
        idx += 1;
    }
    Some(idx)
}

fn neg_in(t: &[Tok]) -> usize {
    t.iter().filter(|x| x.k == K::Word && is_neg(&x.l)).count()
}

fn is_shouting(s: &str) -> bool {
    s.chars().count() >= 2 && !s.chars().any(char::is_lowercase)
}

fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {
            let mut out = String::with_capacity(s.len());
            out.push(c.to_ascii_uppercase());
            out.push_str(chars.as_str());
            out
        }
        _ => s.to_string(),
    }
}

fn push_tok(out: &mut Vec<Tok>, tok: &Tok, cap: &mut bool) {
    if tok.k == K::Word && *cap {
        out.push(Tok::word(&capitalized(&tok.t)));
    } else {
        out.push(tok.clone());
    }
    if tok.k != K::Space {
        *cap = false;
    }
}

fn real_terminator(t: &[Tok], p: usize) -> bool {
    match t[p].ch() {
        Some('!' | '?') => true,
        Some('.') => {
            if t.get(p + 1).is_some_and(|n| n.is_punct('.')) {
                return false;
            }
            if p == 0 {
                return false;
            }
            let prev = &t[p - 1];
            match prev.k {
                K::Word => {
                    prev.l.chars().count() != 1 && !ABBREVIATIONS.contains(&prev.l.as_str())
                }
                K::Punct => !prev.is_punct('.'),
                _ => true,
            }
        }
        _ => false,
    }
}

fn is_start(t: &[Tok], i: usize) -> bool {
    let mut j = i;
    let mut saw_space = false;
    while j > 0 {
        let p = &t[j - 1];
        match p.k {
            K::Space => {
                saw_space = true;
                j -= 1;
            }
            K::Newline => return true,
            K::Punct => {
                let c = p.ch().unwrap_or(' ');
                if "-*+>#([{\"'".contains(c) {
                    j -= 1;
                } else if matches!(c, '.' | '!' | '?') {
                    return saw_space && real_terminator(t, j - 1);
                } else {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

fn question_flags(t: &[Tok]) -> Vec<bool> {
    let mut flags = vec![false; t.len()];
    let mut q = false;
    for i in (0..t.len()).rev() {
        if t[i].is_punct('?') {
            q = true;
        } else if matches!(t[i].ch(), Some('.' | '!')) && real_terminator(t, i) {
            q = false;
        }
        flags[i] = q;
    }
    flags
}

pub(super) fn tidy(t: Vec<Tok>) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::with_capacity(t.len());
    for (i, tok) in t.iter().enumerate() {
        if tok.k == K::Space && i > 0 {
            let prev_space = out.last().is_some_and(|p| p.k == K::Space);
            let next_end = t.get(i + 1).is_none_or(|n| n.k == K::Newline);
            if prev_space || next_end {
                continue;
            }
        }
        out.push(tok.clone());
    }
    out
}

pub(super) fn normalize(t: Vec<Tok>) -> Vec<Tok> {
    t.into_iter()
        .enumerate()
        .map(|(i, tok)| {
            if tok.k == K::Space && i > 0 {
                Tok::space()
            } else {
                tok
            }
        })
        .collect()
}

pub(super) fn verbatim(t: &[Tok]) -> bool {
    match t.first() {
        Some(f) if f.k == K::Space => {
            f.t.contains('\t') || f.t.chars().count() >= 4 || {
                t.iter()
                    .find(|x| x.k != K::Space)
                    .is_some_and(|x| x.is_punct('|'))
            }
        }
        Some(f) => f.is_punct('|'),
        None => false,
    }
}

fn terminator_group_end(t: &[Tok], from: usize) -> usize {
    let mut k = from;
    while t.get(k).is_some_and(|x| matches!(x.ch(), Some('.' | '!' | '?'))) {
        k += 1;
    }
    k
}

fn closer(t: &[Tok], j: usize) -> Option<usize> {
    for pat in CLOSERS {
        let Some(mut k) = match_words(t, j, pat) else {
            continue;
        };
        if t.get(k).is_some_and(|x| x.k == K::Space)
            && t.get(k + 1)
                .is_some_and(|x| x.is_word("please") || x.is_word("thanks"))
        {
            k += 2;
        }
        let end = terminator_group_end(t, k);
        if end == k && k < t.len() {
            continue;
        }
        if t.get(end).is_some_and(|x| x.k != K::Space && x.k != K::Newline) {
            continue;
        }
        if neg_in(&t[j..end]) > 0 {
            continue;
        }
        return Some(end);
    }
    None
}

fn leadin(t: &[Tok], j: usize) -> Option<usize> {
    for pat in LEADINS {
        let Some(k) = match_words(t, j, pat) else {
            continue;
        };
        if !t
            .get(k)
            .is_some_and(|x| matches!(x.ch(), Some(',' | '!' | '.' | ':' | ';')))
        {
            continue;
        }
        if neg_in(&t[j..k]) > 0 {
            continue;
        }
        let mut e = k + 1;
        if t.get(e).is_some_and(|x| x.k == K::Space) {
            e += 1;
        }
        return Some(e);
    }
    None
}

fn tag_tokens() -> [Tok; 5] {
    [
        Tok::word("Me"),
        Tok::space(),
        Tok::word("not"),
        Tok::space(),
        Tok::word("sure"),
    ]
}

fn hedge(t: &[Tok], i: usize) -> Option<(usize, Vec<Tok>)> {
    let pe = HEDGE_PREFIXES
        .iter()
        .find_map(|pat| match_words(t, i, pat))?;
    if neg_in(&t[i..pe]) > 1 {
        return None;
    }
    let mut k = pe;
    let mut sep = false;
    if t.get(k).is_some_and(|x| x.is_punct(',')) {
        k += 1;
        sep = true;
    }
    let kk = if t.get(k).is_some_and(|x| x.k == K::Space) {
        k + 1
    } else {
        k
    };
    if t.get(kk).is_some_and(|x| x.is_word("but")) {
        k = kk + 1;
        sep = true;
        if t.get(k).is_some_and(|x| x.is_punct(',')) {
            k += 1;
        }
    }
    if t.get(k).is_some_and(|x| x.k == K::Space) {
        k += 1;
    }
    let rest_empty = k >= t.len()
        || matches!(t[k].ch(), Some('.' | '!' | '?'))
        || t[k].k == K::Newline;
    if rest_empty {
        let emit: Vec<Tok> = tag_tokens().into();
        return fits(t, i, k, emit);
    }
    if !sep || !t[k].is_atom() {
        return None;
    }
    let mut m = k;
    while m < t.len() {
        if matches!(t[m].ch(), Some('.' | '!' | '?')) && real_terminator(t, m) {
            break;
        }
        m += 1;
    }
    let mut cap = true;
    let mut emit = Vec::new();
    for tok in &t[k..m] {
        push_tok(&mut emit, tok, &mut cap);
    }
    let end = terminator_group_end(t, m);
    if end > m {
        emit.extend(t[m..end].iter().cloned());
    } else {
        emit.push(Tok::punct('.'));
    }
    emit.push(Tok::space());
    emit.extend(tag_tokens());
    emit.push(Tok::punct('.'));
    fits(t, i, end, emit)
}

fn fits(t: &[Tok], from: usize, to: usize, emit: Vec<Tok>) -> Option<(usize, Vec<Tok>)> {
    let before: usize = t[from..to].iter().map(|x| x.t.len()).sum();
    let after: usize = emit.iter().map(|x| x.t.len()).sum();
    (after <= before).then_some((to, emit))
}

pub(super) fn sentences(t: Vec<Tok>) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::with_capacity(t.len());
    let mut i = 0;
    let mut cap = false;
    let mut forced = false;
    while i < t.len() {
        if t[i].k == K::Word && (forced || is_start(&t, i)) {
            let mut j = i;
            let mut removed = false;
            let mut cap_flag = false;
            while j < t.len() && t[j].k == K::Word {
                if let Some(mut e) = closer(&t, j) {
                    if t.get(e).is_some_and(|x| x.k == K::Space) {
                        e += 1;
                    }
                    j = e;
                    removed = true;
                    continue;
                }
                if let Some(e) = leadin(&t, j) {
                    j = e;
                    removed = true;
                    cap_flag = true;
                    continue;
                }
                break;
            }
            if removed {
                if j >= t.len() && out.last().is_some_and(|x| x.k == K::Space) {
                    out.pop();
                }
                i = j;
                cap = cap_flag;
                forced = true;
                continue;
            }
            if let Some((next, emit)) = hedge(&t, i) {
                out.extend(emit);
                i = next;
                forced = false;
                cap = false;
                continue;
            }
        }
        forced = false;
        push_tok(&mut out, &t[i], &mut cap);
        i += 1;
    }
    out
}

fn drop_ok(t: &[Tok], i: usize, end: usize) -> bool {
    let before = i == 0
        || t[i - 1].k == K::Space
        || t[i - 1]
            .ch()
            .is_some_and(|c| "([{\"'-*#>+".contains(c));
    before
        && t.get(end).is_some_and(|x| x.k == K::Space)
        && t.get(end + 1).is_some_and(Tok::is_atom)
}

pub(super) fn phrases(t: Vec<Tok>, idx: &Index) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::with_capacity(t.len());
    let mut i = 0;
    let mut cap = false;
    while i < t.len() {
        if t[i].k == K::Word && !is_shouting(&t[i].t) {
            let mut best: Option<(usize, &'static str)> = None;
            for rule in idx.candidates(&t[i].l) {
                if let Some(end) = match_words(&t, i, rule.pat) {
                    if best.is_none_or(|(b, _)| end > b) {
                        best = Some((end, rule.rep));
                    }
                }
            }
            if let Some((end, rep)) = best {
                let rep_neg = rep.split(' ').filter(|w| is_neg(w)).count();
                if neg_in(&t[i..end]) <= rep_neg {
                    let start = is_start(&t, i);
                    if rep.is_empty() {
                        if drop_ok(&t, i, end) {
                            cap = cap || start;
                            i = end + 1;
                            continue;
                        }
                    } else {
                        let mut first = start || cap;
                        for (n, piece) in rep.split(' ').enumerate() {
                            if n > 0 {
                                out.push(Tok::space());
                            }
                            let text = if first {
                                capitalized(piece)
                            } else {
                                piece.to_string()
                            };
                            first = false;
                            out.push(Tok::word(&text));
                        }
                        cap = false;
                        i = end;
                        continue;
                    }
                }
            }
        }
        push_tok(&mut out, &t[i], &mut cap);
        i += 1;
    }
    out
}

fn prev_atom(t: &[Tok], i: usize) -> Option<&Tok> {
    if i >= 2 && t[i - 1].k == K::Space && t[i - 2].is_atom() {
        Some(&t[i - 2])
    } else {
        None
    }
}

fn next_atom(t: &[Tok], i: usize) -> Option<&Tok> {
    if t.get(i + 1).is_some_and(|x| x.k == K::Space) {
        t.get(i + 2).filter(|x| x.is_atom())
    } else {
        None
    }
}

pub(super) fn fillers(t: Vec<Tok>) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::with_capacity(t.len());
    let mut i = 0;
    let mut cap = false;
    while i < t.len() {
        let tok = &t[i];
        if tok.k == K::Word
            && FILLERS.contains(&tok.l.as_str())
            && (i == 0 || t[i - 1].k == K::Space)
            && next_atom(&t, i).is_some()
            && !prev_atom(&t, i).is_some_and(|p| p.k == K::Word && is_neg(&p.l))
        {
            let start = is_start(&t, i);
            cap = cap || start;
            i += 2;
            continue;
        }
        push_tok(&mut out, tok, &mut cap);
        i += 1;
    }
    out
}

pub(super) fn articles(t: Vec<Tok>) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::with_capacity(t.len());
    let mut i = 0;
    let mut cap = false;
    while i < t.len() {
        let tok = &t[i];
        if tok.k == K::Word && matches!(tok.l.as_str(), "a" | "an" | "the") && !is_shouting(&tok.t)
        {
            let before = i == 0
                || t[i - 1].k == K::Space
                || t[i - 1].ch().is_some_and(|c| "([{\"'-*#>+".contains(c));
            let start = is_start(&t, i);
            let lowercase = tok.t == tok.l;
            let next = next_atom(&t, i);
            let next_ok = match next {
                None => false,
                Some(_) if tok.l == "the" => true,
                Some(n) => n.k == K::Prot || !AFTER_A_STOP.contains(&n.l.as_str()),
            };
            if before && next_ok && (lowercase || start) {
                cap = cap || start;
                i += 2;
                continue;
            }
        }
        push_tok(&mut out, tok, &mut cap);
        i += 1;
    }
    out
}

pub(super) fn copulas(t: Vec<Tok>) -> Vec<Tok> {
    let q = question_flags(&t);
    let mut out: Vec<Tok> = Vec::with_capacity(t.len());
    let mut i = 0;
    let mut cap = false;
    while i < t.len() {
        let tok = &t[i];
        if tok.k == K::Word && matches!(tok.t.as_str(), "is" | "are") && !q[i] {
            let before_ok = prev_atom(&t, i).is_some_and(|p| {
                p.k == K::Prot || !BEFORE_COPULA_STOP.contains(&p.l.as_str())
            });
            let after_ok = next_atom(&t, i).is_some_and(|n| {
                n.k == K::Prot || (!is_neg(&n.l) && !AFTER_COPULA_STOP.contains(&n.l.as_str()))
            });
            if before_ok && after_ok {
                i += 2;
                continue;
            }
        }
        push_tok(&mut out, tok, &mut cap);
        i += 1;
    }
    out
}

fn marker_like(tok: &Tok) -> bool {
    match tok.k {
        K::Prot => tok.t.len() <= 3 && tok.t.bytes().all(|b| b.is_ascii_digit()),
        K::Word => tok.t.chars().count() == 1,
        _ => false,
    }
}

fn follows_tag(t: &[Tok], k: usize) -> bool {
    match_words(t, k, "me not sure").is_some()
}

fn begins_sentence(tok: &Tok) -> bool {
    match tok.k {
        K::Prot => true,
        K::Word => tok.t.chars().next().is_some_and(char::is_uppercase),
        _ => false,
    }
}

pub(super) fn split(t: Vec<Tok>) -> Vec<Tok> {
    let indent = t.first().filter(|x| x.k == K::Space).map(|x| x.t.clone());
    let mut out: Vec<Tok> = Vec::with_capacity(t.len() + 4);
    let mut atoms = 0usize;
    let mut first_marker = false;
    let mut i = 0;
    while i < t.len() {
        let tok = &t[i];
        if tok.is_atom() {
            if atoms == 0 {
                first_marker = marker_like(tok);
            }
            atoms += 1;
        }
        out.push(tok.clone());
        if matches!(tok.ch(), Some('.' | '!' | '?'))
            && !t
                .get(i + 1)
                .is_some_and(|n| matches!(n.ch(), Some('.' | '!' | '?')))
            && real_terminator(&t, i)
        {
            let marker = atoms == 1 && first_marker;
            let splittable = t.get(i + 1).is_some_and(|x| x.k == K::Space)
                && t.get(i + 2).is_some_and(begins_sentence)
                && !marker
                && !follows_tag(&t, i + 2);
            atoms = 0;
            first_marker = false;
            if splittable {
                out.push(Tok::new(K::Newline, "\n"));
                if let Some(ind) = &indent {
                    out.push(Tok::new(K::Space, ind));
                }
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

pub(super) fn strip_periods(t: Vec<Tok>) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::with_capacity(t.len());
    let mut atoms = 0usize;
    let mut first_marker = false;
    for (i, tok) in t.iter().enumerate() {
        if tok.k == K::Newline {
            atoms = 0;
        }
        if tok.is_atom() {
            if atoms == 0 {
                first_marker = marker_like(tok);
            }
            atoms += 1;
        }
        let at_end = t.get(i + 1).is_none_or(|n| n.k == K::Newline);
        if tok.is_punct('.')
            && at_end
            && i > 0
            && t[i - 1].is_atom()
            && !(atoms == 1 && first_marker)
        {
            continue;
        }
        out.push(tok.clone());
    }
    out
}

pub(super) fn run_line(line: Vec<Tok>, level: Level, idx: &Index) -> Vec<Tok> {
    if verbatim(&line) {
        return line;
    }
    let mut t = tidy(normalize(line));
    t = sentences(t);
    t = phrases(t, idx);
    t = fillers(t);
    if level >= Level::Full {
        t = articles(t);
        t = copulas(t);
        t = split(t);
    }
    if level >= Level::Ultra {
        t = strip_periods(t);
    }
    tidy(t)
}
