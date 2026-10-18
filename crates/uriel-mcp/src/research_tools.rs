use std::collections::BTreeMap;

use serde_json::{json, Map, Value as Json};
use sha2::{Digest, Sha256};
use uriel_algo::caustic::{audit_orbits, audit_pooling, Audit, Claims, Pooling};
use uriel_core::Value;

use crate::tools::{object, read_rows, req_str, str_prop, write_query, Context, ToolDef, ToolResult};

pub const MAX_ENTITIES: usize = 10_000;
pub const MAX_ADMISSIBLE: usize = 20_000;
pub const MAX_TEXT: usize = 512;
const MAX_WITNESSES: usize = 20;
const MAX_MEMBERS: usize = 20;
const MAX_WHY: usize = 2_000;
const CLEAN: &str = "no violation found; this is not confirmation";
const AUDITS: [&str; 2] = ["research_orbit_audit", "research_pooling_audit"];

fn relation_prop() -> Json {
    json!({
        "type": "array",
        "maxItems": MAX_ENTITIES,
        "items": {
            "type": "object",
            "properties": {
                "entity": str_prop("The entity the claim is about."),
                "answer": str_prop("The answer claimed for that entity.")
            },
            "required": ["entity", "answer"],
            "additionalProperties": false
        },
        "description": "The relation as claims entity -> answer, at most 10000. An entity given two different answers is skipped and counted."
    })
}

fn edge_prop() -> Json {
    str_prop("Read the relation from graph edges instead: every node with an outgoing edge of this type is an entity and the node it points to is its answer. A node with several different targets is skipped and counted. At most 10000 edges.")
}

fn orbit_schema() -> Json {
    object(
        json!({
            "relation": relation_prop(),
            "edge_type": edge_prop(),
            "injective": {"type": "boolean", "description": "The caller's premise that distinct entities have distinct correct answers. Every bound here depends on it; false means nothing is claimed."},
            "admissible": {"type": "array", "maxItems": MAX_ADMISSIBLE, "items": {"type": "string"}, "description": "The set of correct answers, without the pairing. Needs at least as many distinct values as entities, or the starred bounds are void."},
            "claimed_errors": {"type": "integer", "minimum": 0, "description": "The most wrong answers the claimant admits to. Default 0 when no claim at all is given: every answer is correct."},
            "claimed_precision": {"type": "number", "minimum": 0, "maximum": 1, "description": "The claimed fraction of entities sharing an answer that are wrong."},
            "claimed_recall": {"type": "number", "minimum": 0, "maximum": 1, "description": "The claimed fraction of all wrong answers that sit in the flagged set. Needs `admissible`."}
        }),
        &["injective"],
    )
}

fn pooling_schema() -> Json {
    object(
        json!({
            "relation": relation_prop(),
            "edge_type": edge_prop(),
            "recoveries": {
                "type": "array",
                "maxItems": MAX_ENTITIES,
                "items": {
                    "type": "object",
                    "properties": {
                        "entity": str_prop("An entity of the relation."),
                        "recovery": {"type": "number", "minimum": 0, "maximum": 1, "description": "The claimed probability that the entity can be recovered from its answer."}
                    },
                    "required": ["entity", "recovery"],
                    "additionalProperties": false
                },
                "description": "Recovery claims to test against the pooling ceiling 1/k of the entity's block."
            }
        }),
        &["recoveries"],
    )
}

fn record_schema() -> Json {
    object(
        json!({
            "tool": {"type": "string", "enum": AUDITS, "description": "The audit to run."},
            "arguments": {"type": "object", "description": "The arguments of that audit.", "additionalProperties": true},
            "goal": str_prop("Id of the Goal node the AUDITED edge starts from.")
        }),
        &["tool", "arguments", "goal"],
    )
}

pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "research_orbit_audit",
        title: "Caustic orbit audit",
        description: "Inspector skill (house mode). Partitions a relation entity -> answer into orbits and applies Caustic's deterministic, one-sided floors T1, T1*, T6, T6*, T8 to the claims made about it. It can prove a claim impossible; it can never confirm one. Returns {violated, bound, witnesses, note}; a clean result means only that no violation was found. Reads only.",
        writes: false,
        schema: orbit_schema,
    },
    ToolDef {
        name: "research_pooling_audit",
        title: "Caustic pooling audit",
        description: "Inspector skill (house mode). Tests recovery claims against Caustic's T2 ceiling: an entity whose answer is shared by k entities cannot be recovered with probability above 1/k. The test is one-sided: it can prove a claim impossible; it can never confirm one. Returns {violated, bound, witnesses, note}. Reads only.",
        writes: false,
        schema: pooling_schema,
    },
    ToolDef {
        name: "research_record",
        title: "Record an audit on a goal",
        description: "Runs one research audit and writes an AUDITED edge from a Goal node to an Audit node, with the verdict (violation or no_violation_found) and why (the tool name and its input). The audits are one-sided and never confirm a claim, so no confirming verdict exists. Idempotent per input.",
        writes: true,
        schema: record_schema,
    },
];

struct Relation {
    entities: Vec<String>,
    answers: Vec<String>,
    skipped: usize,
    skipped_key: &'static str,
}

impl Relation {
    fn answers(&self) -> Vec<&str> {
        self.answers.iter().map(String::as_str).collect()
    }

    fn names(&self, idx: &[usize]) -> (Vec<Json>, bool) {
        let shown = idx
            .iter()
            .take(MAX_MEMBERS)
            .map(|&i| json!(self.entities[i]))
            .collect();
        (shown, idx.len() > MAX_MEMBERS)
    }

    fn skipped_json(&self) -> Json {
        let mut m = Map::new();
        m.insert(self.skipped_key.to_string(), json!(self.skipped));
        Json::Object(m)
    }
}

fn text(v: Option<&Json>, what: &str, allow_empty: bool) -> Result<String, String> {
    let s = v
        .and_then(Json::as_str)
        .ok_or_else(|| format!("{what} must be a string"))?;
    if s.len() > MAX_TEXT {
        return Err(format!("{what} exceeds {MAX_TEXT} bytes"));
    }
    if s.is_empty() && !allow_empty {
        return Err(format!("{what} must not be empty"));
    }
    Ok(s.to_string())
}

fn reject_unknown(obj: &Map<String, Json>, allowed: &[&str], what: &str) -> Result<(), String> {
    match obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!("{what} has an unknown property '{k}'")),
        None => Ok(()),
    }
}

fn parse_pairs(v: &Json) -> Result<Vec<(String, String)>, String> {
    let items = v
        .as_array()
        .ok_or("'relation' must be an array of {entity, answer}")?;
    if items.len() > MAX_ENTITIES {
        return Err(format!(
            "'relation' holds {} entries; the limit is {MAX_ENTITIES}",
            items.len()
        ));
    }
    items
        .iter()
        .map(|item| {
            let obj = item
                .as_object()
                .ok_or("each 'relation' entry must be an object")?;
            reject_unknown(obj, &["entity", "answer"], "a 'relation' entry")?;
            Ok((
                text(obj.get("entity"), "'entity'", false)?,
                text(obj.get("answer"), "'answer'", true)?,
            ))
        })
        .collect()
}

fn build(pairs: Vec<(String, String)>, skipped_key: &'static str) -> Result<Relation, String> {
    let mut order: Vec<String> = Vec::new();
    let mut seen: BTreeMap<String, Option<String>> = BTreeMap::new();
    for (entity, answer) in pairs {
        match seen.get_mut(&entity) {
            None => {
                order.push(entity.clone());
                seen.insert(entity, Some(answer));
            }
            Some(slot) => {
                if slot.as_deref() != Some(answer.as_str()) {
                    *slot = None;
                }
            }
        }
    }
    let mut rel = Relation {
        entities: Vec::new(),
        answers: Vec::new(),
        skipped: 0,
        skipped_key,
    };
    for entity in order {
        match seen.remove(&entity).flatten() {
            Some(answer) => {
                rel.entities.push(entity);
                rel.answers.push(answer);
            }
            None => rel.skipped += 1,
        }
    }
    if rel.entities.is_empty() {
        return Err("the relation has no usable entity".into());
    }
    Ok(rel)
}

fn edge_type(raw: &str) -> Result<&str, String> {
    let ok = !raw.is_empty()
        && raw.len() <= 128
        && raw.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok {
        Ok(raw)
    } else {
        Err("'edge_type' must be 1 to 128 letters, digits or underscores".into())
    }
}

fn node_label(id: &Json, name: &Json) -> Result<String, String> {
    let id = id.as_i64().ok_or("unexpected node id encoding")?;
    Ok(match name.as_str().filter(|s| !s.is_empty()) {
        Some(n) => format!("#{id} {}", n.chars().take(64).collect::<String>()),
        None => format!("#{id}"),
    })
}

fn read_edges(ctx: &Context, kind: &str) -> Result<Relation, String> {
    let kind = edge_type(kind)?;
    let query = format!(
        "MATCH (a)-[r:`{kind}`]->(b) RETURN id(a) AS a, a.name AS an, id(b) AS b, b.name AS bn ORDER BY a, b LIMIT {}",
        MAX_ENTITIES + 1
    );
    let rows = read_rows(ctx, &query, &BTreeMap::new())?;
    if rows.len() > MAX_ENTITIES {
        return Err(format!(
            "'{kind}' holds more than {MAX_ENTITIES} edges; the limit is {MAX_ENTITIES}"
        ));
    }
    let pairs = rows
        .iter()
        .map(|row| -> Result<(String, String), String> {
            Ok((node_label(&row["a"], &row["an"])?, node_label(&row["b"], &row["bn"])?))
        })
        .collect::<Result<Vec<_>, String>>()?;
    build(pairs, "multi_valued")
}

fn load_relation(ctx: &Context, args: &Json) -> Result<Relation, String> {
    let given = |k: &str| args.get(k).filter(|v| !v.is_null());
    match (given("relation"), given("edge_type")) {
        (Some(_), Some(_)) => Err("give either 'relation' or 'edge_type', not both".into()),
        (None, None) => Err("one of 'relation' or 'edge_type' is required".into()),
        (Some(rel), None) => build(parse_pairs(rel)?, "conflicting"),
        (None, Some(kind)) => read_edges(
            ctx,
            kind.as_str().ok_or("'edge_type' must be a string")?,
        ),
    }
}

fn check_args(args: &Json, allowed: &[&str]) -> Result<(), String> {
    match args {
        Json::Object(m) => reject_unknown(m, allowed, "the arguments"),
        Json::Null => Ok(()),
        _ => Err("arguments must be an object".into()),
    }
}

fn opt_unit(args: &Json, key: &str) -> Result<Option<f64>, String> {
    match args.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite() && (0.0..=1.0).contains(x))
            .map(Some)
            .ok_or_else(|| format!("'{key}' must be a number in [0, 1]")),
    }
}

fn opt_count(args: &Json, key: &str) -> Result<Option<usize>, String> {
    match args.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("'{key}' must be a non-negative integer")),
    }
}

fn admissible_set(args: &Json) -> Result<Option<Vec<String>>, String> {
    match args.get("admissible") {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Array(items)) => {
            if items.len() > MAX_ADMISSIBLE {
                return Err(format!(
                    "'admissible' holds {} values; the limit is {MAX_ADMISSIBLE}",
                    items.len()
                ));
            }
            items
                .iter()
                .map(|v| text(Some(v), "an 'admissible' value", true))
                .collect::<Result<Vec<_>, _>>()
                .map(Some)
        }
        Some(_) => Err("'admissible' must be an array of strings".into()),
    }
}

fn rate_json(v: Option<f64>) -> Json {
    v.map_or(Json::Null, |x| json!(x))
}

fn orbit_audit(ctx: &Context, args: &Json) -> Result<Json, String> {
    check_args(
        args,
        &[
            "relation",
            "edge_type",
            "injective",
            "admissible",
            "claimed_errors",
            "claimed_precision",
            "claimed_recall",
        ],
    )?;
    let injective = args
        .get("injective")
        .and_then(Json::as_bool)
        .ok_or("'injective' is required and must be a boolean")?;
    let rel = load_relation(ctx, args)?;
    let gold = admissible_set(args)?;
    let mut claims = Claims {
        errors: opt_count(args, "claimed_errors")?,
        precision: opt_unit(args, "claimed_precision")?,
        recall: opt_unit(args, "claimed_recall")?,
    };
    if claims.errors.is_none() && claims.precision.is_none() && claims.recall.is_none() {
        claims.errors = Some(0);
    }
    let claims_json = json!({
        "errors": claims.errors,
        "precision": rate_json(claims.precision),
        "recall": rate_json(claims.recall)
    });
    if !injective {
        return Ok(json!({
            "tool": "research_orbit_audit",
            "violated": false,
            "bound": "none",
            "n": rel.entities.len(),
            "orbits": Json::Null,
            "admissible_orbits": Json::Null,
            "claims": claims_json,
            "checks": [],
            "inapplicable": [{"bound": "all", "reason": "the caller did not assert that the relation is injective; T1, T1*, T6, T6* and T8 hold only for an injective relation"}],
            "witnesses": [],
            "witnesses_truncated": false,
            "skipped": rel.skipped_json(),
            "note": CLEAN
        }));
    }
    let answers = rel.answers();
    let gold_refs: Option<Vec<&str>> = gold.as_ref().map(|g| g.iter().map(String::as_str).collect());
    let audit = audit_orbits(&answers, gold_refs.as_deref(), &claims).map_err(|e| e.to_string())?;
    let (witnesses, truncated) = if audit.violated() {
        orbit_witnesses(&rel, &audit)
    } else {
        (Vec::new(), false)
    };
    let note = if audit.violated() {
        let floor = audit
            .checks
            .iter()
            .find(|c| c.violated && c.bound == audit.bound())
            .map_or(0.0, |c| c.floor);
        format!(
            "violation found: {} puts the floor at {floor}, above the claim. The claim is impossible if the relation is injective as asserted. This is evidence about the claim, not a verdict on the data.",
            audit.bound()
        )
    } else {
        CLEAN.to_string()
    };
    Ok(json!({
        "tool": "research_orbit_audit",
        "violated": audit.violated(),
        "bound": audit.bound(),
        "n": audit.n,
        "orbits": audit.m,
        "admissible_orbits": audit.m_star,
        "claims": claims_json,
        "checks": audit.checks.iter().map(|c| json!({
            "bound": c.bound,
            "floor": c.floor,
            "claimed": c.claimed,
            "violated": c.violated
        })).collect::<Vec<_>>(),
        "inapplicable": audit.inapplicable.iter().map(|(b, why)| json!({"bound": b, "reason": why})).collect::<Vec<_>>(),
        "witnesses": witnesses,
        "witnesses_truncated": truncated,
        "skipped": rel.skipped_json(),
        "note": note
    }))
}

fn orbit_witnesses(rel: &Relation, audit: &Audit) -> (Vec<Json>, bool) {
    let part = uriel_algo::caustic::partition(&rel.answers());
    let mut out: Vec<Json> = Vec::new();
    for &o in &audit.collisions {
        let members = &part.orbits[o];
        let (entities, cut) = rel.names(members);
        out.push(json!({
            "kind": "collision",
            "answer": rel.answers[members[0]],
            "size": members.len(),
            "entities": entities,
            "entities_truncated": cut
        }));
    }
    let mut groups: Vec<(&str, Vec<usize>)> = Vec::new();
    for &i in &audit.inadmissible {
        let answer = rel.answers[i].as_str();
        match groups.iter_mut().find(|(a, _)| *a == answer) {
            Some((_, idx)) => idx.push(i),
            None => groups.push((answer, vec![i])),
        }
    }
    for (answer, idx) in groups {
        let (entities, cut) = rel.names(&idx);
        out.push(json!({
            "kind": "inadmissible",
            "answer": answer,
            "size": idx.len(),
            "entities": entities,
            "entities_truncated": cut
        }));
    }
    let truncated = out.len() > MAX_WITNESSES;
    out.truncate(MAX_WITNESSES);
    (out, truncated)
}

fn recoveries(args: &Json, rel: &Relation) -> Result<Vec<(usize, f64)>, String> {
    let items = args
        .get("recoveries")
        .and_then(Json::as_array)
        .filter(|a| !a.is_empty())
        .ok_or("'recoveries' is required and must be a non-empty array")?;
    if items.len() > MAX_ENTITIES {
        return Err(format!(
            "'recoveries' holds {} entries; the limit is {MAX_ENTITIES}",
            items.len()
        ));
    }
    let index: BTreeMap<&str, usize> = rel
        .entities
        .iter()
        .enumerate()
        .map(|(i, e)| (e.as_str(), i))
        .collect();
    items
        .iter()
        .map(|item| {
            let obj = item
                .as_object()
                .ok_or("each 'recoveries' entry must be an object")?;
            reject_unknown(obj, &["entity", "recovery"], "a 'recoveries' entry")?;
            let name = text(obj.get("entity"), "'entity'", false)?;
            let at = *index
                .get(name.as_str())
                .ok_or_else(|| format!("entity '{name}' is not in the relation or was skipped"))?;
            let claimed = obj
                .get("recovery")
                .and_then(Json::as_f64)
                .filter(|x| x.is_finite() && (0.0..=1.0).contains(x))
                .ok_or("'recovery' must be a number in [0, 1]")?;
            Ok((at, claimed))
        })
        .collect()
}

fn pooling_audit(ctx: &Context, args: &Json) -> Result<Json, String> {
    check_args(args, &["relation", "edge_type", "recoveries"])?;
    let rel = load_relation(ctx, args)?;
    let claims = recoveries(args, &rel)?;
    let hits = audit_pooling(&rel.answers(), &claims).map_err(|e| e.to_string())?;
    let violated: Vec<&Pooling> = hits.iter().filter(|h| h.violated).collect();
    let witnesses: Vec<Json> = violated
        .iter()
        .take(MAX_WITNESSES)
        .map(|h| {
            let (entities, cut) = rel.names(&h.members);
            json!({
                "entity": rel.entities[h.entity],
                "claimed_recovery": h.claimed,
                "ceiling": h.ceiling,
                "block_size": h.k,
                "answer": rel.answers[h.entity],
                "entities": entities,
                "entities_truncated": cut
            })
        })
        .collect();
    let note = if violated.is_empty() {
        CLEAN.to_string()
    } else {
        "violation found: T2 caps recovery of an entity at 1/k when k entities share its answer, and a claim above that is impossible. This is evidence about the claim, not a verdict on the data.".to_string()
    };
    Ok(json!({
        "tool": "research_pooling_audit",
        "violated": !violated.is_empty(),
        "bound": "T2",
        "n": rel.entities.len(),
        "orbits": uriel_algo::caustic::partition(&rel.answers()).m(),
        "checks": hits.iter().map(|h| json!({
            "entity": rel.entities[h.entity],
            "claimed": h.claimed,
            "ceiling": h.ceiling,
            "block_size": h.k,
            "violated": h.violated
        })).collect::<Vec<_>>(),
        "witnesses": witnesses,
        "witnesses_truncated": violated.len() > MAX_WITNESSES,
        "skipped": rel.skipped_json(),
        "note": note
    }))
}

fn record(ctx: &Context, args: &Json) -> Result<Json, String> {
    check_args(args, &["tool", "arguments", "goal"])?;
    let tool = req_str(args, "tool")?;
    if !AUDITS.contains(&tool) {
        return Err(format!("'tool' must be one of {}", AUDITS.join(", ")));
    }
    let inner = match args.get("arguments") {
        Some(v @ Json::Object(_)) => v,
        _ => return Err("'arguments' is required and must be an object".into()),
    };
    let goal = req_str(args, "goal")?;
    let mut result = if tool == "research_orbit_audit" {
        orbit_audit(ctx, inner)?
    } else {
        pooling_audit(ctx, inner)?
    };
    let found = read_rows(
        ctx,
        "MATCH (g:Goal {gid: $gid}) RETURN g.gid AS gid",
        &BTreeMap::from([("gid".to_string(), Value::String(goal.to_string()))]),
    )?;
    if found.is_empty() {
        return Err(format!("goal '{goal}' not found"));
    }
    let verdict = if result["violated"] == true {
        "violation"
    } else {
        "no_violation_found"
    };
    let input = serde_json::to_string(inner).map_err(|e| e.to_string())?;
    let sha: String = Sha256::digest(format!("{tool}\n{input}").as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut why = format!("{tool} {input}");
    if why.chars().count() > MAX_WHY {
        why = why.chars().take(MAX_WHY).collect::<String>() + "...[truncated]";
    }
    let key = format!("{tool}:{sha}");
    let params: BTreeMap<String, Value> = [
        ("gid", goal.to_string()),
        ("key", key.clone()),
        ("tool", tool.to_string()),
        ("sha", sha),
        ("verdict", verdict.to_string()),
        ("why", why),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), Value::String(v)))
    .collect();
    write_query(
        ctx,
        "MATCH (g:Goal {gid: $gid}) MERGE (a:Audit {key: $key}) SET a.tool = $tool, a.input_sha256 = $sha, a.verdict = $verdict MERGE (g)-[r:AUDITED]->(a) SET r.verdict = $verdict, r.why = $why",
        &params,
    )?;
    if let Some(obj) = result.as_object_mut() {
        obj.insert(
            "recorded".into(),
            json!({"goal": goal, "audit": key, "verdict": verdict}),
        );
    }
    Ok(result)
}

pub fn call(ctx: &Context, name: &str, args: &Json) -> ToolResult {
    let out = match name {
        "research_orbit_audit" => orbit_audit(ctx, args),
        "research_pooling_audit" => pooling_audit(ctx, args),
        "research_record" => record(ctx, args),
        other => Err(format!("unknown tool '{other}'")),
    }?;
    Ok(out.to_string())
}
