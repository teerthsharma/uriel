use std::collections::{BTreeMap, VecDeque};

use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};

pub const SCHEMA: &str = "uriel.fleet.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Start,
    End,
    Mark,
    Llm,
    Tool,
    Reap,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Start => "scope_start",
            Kind::End => "scope_end",
            Kind::Mark => "mark",
            Kind::Llm => "llm",
            Kind::Tool => "tool",
            Kind::Reap => "reap",
        }
    }

    pub fn parse(name: &str) -> Option<Kind> {
        [Kind::Start, Kind::End, Kind::Mark, Kind::Llm, Kind::Tool, Kind::Reap]
            .into_iter()
            .find(|k| k.name() == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub seq: u64,
    pub run: String,
    pub agent: String,
    pub parent: Option<String>,
    pub kind: Kind,
    pub name: String,
    pub status: Option<String>,
    pub role: Option<String>,
    pub tier: Option<String>,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cached_in: u64,
    pub t_ms: Option<u64>,
    pub attrs: BTreeMap<String, String>,
    pub prev: String,
    pub hash: String,
}

impl Event {
    pub fn new(run: &str, agent: &str, kind: Kind, name: &str) -> Event {
        Event {
            seq: 0,
            run: run.to_string(),
            agent: agent.to_string(),
            parent: None,
            kind,
            name: name.to_string(),
            status: None,
            role: None,
            tier: None,
            tokens_in: 0,
            tokens_out: 0,
            cached_in: 0,
            t_ms: None,
            attrs: BTreeMap::new(),
            prev: String::new(),
            hash: String::new(),
        }
    }

    fn body(&self) -> Json {
        json!({
            "schema": SCHEMA,
            "seq": self.seq,
            "run": self.run,
            "agent": self.agent,
            "parent": self.parent,
            "category": if self.kind == Kind::Llm { "llm" } else if self.kind == Kind::Tool { "tool" } else { "agent" },
            "kind": self.kind.name(),
            "name": self.name,
            "status": self.status,
            "role": self.role,
            "tier": self.tier,
            "tokens_in": self.tokens_in,
            "tokens_out": self.tokens_out,
            "cached_in": self.cached_in,
            "t_ms": self.t_ms,
            "attrs": self.attrs,
            "prev": self.prev
        })
    }

    fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.body().to_string().as_bytes());
        hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn to_json(&self) -> Json {
        let mut v = self.body();
        v["hash"] = Json::String(self.hash.clone());
        v
    }
}

#[derive(Debug, Clone)]
pub struct Trace {
    anchor: String,
    events: VecDeque<Event>,
    cap: usize,
    next_seq: u64,
    dropped: u64,
}

impl Trace {
    pub fn new(cap: usize) -> Trace {
        Trace {
            anchor: genesis(),
            events: VecDeque::new(),
            cap: cap.max(1),
            next_seq: 0,
            dropped: 0,
        }
    }

    pub fn push(&mut self, mut ev: Event) -> &Event {
        ev.seq = self.next_seq;
        self.next_seq += 1;
        ev.prev = self
            .events
            .back()
            .map_or_else(|| self.anchor.clone(), |last| last.hash.clone());
        ev.hash = ev.digest();
        self.events.push_back(ev);
        while self.events.len() > self.cap {
            if let Some(old) = self.events.pop_front() {
                self.anchor = old.hash;
                self.dropped += 1;
            }
        }
        self.events.back().expect("an event was just pushed")
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn events(&self) -> impl Iterator<Item = &Event> {
        self.events.iter()
    }

    pub fn head(&self) -> String {
        self.events
            .back()
            .map_or_else(|| self.anchor.clone(), |e| e.hash.clone())
    }

    pub fn verify(&self) -> Result<(), u64> {
        let mut prev = self.anchor.clone();
        for ev in &self.events {
            if ev.prev != prev || ev.digest() != ev.hash {
                return Err(ev.seq);
            }
            prev = ev.hash.clone();
        }
        Ok(())
    }

    pub fn jsonl(&self) -> String {
        let header = json!({
            "schema": SCHEMA,
            "type": "trace_header",
            "anchor": self.anchor,
            "dropped": self.dropped,
            "events": self.events.len(),
            "head": self.head()
        });
        let mut out = header.to_string();
        for ev in &self.events {
            out.push('\n');
            out.push_str(&ev.to_json().to_string());
        }
        out
    }
}

fn genesis() -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"uriel-fleet-genesis");
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(n: usize, cap: usize) -> Trace {
        let mut t = Trace::new(cap);
        for i in 0..n {
            t.push(Event::new("r", "a1", Kind::Mark, &format!("m{i}")));
        }
        t
    }

    #[test]
    fn an_untouched_chain_verifies() {
        assert_eq!(filled(10, 100).verify(), Ok(()));
    }

    #[test]
    fn editing_any_field_of_any_event_breaks_the_chain_at_that_event() {
        for target in 0..6usize {
            let mut t = filled(6, 100);
            t.events[target].name = "forged".to_string();
            assert_eq!(t.verify(), Err(target as u64), "edit at {target}");
        }
    }

    #[test]
    fn deleting_an_event_from_the_middle_breaks_the_chain() {
        let mut t = filled(6, 100);
        t.events.remove(3);
        assert_eq!(t.verify(), Err(4));
    }

    #[test]
    fn reordering_two_events_breaks_the_chain() {
        let mut t = filled(6, 100);
        t.events.swap(1, 2);
        assert!(t.verify().is_err());
    }

    #[test]
    fn eviction_keeps_the_chain_verifiable_from_the_anchor() {
        let t = filled(50, 8);
        assert_eq!(t.len(), 8);
        assert_eq!(t.dropped(), 42);
        assert_eq!(t.verify(), Ok(()));
        assert_eq!(t.events().next().map(|e| e.seq), Some(42));
    }

    #[test]
    fn a_rewritten_head_after_eviction_is_caught() {
        let mut t = filled(50, 8);
        t.events[0].prev = "0".repeat(64);
        assert!(t.verify().is_err());
    }

    #[test]
    fn the_same_events_always_hash_to_the_same_head() {
        assert_eq!(filled(20, 100).head(), filled(20, 100).head());
        assert_ne!(filled(20, 100).head(), filled(21, 100).head());
    }

    #[test]
    fn jsonl_has_a_header_then_one_line_per_event_and_every_line_parses() {
        let t = filled(5, 100);
        let text = t.jsonl();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 6);
        for l in &lines {
            assert!(serde_json::from_str::<Json>(l).is_ok());
        }
        assert!(lines[0].contains("\"trace_header\""));
        assert!(lines[1].contains("\"hash\""));
    }
}
