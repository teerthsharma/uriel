const STOPWORDS: &[&str] = &[
    "a", "an", "the", "of", "to", "in", "is", "it", "and", "or", "for", "on", "as", "by", "be",
    "are", "was", "this", "that", "with", "from", "at", "if", "not", "no", "so", "do", "does",
    "its", "into", "than", "then", "when", "which", "who", "will", "can", "has", "have",
];

pub(crate) fn raw_words(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        let word_char = c.is_alphanumeric() || c == '_';
        match (word_char, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                out.push(&text[s..i]);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push(&text[s..]);
    }
    out
}

fn split_identifier(word: &str) -> Vec<String> {
    let mut parts = Vec::new();
    for piece in word.split('_').filter(|p| !p.is_empty()) {
        let chars: Vec<char> = piece.chars().collect();
        let mut cur = String::new();
        for (i, &c) in chars.iter().enumerate() {
            if i > 0 && c.is_uppercase() {
                let prev = chars[i - 1];
                let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
                if prev.is_lowercase()
                    || prev.is_ascii_digit()
                    || (prev.is_uppercase() && next_lower)
                {
                    parts.push(std::mem::take(&mut cur));
                }
            }
            cur.extend(c.to_lowercase());
        }
        parts.push(cur);
    }
    parts.retain(|p| !p.is_empty());
    parts
}

fn stem(word: String) -> String {
    let n = word.chars().count();
    if !word.is_ascii() || n < 5 {
        return word;
    }
    if let Some(s) = word.strip_suffix("ing") {
        if n > 6 {
            return s.to_string();
        }
    }
    if let Some(s) = word.strip_suffix("ed") {
        if n > 5 {
            return s.to_string();
        }
    }
    if let Some(s) = word.strip_suffix("es") {
        if n > 5 {
            return s.to_string();
        }
    }
    if word.ends_with('s') && !word.ends_with("ss") {
        return word[..word.len() - 1].to_string();
    }
    word
}

pub(crate) fn terms(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in raw_words(text) {
        let parts = split_identifier(word);
        let multi = parts.len() > 1;
        for p in parts.iter() {
            if p.chars().count() >= 2 && !STOPWORDS.contains(&p.as_str()) {
                out.push(stem(p.clone()));
            }
        }
        if multi {
            let whole = word.to_lowercase();
            if whole.chars().count() >= 2 {
                out.push(whole);
            }
        }
    }
    out
}

pub(crate) fn token_estimate(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_snake_and_camel_and_keeps_whole() {
        let t = terms("parse_duration HttpServer");
        assert!(t.contains(&"parse".to_string()));
        assert!(t.contains(&"duration".to_string()));
        assert!(t.contains(&"parse_duration".to_string()));
        assert!(t.contains(&"http".to_string()));
        assert!(t.contains(&"server".to_string()));
        assert!(t.contains(&"httpserver".to_string()));
    }

    #[test]
    fn stopwords_and_stemming() {
        assert_eq!(
            terms("the buffers of the dog"),
            vec!["buffer".to_string(), "dog".to_string()]
        );
        assert_eq!(stem("running".into()), "runn");
        assert_eq!(stem("class".into()), "class");
    }

    #[test]
    fn token_estimate_rounds_up() {
        assert_eq!(token_estimate(""), 0);
        assert_eq!(token_estimate("abcd"), 1);
        assert_eq!(token_estimate("abcde"), 2);
    }
}
