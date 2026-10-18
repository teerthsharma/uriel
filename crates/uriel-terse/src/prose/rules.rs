use super::Level;

pub(super) const NEGATIONS: &[&str] = &[
    "not", "no", "never", "cannot", "nor", "neither", "none", "nothing", "nobody", "nowhere",
    "without",
];

pub(super) fn is_neg(word: &str) -> bool {
    NEGATIONS.contains(&word) || word.ends_with("n't")
}

pub(super) const ABBREVIATIONS: &[&str] = &[
    "vs", "cf", "dr", "mr", "mrs", "ms", "prof", "fig", "eq", "approx", "ca", "st", "jr", "no",
];

pub(super) const FILLERS: &[&str] = &[
    "really",
    "very",
    "quite",
    "actually",
    "basically",
    "essentially",
    "simply",
    "literally",
    "honestly",
    "frankly",
    "definitely",
    "certainly",
    "obviously",
    "totally",
    "just",
];

pub(super) const AFTER_A_STOP: &[&str] = &[
    "to", "is", "was", "and", "or", "in", "on", "at", "by", "of", "for", "with", "as", "if",
    "then", "the", "equals", "plus", "minus", "times", "are", "be", "from", "than",
];

pub(super) const BEFORE_COPULA_STOP: &[&str] = &[
    "there", "here", "what", "where", "how", "who", "whom", "which", "why", "when", "that", "this",
    "whatever", "whoever",
];

pub(super) const AFTER_COPULA_STOP: &[&str] = &["to", "being"];

pub(super) const HEDGE_PREFIXES: &[&str] = &[
    "i'm not entirely sure",
    "i'm not entirely certain",
    "i'm not completely sure",
    "i'm not completely certain",
    "i'm not totally sure",
    "i'm not totally certain",
    "i am not entirely sure",
    "i am not entirely certain",
    "i am not completely sure",
    "i am not completely certain",
    "i'm not sure",
    "i'm not certain",
    "i am not sure",
    "i am not certain",
    "i don't know",
    "i do not know",
    "i dunno",
    "not sure",
    "not certain",
];

pub(super) const LEADINS: &[&str] = &[
    "sure",
    "certainly",
    "of course",
    "absolutely",
    "definitely",
    "great",
    "okay",
    "alright",
    "good question",
    "great question",
    "excellent question",
    "good catch",
    "great catch",
    "thanks",
    "thank you",
    "sorry",
    "my apologies",
    "apologies",
    "basically",
    "actually",
    "essentially",
    "honestly",
    "frankly",
    "obviously",
    "clearly",
    "no problem",
    "no worries",
    "not a problem",
    "don't worry",
    "thanks for asking",
    "thank you for asking",
    "sorry about that",
    "sorry for the confusion",
    "sorry for the inconvenience",
];

pub(super) const CLOSERS: &[&str] = &[
    "i hope this helps",
    "i hope that helps",
    "i hope it helps",
    "hope this helps",
    "hope that helps",
    "hope it helps",
    "hope this clarifies things",
    "hope that clarifies things",
    "hope this clarifies",
    "let me know if you have any questions",
    "let me know if you have any other questions",
    "let me know if you need anything else",
    "let me know if you need anything",
    "let me know if you need more help",
    "let me know if that works",
    "let me know if that helps",
    "feel free to ask",
    "feel free to ask questions",
    "feel free to reach out",
    "happy to help",
    "i'm happy to help",
    "i am happy to help",
    "i'd be happy to help",
    "i would be happy to help",
    "glad i could help",
    "glad to help",
    "i apologize for the confusion",
    "i apologize for the inconvenience",
    "i apologize",
    "sorry for the confusion",
    "sorry for the inconvenience",
    "sorry about that",
    "my apologies",
    "is there anything else i can help with",
    "is there anything else",
    "thanks for asking",
    "thank you for asking",
    "thanks for your patience",
    "thank you for your patience",
    "great question",
    "good question",
    "excellent question",
    "good catch",
    "great catch",
    "no problem",
    "no worries",
    "don't hesitate to ask",
    "do not hesitate to ask",
];

pub(super) struct Rule {
    pub pat: &'static str,
    pub rep: &'static str,
}

const fn r(pat: &'static str, rep: &'static str) -> Rule {
    Rule { pat, rep }
}

pub(super) const LITE: &[Rule] = &[
    r("i think that", "likely"),
    r("i think", "likely"),
    r("i believe that", "likely"),
    r("i believe", "likely"),
    r("i guess", "likely"),
    r("i suppose", "likely"),
    r("i suspect", "likely"),
    r("i reckon", "likely"),
    r("i'm fairly sure", "likely"),
    r("i am fairly sure", "likely"),
    r("i'm pretty sure", "likely"),
    r("i am pretty sure", "likely"),
    r("i'm quite sure", "likely"),
    r("it seems that", "seems"),
    r("it seems like", "seems"),
    r("it seems", "seems"),
    r("it appears that", "seems"),
    r("it appears", "seems"),
    r("as far as i know", "afaik"),
    r("to the best of my knowledge", "afaik"),
    r("if i remember correctly", "iirc"),
    r("if i recall correctly", "iirc"),
    r("as i recall", "iirc"),
    r("in my opinion", "imo"),
    r("perhaps", "maybe"),
    r("possibly", "maybe"),
    r("probably", "likely"),
    r("please note that", ""),
    r("it is important to note that", ""),
    r("it is worth noting that", ""),
    r("it should be noted that", ""),
    r("it is worth mentioning that", ""),
    r("needless to say", ""),
    r("as you can see", ""),
    r("i would like to", ""),
    r("i'd like to", ""),
    r("i just wanted to", ""),
    r("as a matter of fact", ""),
    r("for what it's worth", ""),
    r("to be honest", ""),
    r("at the end of the day", ""),
    r("all things considered", ""),
    r("if you don't mind", ""),
    r("if i may", ""),
    r("when all is said and done", ""),
    r("it goes without saying that", ""),
];

pub(super) const FULL: &[Rule] = &[
    r("in order to", "to"),
    r("due to the fact that", "since"),
    r("owing to the fact that", "since"),
    r("in the event that", "if"),
    r("at this point in time", "now"),
    r("at the present time", "now"),
    r("a number of", "some"),
    r("a lot of", "many"),
    r("lots of", "many"),
    r("a large number of", "many"),
    r("is able to", "can"),
    r("are able to", "can"),
    r("was able to", "could"),
    r("were able to", "could"),
    r("has the ability to", "can"),
    r("have the ability to", "can"),
    r("as well as", "plus"),
    r("with regard to", "about"),
    r("with respect to", "about"),
    r("in regard to", "about"),
    r("for the purpose of", "for"),
    r("prior to", "before"),
    r("subsequent to", "after"),
    r("in spite of", "despite"),
    r("on the other hand", "but"),
    r("make use of", "use"),
    r("makes use of", "uses"),
    r("take into account", "consider"),
    r("in addition to", "besides"),
    r("in addition", "also"),
    r("as a result", "so"),
    r("for example", "e.g."),
    r("for instance", "e.g."),
    r("that is to say", "i.e."),
    r("in other words", "i.e."),
    r("such as", "like"),
    r("a majority of", "most"),
    r("the majority of", "most"),
    r("in terms of", "in"),
    r("with the exception of", "except"),
    r("whether or not", "whether"),
    r("each and every", "each"),
    r("first and foremost", "first"),
    r("utilize", "use"),
    r("utilizes", "uses"),
    r("utilized", "used"),
    r("utilizing", "using"),
    r("utilise", "use"),
    r("utilises", "uses"),
    r("utilised", "used"),
    r("utilization", "use"),
    r("identify", "find"),
    r("identifies", "finds"),
    r("identified", "found"),
    r("identifying", "finding"),
    r("approximately", "about"),
    r("demonstrate", "show"),
    r("demonstrates", "shows"),
    r("demonstrated", "showed"),
    r("subsequently", "then"),
    r("additionally", "also"),
    r("furthermore", "also"),
    r("moreover", "also"),
    r("consequently", "so"),
    r("therefore", "so"),
    r("thus", "so"),
    r("hence", "so"),
    r("however", "but"),
    r("nevertheless", "but"),
    r("nonetheless", "but"),
    r("regarding", "about"),
    r("concerning", "about"),
    r("require", "need"),
    r("requires", "needs"),
    r("required", "needed"),
    r("assist", "help"),
    r("facilitate", "help"),
    r("facilitates", "helps"),
    r("commence", "start"),
    r("obtain", "get"),
    r("sufficient", "enough"),
    r("numerous", "many"),
    r("attempt", "try"),
];

pub(super) const FULL_MORE: &[Rule] = &[
    r("assists", "helps"),
    r("assisted", "helped"),
    r("commences", "starts"),
    r("commenced", "started"),
    r("obtains", "gets"),
    r("obtained", "got"),
    r("purchase", "buy"),
    r("purchased", "bought"),
    r("attempts", "tries"),
    r("attempted", "tried"),
    r("modifies", "changes"),
    r("modified", "changed"),
    r("modification", "change"),
    r("indicate", "show"),
    r("indicates", "shows"),
    r("indicated", "showed"),
    r("construct", "build"),
    r("constructs", "builds"),
    r("constructed", "built"),
    r("individuals", "people"),
    r("additional", "more"),
    r("adequate", "enough"),
    r("currently", "now"),
    r("previously", "before"),
    r("frequently", "often"),
    r("occasionally", "sometimes"),
    r("initially", "first"),
    r("locate", "find"),
    r("locates", "finds"),
];

pub(super) const ULTRA: &[Rule] = &[
    r("configuration", "config"),
    r("configurations", "configs"),
    r("function", "fn"),
    r("functions", "fns"),
    r("implementation", "impl"),
    r("implementations", "impls"),
    r("repository", "repo"),
    r("repositories", "repos"),
    r("parameter", "param"),
    r("parameters", "params"),
    r("request", "req"),
    r("requests", "reqs"),
    r("response", "resp"),
    r("responses", "resps"),
    r("environment", "env"),
    r("environments", "envs"),
    r("application", "app"),
    r("applications", "apps"),
    r("directory", "dir"),
    r("directories", "dirs"),
    r("documentation", "docs"),
    r("information", "info"),
    r("dependency", "dep"),
    r("dependencies", "deps"),
    r("authentication", "auth"),
    r("development", "dev"),
    r("production", "prod"),
    r("database", "DB"),
    r("databases", "DBs"),
    r("specification", "spec"),
    r("specifications", "specs"),
    r("variable", "var"),
    r("variables", "vars"),
    r("argument", "arg"),
    r("arguments", "args"),
    r("command", "cmd"),
    r("commands", "cmds"),
    r("message", "msg"),
    r("messages", "msgs"),
    r("memory", "mem"),
    r("performance", "perf"),
    r("initialize", "init"),
    r("initialization", "init"),
    r("synchronize", "sync"),
    r("synchronization", "sync"),
    r("management", "mgmt"),
    r("library", "lib"),
    r("libraries", "libs"),
    r("package", "pkg"),
    r("packages", "pkgs"),
    r("reference", "ref"),
    r("references", "refs"),
    r("temporary", "temp"),
    r("maximum", "max"),
    r("minimum", "min"),
    r("average", "avg"),
    r("administrator", "admin"),
    r("and", "&"),
    r("with", "w/"),
    r("because", "bc"),
    r("leads to", "->"),
    r("results in", "->"),
    r("which means", "=>"),
];

pub(super) struct Index {
    rows: Vec<(&'static str, &'static Rule)>,
}

impl Index {
    pub fn new(level: Level) -> Index {
        let mut rows: Vec<(&'static str, &'static Rule)> = Vec::new();
        let mut add = |rules: &'static [Rule]| {
            for rule in rules {
                let first = rule.pat.split(' ').next().unwrap_or("");
                rows.push((first, rule));
            }
        };
        add(LITE);
        if level >= Level::Full {
            add(FULL);
            add(FULL_MORE);
        }
        if level >= Level::Ultra {
            add(ULTRA);
        }
        rows.sort_by(|a, b| a.0.cmp(b.0));
        Index { rows }
    }

    pub fn candidates(&self, first: &str) -> impl Iterator<Item = &'static Rule> + '_ {
        let lo = self.rows.partition_point(|r| r.0 < first);
        let hi = self.rows.partition_point(|r| r.0 <= first);
        self.rows[lo..hi].iter().map(|r| r.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<&'static Rule> {
        LITE.iter()
            .chain(FULL.iter())
            .chain(FULL_MORE.iter())
            .chain(ULTRA.iter())
            .collect()
    }

    #[test]
    fn replacements_are_strictly_shorter() {
        for rule in all() {
            assert!(
                rule.rep.chars().count() < rule.pat.chars().count(),
                "{} -> {}",
                rule.pat,
                rule.rep
            );
        }
    }

    #[test]
    fn patterns_are_lowercase_ascii_and_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for rule in all() {
            assert!(
                rule.pat
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == ' ' || c == '\''),
                "{}",
                rule.pat
            );
            assert!(seen.insert(rule.pat), "duplicate {}", rule.pat);
        }
    }

    #[test]
    fn replacements_never_feed_other_patterns() {
        let pats: std::collections::BTreeSet<&str> = all().iter().map(|r| r.pat).collect();
        for rule in all() {
            assert!(!pats.contains(rule.rep), "{} -> {}", rule.pat, rule.rep);
        }
    }

    #[test]
    fn replacements_carry_no_negation_but_patterns_may() {
        for rule in all() {
            assert!(!rule.rep.split(' ').any(is_neg), "{}", rule.pat);
        }
    }

    #[test]
    fn closers_and_leadins_are_lowercase() {
        for p in CLOSERS
            .iter()
            .chain(LEADINS.iter())
            .chain(HEDGE_PREFIXES.iter())
        {
            assert!(
                p.chars()
                    .all(|c| c.is_ascii_lowercase() || c == ' ' || c == '\''),
                "{p}"
            );
        }
    }

    #[test]
    fn index_finds_candidates_by_first_word() {
        let idx = Index::new(Level::Ultra);
        let firsts: Vec<&str> = idx.candidates("in").map(|r| r.pat).collect();
        assert!(firsts.contains(&"in order to"));
        assert!(firsts.contains(&"in other words"));
        assert!(idx.candidates("zzz").next().is_none());
        assert!(Index::new(Level::Lite)
            .candidates("utilize")
            .next()
            .is_none());
    }
}
