use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use uriel_terse::estimate_tokens;

use crate::config::Tools;
use crate::session::Seen;
use crate::tools::{object, opt_usize, req_str, str_prop, Context, ToolDef, ToolResult};

pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ROWS: usize = 200;
pub const DEFAULT_SOFT_TOKENS: u64 = 40_000;
pub const DEFAULT_HARD_TOKENS: u64 = 100_000;
pub const SPILL_BYTES: usize = 2 * 1024 * 1024;
pub const MIN_STUB_TOKENS: usize = 200;
pub const MIN_TIGHT_CHARS: usize = 2_000;
pub const MIN_CRITICAL_CHARS: usize = 800;

pub const MORE: &str = "more";
pub const USAGE: &str = "usage";
pub const ENABLE: &str = "tools_enable";

const REPEATABLE: &[&str] = &[
    "get_neo4j_schema",
    "read_neo4j_cypher",
    "read_graph",
    "search_memories",
    "find_memories_by_name",
    "tier_stats",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    Core,
    Write,
    Memory,
    Place,
    Rag,
    House,
    Research,
    Dispatch,
}

impl Group {
    pub const ENABLEABLE: &'static [Group] = &[
        Group::Write,
        Group::Memory,
        Group::Place,
        Group::Rag,
        Group::House,
        Group::Research,
        Group::Dispatch,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Group::Core => "core",
            Group::Write => "write",
            Group::Memory => "memory",
            Group::Place => "place",
            Group::Rag => "rag",
            Group::House => "house",
            Group::Research => "research",
            Group::Dispatch => "dispatch",
        }
    }

    pub fn parse(s: &str) -> Option<Group> {
        Group::ENABLEABLE.iter().copied().find(|g| g.name() == s)
    }
}

pub fn group_of(name: &str) -> Group {
    match name {
        "write_neo4j_cypher" => Group::Write,
        "create_entities"
        | "create_relations"
        | "add_observations"
        | "delete_entities"
        | "delete_observations"
        | "delete_relations"
        | "read_graph"
        | "search_memories"
        | "find_memories_by_name" => Group::Memory,
        "why_hot" | "pin" | "unpin" | "replay" | "calibrate" | "tier_stats" => Group::Place,
        "rag_feedback" | "soul" | "session_reset" => Group::Rag,
        "house_need" | "dispatch_open" => Group::Core,
        n if n.starts_with("house_") => Group::House,
        n if n.starts_with("research_") => Group::Research,
        n if n.starts_with("dispatch_") => Group::Dispatch,
        _ => Group::Core,
    }
}

fn opens(name: &str) -> &'static [Group] {
    match name {
        "house_need" => &[Group::House, Group::Research],
        "dispatch_open" => &[Group::Dispatch],
        _ => &[],
    }
}

fn closes(name: &str) -> &'static [Group] {
    match name {
        "dispatch_close" => &[Group::Dispatch],
        _ => &[],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pressure {
    Normal,
    Tight,
    Critical,
}

impl Pressure {
    pub fn name(self) -> &'static str {
        match self {
            Pressure::Normal => "normal",
            Pressure::Tight => "tight",
            Pressure::Critical => "critical",
        }
    }
}

pub fn cap_chars(limit: usize, pressure: Pressure) -> usize {
    match pressure {
        Pressure::Normal => limit,
        Pressure::Tight => (limit / 2).max(MIN_TIGHT_CHARS).min(limit),
        Pressure::Critical => (limit / 4).max(MIN_CRITICAL_CHARS).min(limit),
    }
}

pub fn cut_index(text: &str, max_chars: usize) -> usize {
    let Some((end, _)) = text.char_indices().nth(max_chars) else {
        return text.len();
    };
    match text[..end].rfind('\n') {
        Some(nl) if nl > 0 && text[..nl].chars().count() * 2 >= max_chars => nl + 1,
        _ => end,
    }
}

#[derive(Debug, Default)]
struct SpillInner {
    next: u64,
    items: VecDeque<(u64, String)>,
    bytes: usize,
}

#[derive(Debug)]
pub struct Spill {
    inner: Mutex<SpillInner>,
    cap_bytes: usize,
}

impl Spill {
    pub fn new(cap_bytes: usize) -> Spill {
        Spill {
            inner: Mutex::new(SpillInner::default()),
            cap_bytes: cap_bytes.max(1),
        }
    }

    fn lock(&self) -> MutexGuard<'_, SpillInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn put(&self, rest: String) -> Option<u64> {
        if rest.len() > self.cap_bytes {
            return None;
        }
        let mut s = self.lock();
        s.next += 1;
        let id = s.next;
        s.bytes += rest.len();
        s.items.push_back((id, rest));
        while s.bytes > self.cap_bytes {
            match s.items.pop_front() {
                Some((_, t)) => s.bytes -= t.len(),
                None => break,
            }
        }
        Some(id)
    }

    pub fn page(&self, id: u64, max_chars: usize) -> Option<(String, bool)> {
        let mut s = self.lock();
        let pos = s.items.iter().position(|(i, _)| *i == id)?;
        let text = std::mem::take(&mut s.items[pos].1);
        let at = cut_index(&text, max_chars.max(1));
        let (head, tail) = text.split_at(at);
        let head = head.to_string();
        let tail = tail.to_string();
        s.bytes -= text.len();
        if tail.is_empty() {
            s.items.remove(pos);
            return Some((head, false));
        }
        s.bytes += tail.len();
        s.items[pos].1 = tail;
        Some((head, true))
    }

    pub fn len(&self) -> usize {
        self.lock().items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn bytes(&self) -> usize {
        self.lock().bytes
    }

    pub fn clear(&self) {
        let mut s = self.lock();
        s.items.clear();
        s.bytes = 0;
    }
}

#[derive(Debug)]
pub struct Guard {
    pub tools: Tools,
    pub soft: u64,
    pub hard: u64,
    sent: AtomicU64,
    calls: AtomicU64,
    stubbed: AtomicU64,
    spilled: AtomicU64,
    told: AtomicU8,
    changed: AtomicBool,
    active: Mutex<BTreeSet<Group>>,
    pub spill: Spill,
}

impl Guard {
    pub fn new(tools: Tools, soft: u64, hard: u64) -> Guard {
        Guard {
            tools,
            soft,
            hard,
            sent: AtomicU64::new(0),
            calls: AtomicU64::new(0),
            stubbed: AtomicU64::new(0),
            spilled: AtomicU64::new(0),
            told: AtomicU8::new(0),
            changed: AtomicBool::new(false),
            active: Mutex::new(BTreeSet::new()),
            spill: Spill::new(SPILL_BYTES),
        }
    }

    pub fn adaptive(&self) -> bool {
        self.tools == Tools::Adaptive
    }

    pub fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    pub fn pressure(&self) -> Pressure {
        let sent = self.sent();
        if self.hard > 0 && sent >= self.hard {
            Pressure::Critical
        } else if self.soft > 0 && sent >= self.soft {
            Pressure::Tight
        } else {
            Pressure::Normal
        }
    }

    pub fn active(&self) -> Vec<Group> {
        self.active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .copied()
            .collect()
    }

    pub fn is_listed(&self, name: &str) -> bool {
        let g = group_of(name);
        !self.adaptive()
            || g == Group::Core
            || self
                .active
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(&g)
    }

    fn set(&self, groups: &[Group], on: bool) -> bool {
        let mut a = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        let mut changed = false;
        for g in groups {
            changed |= if on { a.insert(*g) } else { a.remove(g) };
        }
        if changed {
            self.changed.store(true, Ordering::Relaxed);
        }
        changed
    }

    pub fn take_list_changed(&self) -> bool {
        self.adaptive() && self.changed.swap(false, Ordering::Relaxed)
    }

    pub fn note_call(&self, name: &str, ok: bool) {
        if name == ENABLE {
            return;
        }
        if ok {
            self.set(opens(name), true);
            self.set(closes(name), false);
        }
        let g = group_of(name);
        if g != Group::Core && !closes(name).contains(&g) {
            self.set(&[g], true);
        }
    }

    fn told_level(&self, p: Pressure) -> bool {
        let level = match p {
            Pressure::Normal => 0,
            Pressure::Tight => 1,
            Pressure::Critical => 2,
        };
        self.told.fetch_max(level, Ordering::Relaxed) < level
    }
}

fn key(name: &str, args: &Json) -> u64 {
    let mut a = args.clone();
    if let Some(o) = a.as_object_mut() {
        o.remove("fresh");
    }
    let d = Sha256::digest(format!("{name}\u{0}{a}").as_bytes());
    u64::from_le_bytes(d[..8].try_into().unwrap_or([0; 8]))
}

fn fingerprint(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(12)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn fresh(args: &Json) -> bool {
    args.get("fresh").and_then(Json::as_bool).unwrap_or(false)
}

pub fn shape(
    ctx: &Context,
    name: &str,
    args: &Json,
    text: String,
    is_error: bool,
    limit: usize,
) -> String {
    let g = &ctx.guard;
    g.calls.fetch_add(1, Ordering::Relaxed);
    let mut text = text;
    if !is_error && name != MORE && REPEATABLE.contains(&name) && !fresh(args) {
        let tokens = estimate_tokens(&text);
        if tokens >= MIN_STUB_TOKENS {
            let id = key(name, args);
            let fp = fingerprint(&text);
            if ctx.session.check(id, &fp, 1) == Seen::Same {
                g.stubbed.fetch_add(1, Ordering::Relaxed);
                text = format!("=same ({tokens} tokens sent earlier; fresh=true resends)");
            } else {
                ctx.session.record(id, &fp, 1);
            }
        }
    }
    let pressure = g.pressure();
    let cap = cap_chars(limit, pressure);
    if name != MORE && text.chars().count() > cap {
        let at = cut_index(&text, cap);
        let rest = text.split_off(at);
        let total = estimate_tokens(&text) + estimate_tokens(&rest);
        let rest_tokens = estimate_tokens(&rest);
        match g.spill.put(rest) {
            Some(id) => {
                g.spilled.fetch_add(1, Ordering::Relaxed);
                text.push_str(&format!(
                    "\n... [truncated: ~{rest_tokens} of ~{total} tokens left; {MORE}(h={id}) continues, or refine the query]"
                ));
            }
            None => text.push_str(&format!(
                "\n... [truncated: ~{rest_tokens} of ~{total} tokens dropped as too large to keep; refine the query]"
            )),
        }
    }
    g.sent
        .fetch_add(estimate_tokens(&text) as u64, Ordering::Relaxed);
    let after = g.pressure();
    let machine = text.starts_with('{') || text.starts_with('[');
    if after != Pressure::Normal && !machine && g.told_level(after) {
        let cap = cap_chars(limit, after) / 4;
        text.push_str(&format!(
            "\n[budget: ~{} tokens sent this session; outputs now capped near {cap} tokens, {MORE} continues]",
            g.sent()
        ));
    }
    text
}

pub fn with_notes(base: &str, ctx: &Context) -> String {
    if !ctx.guard.adaptive() {
        return base.to_string();
    }
    let mut groups = String::new();
    for grp in Group::ENABLEABLE {
        let names: Vec<&str> = crate::tools::visible(ctx.read_only, ctx.mode, ctx.terse)
            .filter(|t| group_of(t.name) == *grp)
            .map(|t| t.name)
            .collect();
        if !names.is_empty() {
            groups.push_str(&format!(" {}: {}.", grp.name(), names.join(", ")));
        }
    }
    format!(
        "{base} Listed now: the core tools. Others are listed when first used or by {ENABLE}(group); they work before they are listed.{groups} {}",
        protocol_note()
    )
}

fn protocol_note() -> String {
    format!("A cut result ends with {MORE}(h=N), which continues it. A repeated large read returns `=same`; fresh=true resends. {USAGE} shows tokens sent.")
}

fn more_schema() -> Json {
    object(
        json!({
            "h": {"type": "integer", "description": "Handle from a truncated result."},
            "chars": {"type": "integer", "description": "Page size in characters (default: the current output cap)."}
        }),
        &["h"],
    )
}

fn usage_schema() -> Json {
    object(json!({}), &[])
}

fn enable_schema() -> Json {
    object(
        json!({
            "group": str_prop("memory, write, place, rag, house, research or dispatch."),
            "off": {"type": "boolean", "description": "Hide the group again."}
        }),
        &["group"],
    )
}

pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: MORE,
        title: "Continue a cut result",
        description: "Next page of a result that ended in a truncated notice; pass its h.",
        writes: false,
        schema: more_schema,
    },
    ToolDef {
        name: USAGE,
        title: "Session token use",
        description: "Tokens sent this session, output pressure, listed groups, repeats avoided.",
        writes: false,
        schema: usage_schema,
    },
    ToolDef {
        name: ENABLE,
        title: "List or hide a tool group",
        description: "List a group of tools: memory, write, place, rag, house, research, dispatch. Calling a tool of a group also lists it.",
        writes: false,
        schema: enable_schema,
    },
];

pub fn call(ctx: &Context, name: &str, args: &Json, limit: usize) -> ToolResult {
    let g = &ctx.guard;
    match name {
        MORE => {
            let h = opt_usize(args, "h", 0)?;
            let cap = cap_chars(limit, g.pressure());
            let chars = opt_usize(args, "chars", cap)?.clamp(1, cap.max(1));
            match g.spill.page(h as u64, chars) {
                None => Err(format!("no stored remainder for h={h}: it was read to the end, evicted, or never existed; rerun the query")),
                Some((mut page, more)) => {
                    if more {
                        page.push_str(&format!("\n... [{MORE}(h={h}) continues]"));
                    }
                    Ok(page)
                }
            }
        }
        USAGE => Ok(json!({
            "tokens_sent": g.sent(),
            "calls": g.calls.load(Ordering::Relaxed),
            "pressure": g.pressure().name(),
            "soft": g.soft,
            "hard": g.hard,
            "listed": g.active().iter().map(|x| x.name()).collect::<Vec<_>>(),
            "repeats_stubbed": g.stubbed.load(Ordering::Relaxed),
            "cut_results": g.spilled.load(Ordering::Relaxed),
            "kept_bytes": g.spill.bytes(),
        })
        .to_string()),
        ENABLE => {
            let group = req_str(args, "group")?;
            let grp = Group::parse(group).ok_or_else(|| {
                format!(
                    "unknown group '{group}': memory, write, place, rag, house, research, dispatch"
                )
            })?;
            let on = !args.get("off").and_then(Json::as_bool).unwrap_or(false);
            let listed: Vec<&str> = crate::tools::visible(ctx.read_only, ctx.mode, ctx.terse)
                .filter(|t| group_of(t.name) == grp)
                .map(|t| t.name)
                .collect();
            if listed.is_empty() {
                return Err(format!(
                    "group '{group}' has no tools in this server (mode {}, read-only {})",
                    ctx.mode.name(),
                    ctx.read_only
                ));
            }
            let changed = g.set(&[grp], on);
            Ok(
                json!({"group": group, "listed": on, "changed": changed, "tools": listed})
                    .to_string(),
            )
        }
        _ => Err(format!("unhandled tool '{name}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_follows_the_two_thresholds_and_zero_disables() {
        let g = Guard::new(Tools::Adaptive, 100, 300);
        assert_eq!(g.pressure(), Pressure::Normal);
        g.sent.store(100, Ordering::Relaxed);
        assert_eq!(g.pressure(), Pressure::Tight);
        g.sent.store(300, Ordering::Relaxed);
        assert_eq!(g.pressure(), Pressure::Critical);
        let off = Guard::new(Tools::Adaptive, 0, 0);
        off.sent.store(u64::MAX, Ordering::Relaxed);
        assert_eq!(off.pressure(), Pressure::Normal);
    }

    #[test]
    fn caps_shrink_with_pressure_and_never_exceed_the_limit() {
        for limit in [100, 5_000, 20_000, 400_000] {
            let n = cap_chars(limit, Pressure::Normal);
            let t = cap_chars(limit, Pressure::Tight);
            let c = cap_chars(limit, Pressure::Critical);
            assert_eq!(n, limit);
            assert!(c <= t && t <= n, "{limit}: {c} {t} {n}");
        }
        assert_eq!(cap_chars(20_000, Pressure::Tight), 10_000);
        assert_eq!(cap_chars(20_000, Pressure::Critical), 5_000);
        assert_eq!(cap_chars(100, Pressure::Critical), 100);
    }

    #[test]
    fn cuts_land_on_a_line_boundary_when_one_is_near_and_on_a_char_boundary_otherwise() {
        let text = "aaaa\nbbbb\ncccc\n";
        assert_eq!(&text[..cut_index(text, 11)], "aaaa\nbbbb\n");
        let wide = "\u{e9}".repeat(10);
        let at = cut_index(&wide, 4);
        assert_eq!(wide[..at].chars().count(), 4);
        let long_line = format!("x{}", "y".repeat(50));
        assert_eq!(cut_index(&long_line, 20), 20);
        assert_eq!(cut_index("short", 100), 5);
    }

    #[test]
    fn spill_pages_a_remainder_to_the_end_and_then_forgets_it() {
        let s = Spill::new(1 << 20);
        let id = s.put("l1\nl2\nl3\nl4\n".to_string()).unwrap();
        let (p1, more1) = s.page(id, 6).unwrap();
        assert_eq!(p1, "l1\nl2\n");
        assert!(more1);
        let (p2, more2) = s.page(id, 100).unwrap();
        assert_eq!(p2, "l3\nl4\n");
        assert!(!more2);
        assert!(s.page(id, 10).is_none());
        assert_eq!(s.bytes(), 0);
    }

    #[test]
    fn spill_evicts_oldest_past_its_byte_cap_and_refuses_one_that_can_never_fit() {
        let s = Spill::new(100);
        let a = s.put("a".repeat(60)).unwrap();
        let b = s.put("b".repeat(60)).unwrap();
        assert!(s.page(a, 5).is_none(), "oldest evicted");
        assert!(s.page(b, 5).is_some());
        assert!(s.put("c".repeat(101)).is_none());
        assert!(s.bytes() <= 100);
    }

    #[test]
    fn groups_open_on_entry_close_on_exit_and_member_use_lists_the_group() {
        let g = Guard::new(Tools::Adaptive, 0, 0);
        assert!(g.is_listed("rag_search"));
        assert!(!g.is_listed("house_next"));
        g.note_call("house_need", true);
        assert!(g.is_listed("house_next") && g.is_listed("research_record"));
        assert!(g.take_list_changed());
        assert!(!g.take_list_changed());
        g.note_call("dispatch_open", true);
        assert!(g.is_listed("dispatch_add"));
        g.note_call("dispatch_close", true);
        assert!(!g.is_listed("dispatch_add"));
        g.note_call("create_entities", true);
        assert!(g.is_listed("read_graph"));
        let full = Guard::new(Tools::Full, 0, 0);
        assert!(full.is_listed("house_next"));
        full.note_call("house_need", true);
        assert!(
            !full.take_list_changed(),
            "full mode never announces a change"
        );
    }

    #[test]
    fn a_failed_entry_call_does_not_open_its_group() {
        let g = Guard::new(Tools::Adaptive, 0, 0);
        g.note_call("dispatch_open", false);
        assert!(!g.is_listed("dispatch_add"));
    }

    #[test]
    fn every_enableable_group_name_round_trips() {
        for g in Group::ENABLEABLE {
            assert_eq!(Group::parse(g.name()), Some(*g));
        }
        assert_eq!(Group::parse("core"), None);
        assert_eq!(Group::parse("nope"), None);
    }
}
