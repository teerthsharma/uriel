use crate::protected::scan;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum K {
    Word,
    Space,
    Punct,
    Prot,
    Newline,
}

#[derive(Clone, Debug)]
pub(super) struct Tok {
    pub k: K,
    pub t: String,
    pub l: String,
}

fn norm(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\u{2019}' { '\'' } else { c })
        .flat_map(char::to_lowercase)
        .collect()
}

impl Tok {
    pub fn new(k: K, t: &str) -> Tok {
        let l = if k == K::Word { norm(t) } else { String::new() };
        Tok {
            k,
            t: t.to_string(),
            l,
        }
    }

    pub fn word(t: &str) -> Tok {
        Tok::new(K::Word, t)
    }

    pub fn space() -> Tok {
        Tok::new(K::Space, " ")
    }

    pub fn punct(c: char) -> Tok {
        Tok::new(K::Punct, c.encode_utf8(&mut [0; 4]))
    }

    pub fn is_atom(&self) -> bool {
        matches!(self.k, K::Word | K::Prot)
    }

    pub fn ch(&self) -> Option<char> {
        if self.k == K::Punct {
            self.t.chars().next()
        } else {
            None
        }
    }

    pub fn is_punct(&self, c: char) -> bool {
        self.ch() == Some(c)
    }

    pub fn is_word(&self, w: &str) -> bool {
        self.k == K::Word && self.l == w
    }
}

pub(super) fn tokenize(text: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut p = 0;
    for sp in scan(text) {
        gap(&text[p..sp.start], &mut out);
        out.push(Tok::new(K::Prot, &text[sp.start..sp.end]));
        p = sp.end;
    }
    gap(&text[p..], &mut out);
    out
}

fn gap(s: &str, out: &mut Vec<Tok>) {
    let cs: Vec<(usize, char)> = s.char_indices().collect();
    let at = |j: usize| cs.get(j).map_or(s.len(), |x| x.0);
    let is_nl = |j: usize| match cs.get(j).map(|x| x.1) {
        Some('\n') => true,
        Some('\r') => cs.get(j + 1).map(|x| x.1) == Some('\n'),
        _ => false,
    };
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i].1;
        if c == '\n' {
            out.push(Tok::new(K::Newline, "\n"));
            i += 1;
        } else if c == '\r' && is_nl(i) {
            out.push(Tok::new(K::Newline, "\r\n"));
            i += 2;
        } else if c.is_whitespace() {
            let mut j = i + 1;
            while j < cs.len() && cs[j].1.is_whitespace() && !is_nl(j) {
                j += 1;
            }
            out.push(Tok::new(K::Space, &s[at(i)..at(j)]));
            i = j;
        } else if c.is_alphabetic() {
            let mut j = i + 1;
            loop {
                if j < cs.len() && cs[j].1.is_alphabetic() {
                    j += 1;
                } else if j + 1 < cs.len()
                    && matches!(cs[j].1, '\'' | '\u{2019}' | '-')
                    && cs[j + 1].1.is_alphabetic()
                {
                    j += 2;
                } else {
                    break;
                }
            }
            out.push(Tok::new(K::Word, &s[at(i)..at(j)]));
            i = j;
        } else {
            out.push(Tok::new(K::Punct, &s[at(i)..at(i + 1)]));
            i += 1;
        }
    }
}
