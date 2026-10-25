use serde_json::Value as Json;
use sha2::{Digest, Sha256};
use uriel_rag::SoulOptions;

use crate::tools::{self, Context};

pub const OUTPUT_CONTRACT_VERSION: u32 = 1;

const DOMAIN: &str = "uriel-scaffold";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scaffold {
    pub id: String,
    pub preamble: String,
}

fn field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

pub fn fingerprint_of(
    tool_schemas: &str,
    contract: u32,
    soul_digest: &str,
    mode: &str,
) -> String {
    let mut hasher = Sha256::new();
    field(&mut hasher, DOMAIN.as_bytes());
    field(&mut hasher, tool_schemas.as_bytes());
    field(&mut hasher, &contract.to_le_bytes());
    field(&mut hasher, soul_digest.as_bytes());
    field(&mut hasher, mode.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn scaffold(ctx: &Context) -> Result<Scaffold, String> {
    ctx.services.scaffold_with(|| compute(ctx))
}

fn compute(ctx: &Context) -> Result<Scaffold, String> {
    let rag = ctx.rag()?;
    let guide = rag
        .soul(&SoulOptions::default())
        .map_err(|e| e.to_string())?;
    let tools = tools::list_json_with(ctx.read_only, ctx.mode, ctx.terse).to_string();
    let mode = ctx.mode.name();
    let key = crate::terse::fingerprint_key(mode, ctx.terse);
    let id = fingerprint_of(&tools, OUTPUT_CONTRACT_VERSION, &guide.digest, &key);
    let preamble = format!(
        "uriel scaffold {id} mode {mode} contract {OUTPUT_CONTRACT_VERSION} soul {}",
        guide.digest
    );
    Ok(Scaffold { id, preamble })
}

pub fn is_led(body: &str) -> bool {
    body.starts_with("{\"prefix_id\":")
}

pub fn lead_json(ctx: &Context, body: String) -> Result<String, String> {
    if !ctx.services.stable_prefix {
        return Ok(body);
    }
    Ok(assemble(&scaffold(ctx)?, &body))
}

pub fn assemble(scaffold: &Scaffold, body: &str) -> String {
    let Some(rest) = body.strip_prefix("{\"") else {
        return body.to_string();
    };
    let preamble = Json::String(scaffold.preamble.clone()).to_string();
    let base = format!(
        "{{\"prefix_id\":\"{}\",\"preamble\":{preamble},\"stable_prefix_bytes\":",
        scaffold.id
    );
    let len = (1..=20)
        .map(|digits| base.len() + digits + 1)
        .find(|n| n.to_string().len() + base.len() + 1 == *n)
        .unwrap_or(base.len());
    format!("{base}{len},\"{rest}")
}
