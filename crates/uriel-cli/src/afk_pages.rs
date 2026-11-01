use std::collections::BTreeMap;

use serde_json::{json, Value};

fn stage_chip(stage: Option<&str>, status: Option<&str>) -> (&'static str, &'static str) {
    match (status, stage) {
        (Some("Done"), _) => ("done", "done"),
        (Some(s), _) if s != "open" => ("alert", "ended"),
        (_, Some("PersonReview")) => ("done", "review"),
        (_, Some("PersonAnswer")) => ("alert", "answer"),
        (_, Some("PersonBlocked")) => ("alert", "blocked"),
        (_, Some("PersonDelivery")) => ("alert", "undelivered"),
        (_, Some("Working")) => ("ok", "working"),
        (_, Some("Queued")) => ("idle", "queued"),
        (_, Some("Idle")) => ("watch", "idle"),
        _ => ("idle", "not started"),
    }
}

fn pct(p: &Value) -> String {
    let done = p["done"].as_u64().unwrap_or(0);
    let total = p["total"].as_u64().unwrap_or(0);
    if total == 0 {
        "0 of 0".to_string()
    } else {
        format!("{done} of {total}")
    }
}

fn dept_state(p: &Value, held: u64) -> &'static str {
    let done = p["done"].as_u64().unwrap_or(0);
    let total = p["total"].as_u64().unwrap_or(0);
    if total > 0 && done == total {
        "done"
    } else if held > 0 {
        "ok"
    } else {
        "idle"
    }
}

fn tile_state(s: &'static str) -> &'static str {
    if s == "done" {
        "ok"
    } else {
        s
    }
}

fn nav(depts: &[Value], current: Option<&str>) -> Vec<Value> {
    let mut v = vec![json!({"label": "Board", "href": "index.html", "current": current.is_none()})];
    for d in depts {
        let slug = d["slug"].as_str().unwrap_or("");
        v.push(json!({
            "label": d["name"],
            "href": format!("department-{slug}.html"),
            "current": current == Some(slug),
        }));
    }
    v
}

pub fn board(status: &Value, owner: &str, started: u64) -> Value {
    let depts: Vec<Value> = status["departments"].as_array().cloned().unwrap_or_default();
    let w = &status["window"];
    let open = w["open"].as_bool().unwrap_or(false);
    let remaining = w["remaining_s"].as_u64().unwrap_or(0);
    let total = depts.iter().fold((0u64, 0u64), |a, d| {
        (a.0 + d["progress"]["done"].as_u64().unwrap_or(0), a.1 + d["progress"]["total"].as_u64().unwrap_or(0))
    });
    let tickets = status["tickets"].as_array().map_or(0, Vec::len);
    let held = status["slots"]["regular"].as_u64().unwrap_or(0);
    let rows: Vec<Value> = depts
        .iter()
        .map(|d| {
            let slug = d["slug"].as_str().unwrap_or("");
            let held_d = d["held"].as_u64().unwrap_or(0);
            json!([
                {"text": d["name"], "href": format!("department-{slug}.html")},
                d["goal"],
                pct(&d["progress"]),
                format!("{held_d} of {} slots", d["share"].as_u64().unwrap_or(0)),
                {"text": match dept_state(&d["progress"], held_d) { "done" => "complete", "ok" => "running", _ => "waiting" }, "chip": dept_state(&d["progress"], held_d)},
            ])
        })
        .collect();
    let mut blocks = vec![
        json!({"type": "vitals", "tiles": [
            {"label": "Window", "value": if open { format!("{} min", remaining / 60) } else { "closed".to_string() }, "state": if open { "ok" } else { "idle" }, "hint": "time left to start work"},
            {"label": "Workers", "value": format!("{held} of {}", status["capacity"].as_u64().unwrap_or(0)), "state": if held > 0 { "ok" } else { "idle" }},
            {"label": "Tickets", "value": tickets.to_string()},
            {"label": "Checklist", "value": pct(&json!({"done": total.0, "total": total.1})), "state": if total.1 > 0 && total.0 == total.1 { "ok" } else { "watch" }},
        ]}),
        json!({"type": "progress", "label": "Whole plan", "done": total.0, "total": total.1.max(total.0)}),
    ];
    if rows.is_empty() {
        blocks.push(json!({"type": "notes", "title": "No plan yet", "paragraphs": ["Set a three-tier plan with `uriel afk plan <file>`; departments and their pages appear here."]}));
    } else {
        blocks.push(json!({"type": "table", "caption": "Departments", "columns": ["Department", "Goal", "Checklist", "Slots now of share", "State"], "rows": rows}));
    }
    json!({
        "title": "Uriel dispatch board",
        "subtitle": "One goal, split among departments; workers are shared by load",
        "chart_id": format!("AFK-{started}"),
        "ward": "Dispatch",
        "attending": owner,
        "theme": "auto",
        "nav": nav(&depts, None),
        "blocks": blocks,
        "footer": "Generated from the afk log. Same log, same page.",
    })
}

pub fn department(status: &Value, slug: &str, owner: &str, started: u64) -> Option<Value> {
    let depts: Vec<Value> = status["departments"].as_array().cloned().unwrap_or_default();
    let d = depts.iter().find(|d| d["slug"] == slug)?;
    let held = d["held"].as_u64().unwrap_or(0);
    let mut blocks = vec![
        json!({"type": "vitals", "tiles": [
            {"label": "Checklist", "value": pct(&d["progress"]), "state": tile_state(dept_state(&d["progress"], held))},
            {"label": "Open work", "value": d["load"].as_u64().unwrap_or(0).to_string()},
            {"label": "Slots now", "value": held.to_string(), "hint": "workers running for this department"},
            {"label": "Fair share", "value": d["share"].as_u64().unwrap_or(0).to_string(), "hint": "by load, across departments"},
        ]}),
        json!({"type": "progress", "label": "Department progress", "done": d["progress"]["done"], "total": d["progress"]["total"]}),
    ];
    let mut by_sub: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for s in d["subgoals"].as_array().cloned().unwrap_or_default() {
        let sid = s["id"].as_str().unwrap_or("").to_string();
        let mut inner = vec![json!({"type": "progress", "label": "Sub-goal progress", "done": s["progress"]["done"], "total": s["progress"]["total"]})];
        let leaves = s["leaves"].as_array().cloned().unwrap_or_default();
        let rows: Vec<Value> = leaves
            .iter()
            .map(|l| {
                let (chip, label) = stage_chip(l["stage"].as_str(), l["status"].as_str());
                json!([
                    l["id"],
                    l["goal"],
                    l["ticket"].as_u64().map_or_else(|| "none".to_string(), |t| format!("#{t}")),
                    pct(&l["progress"]),
                    {"text": label, "chip": chip},
                ])
            })
            .collect();
        inner.push(json!({"type": "table", "caption": format!("Leaves of {sid}"), "columns": ["Leaf", "Goal", "Ticket", "Checklist", "Stage"], "rows": rows}));
        for l in &leaves {
            let items: Vec<Value> = l["checklist"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|i| json!({"text": i["text"], "done": i["done"]}))
                .collect();
            let mut item_list = items;
            if let Some(r) = l["result"].as_str() {
                item_list.push(json!({"text": "Worker result", "done": true, "note": r}));
            }
            inner.push(json!({"type": "checklist", "title": format!("{}: {}", l["id"].as_str().unwrap_or(""), l["goal"].as_str().unwrap_or("")), "items": item_list}));
        }
        by_sub.entry(sid.clone()).or_default().push(json!({"type": "section", "title": format!("{sid}: {}", s["goal"].as_str().unwrap_or("")), "blocks": inner}));
    }
    for (_, secs) in by_sub {
        blocks.extend(secs);
    }
    Some(json!({
        "title": d["name"],
        "subtitle": d["goal"],
        "chart_id": format!("AFK-{started}-{slug}"),
        "ward": "Department",
        "attending": owner,
        "theme": "auto",
        "nav": nav(&depts, Some(slug)),
        "blocks": blocks,
        "footer": "Generated from the afk log. Same log, same page.",
    }))
}

pub fn render_all(status: &Value, owner: &str, started: u64) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let b = board(status, owner, started);
    out.push(("index.html".to_string(), uriel_page::render_json(&b.to_string()).map_err(|e| e.to_string())?));
    for d in status["departments"].as_array().cloned().unwrap_or_default() {
        let slug = d["slug"].as_str().unwrap_or("").to_string();
        if let Some(spec) = department(status, &slug, owner, started) {
            out.push((format!("department-{slug}.html"), uriel_page::render_json(&spec.to_string()).map_err(|e| e.to_string())?));
        }
    }
    Ok(out)
}
