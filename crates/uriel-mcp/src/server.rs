use std::io::{BufRead, Write};

use serde_json::{json, Value as Json};

use crate::config::Mode;
use crate::tools::{self, Context};

pub const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2024-11-05"];

pub mod code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
}

pub struct Server {
    ctx: Context,
    token_limit: usize,
}

struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

fn error_response(id: Json, code: i64, message: &str) -> Json {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

pub fn truncate(text: String, limit: usize) -> String {
    let total = text.chars().count();
    if total <= limit {
        return text;
    }
    let cut = text
        .char_indices()
        .nth(limit)
        .map_or(text.len(), |(i, _)| i);
    format!(
        "{}\n... [truncated: showing {limit} of {total} characters; refine the query or raise --token-limit]",
        &text[..cut]
    )
}

const NORMAL_RW: &str = "seal graph database, normal mode. Cypher and memory: get_neo4j_schema, read_neo4j_cypher, write_neo4j_cypher and the memory tools. Light research from cached and stored context: rag_index, rag_search, rag_expand, rag_feedback, seal_remember, seal_recall. Placement and views: view, why_hot, pin, unpin, replay, calibrate, tier_stats.";

const NORMAL_RO: &str = "seal graph database (read-only), normal mode. Use get_neo4j_schema, read_neo4j_cypher, the memory read tools, rag_search, rag_expand, seal_recall, view, why_hot, replay, calibrate and tier_stats.";

const HOUSE_RW: &str = "seal graph database, house mode. Cypher and memory: get_neo4j_schema, read_neo4j_cypher, write_neo4j_cypher and the memory tools. The differential: house_need, house_chart, house_merge, house_fact, house_next, house_report, house_test_run, house_leap, house_status, house_select_tests, house_config. Inspector research, one-sided evidence that can disprove a claim and never confirm one: research_orbit_audit, research_pooling_audit, research_record. Retrieval: rag_index, rag_search, rag_expand, rag_feedback, seal_remember, seal_recall. Placement and views: view, why_hot, pin, unpin, replay, calibrate, tier_stats.";

const HOUSE_RO: &str = "seal graph database (read-only), house mode. Use get_neo4j_schema, read_neo4j_cypher, the memory read tools, house_status, house_select_tests, house_config, research_orbit_audit, research_pooling_audit, rag_search, rag_expand, seal_recall, view, why_hot, replay, calibrate and tier_stats.";

fn instructions(mode: Mode, read_only: bool) -> &'static str {
    match (mode, read_only) {
        (Mode::Normal, false) => NORMAL_RW,
        (Mode::Normal, true) => NORMAL_RO,
        (Mode::House, false) => HOUSE_RW,
        (Mode::House, true) => HOUSE_RO,
    }
}

impl Server {
    pub fn new(ctx: Context, token_limit: usize) -> Self {
        Self { ctx, token_limit }
    }

    pub fn scaffold_fingerprint(&self) -> Result<String, String> {
        crate::prefix::scaffold(&self.ctx).map(|s| s.id)
    }

    pub fn handle_line(&self, line: &str) -> Option<String> {
        let parsed: Json = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return Some(
                    error_response(Json::Null, code::PARSE_ERROR, &format!("Parse error: {e}"))
                        .to_string(),
                )
            }
        };
        match parsed {
            Json::Array(items) => {
                if items.is_empty() {
                    return Some(
                        error_response(Json::Null, code::INVALID_REQUEST, "Empty batch")
                            .to_string(),
                    );
                }
                let out: Vec<Json> = items
                    .iter()
                    .filter_map(|m| self.handle_message(m))
                    .collect();
                (!out.is_empty()).then(|| Json::Array(out).to_string())
            }
            other => self.handle_message(&other).map(|r| r.to_string()),
        }
    }

    fn handle_message(&self, msg: &Json) -> Option<Json> {
        let Some(obj) = msg.as_object() else {
            return Some(error_response(
                Json::Null,
                code::INVALID_REQUEST,
                "Request must be a JSON object",
            ));
        };
        let id = obj.get("id").cloned();
        let valid_id = matches!(id, None | Some(Json::String(_) | Json::Number(_)));
        if !valid_id || obj.get("jsonrpc").and_then(Json::as_str) != Some("2.0") {
            return Some(error_response(
                id.filter(|_| valid_id).unwrap_or(Json::Null),
                code::INVALID_REQUEST,
                "Invalid request: expected jsonrpc 2.0 with a string or number id",
            ));
        }
        let Some(method) = obj.get("method").and_then(Json::as_str) else {
            if obj.contains_key("result") || obj.contains_key("error") {
                return None;
            }
            return Some(error_response(
                id.unwrap_or(Json::Null),
                code::INVALID_REQUEST,
                "Invalid request: missing method",
            ));
        };
        let params = obj.get("params").unwrap_or(&Json::Null);
        let outcome = self.dispatch(method, params);
        let id = id?;
        Some(match outcome {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(e) => error_response(id, e.code, &e.message),
        })
    }

    fn dispatch(&self, method: &str, params: &Json) -> Result<Json, RpcError> {
        match method {
            "initialize" => Ok(self.initialize(params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(tools::list_json(self.ctx.read_only, self.ctx.mode)),
            "tools/call" => self.tools_call(params),
            m if m.starts_with("notifications/") => Ok(Json::Null),
            other => Err(RpcError::new(
                code::METHOD_NOT_FOUND,
                format!("Method not found: {other}"),
            )),
        }
    }

    fn initialize(&self, params: &Json) -> Json {
        let requested = params.get("protocolVersion").and_then(Json::as_str);
        let version = requested
            .filter(|v| SUPPORTED_VERSIONS.contains(v))
            .unwrap_or(SUPPORTED_VERSIONS[0]);
        let instructions = instructions(self.ctx.mode, self.ctx.read_only);
        json!({
            "protocolVersion": version,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "uriel-mcp", "title": "seal graph database", "version": env!("CARGO_PKG_VERSION")},
            "instructions": instructions
        })
    }

    fn tools_call(&self, params: &Json) -> Result<Json, RpcError> {
        let name = params.get("name").and_then(Json::as_str).ok_or_else(|| {
            RpcError::new(code::INVALID_PARAMS, "tools/call requires a string 'name'")
        })?;
        let args = params.get("arguments").unwrap_or(&Json::Null);
        let outcome = tools::call(&self.ctx, name, args)
            .ok_or_else(|| RpcError::new(code::INVALID_PARAMS, format!("Unknown tool: {name}")))?;
        let (text, is_error) = match outcome {
            Ok(t) => (t, false),
            Err(t) => (t, true),
        };
        let text = truncate(text, self.token_limit);
        Ok(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
    }
}

pub fn serve(server: &Server, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(resp) = server.handle_line(line) {
            output.write_all(resp.as_bytes())?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_is_untouched() {
        assert_eq!(truncate("abc".into(), 3), "abc");
    }

    #[test]
    fn truncate_cuts_on_char_boundary_and_notes_totals() {
        let t = truncate("\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}".into(), 2);
        assert!(t.starts_with("\u{e9}\u{e9}\n"));
        assert!(t.contains("showing 2 of 5 characters"));
    }
}
