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

pub fn fingerprint_of(tool_schemas: &str, contract: u32, soul_digest: &str) -> String {
    let mut hasher = Sha256::new();
    field(&mut hasher, DOMAIN.as_bytes());
    field(&mut hasher, tool_schemas.as_bytes());
    field(&mut hasher, &contract.to_le_bytes());
    field(&mut hasher, soul_digest.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn scaffold(ctx: &Context) -> Result<Scaffold, String> {
    let rag = ctx.services.rag(ctx.engine.graph())?;
    let guide = rag
        .soul(&SoulOptions::default())
        .map_err(|e| e.to_string())?;
    let tools = tools::list_json(ctx.read_only).to_string();
    let id = fingerprint_of(&tools, OUTPUT_CONTRACT_VERSION, &guide.digest);
    let preamble = format!(
        "uriel scaffold {id} contract {OUTPUT_CONTRACT_VERSION} soul {}",
        guide.digest
    );
    Ok(Scaffold { id, preamble })
}
