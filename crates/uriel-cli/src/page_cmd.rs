use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::json;

fn emit<W: Write>(out: &mut W, v: &serde_json::Value) -> Result<(), String> {
    writeln!(out, "{v}").map_err(|e| e.to_string())
}

pub fn build<W: Write>(out: &mut W, spec: &Path, to: Option<&Path>) -> Result<(), String> {
    let text = std::fs::read_to_string(spec).map_err(|e| format!("cannot read {}: {e}", spec.display()))?;
    let html = uriel_page::render_json(&text).map_err(|e| e.to_string())?;
    match to {
        Some(p) => {
            if let Some(dir) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(p, &html).map_err(|e| e.to_string())?;
            emit(out, &json!({"file": p, "bytes": html.len(), "digest": uriel_page::digest(&html)}))
        }
        None => out.write_all(html.as_bytes()).map_err(|e| e.to_string()),
    }
}

pub fn demo<W: Write>(out: &mut W, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    for (name, html) in [
        ("index.html", uriel_page::render(&uriel_page::demo::index())),
        (
            "department-research.html",
            uriel_page::render(&uriel_page::demo::department_research()),
        ),
    ] {
        std::fs::write(dir.join(name), &html).map_err(|e| e.to_string())?;
        files.push(json!({"file": name, "digest": uriel_page::digest(&html)}));
    }
    emit(out, &json!({"dir": dir, "pages": files}))
}

pub fn run<W: Write>(out: &mut W, cmd: &crate::PageCmd) -> Result<(), String> {
    match cmd {
        crate::PageCmd::Build { spec, out: to } => build(out, spec, to.as_deref()),
        crate::PageCmd::Demo { out: dir } => demo(out, &dir.clone().unwrap_or_else(|| PathBuf::from("docs").join("pages"))),
    }
}
