use std::path::PathBuf;
use std::{env, fs};

fn main() {
    let dir = env::args().nth(1).map_or_else(
        || PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/pages")),
        PathBuf::from,
    );
    fs::create_dir_all(&dir).expect("create output dir");
    for (name, page) in [
        ("index.html", uriel_page::demo::index()),
        (
            "department-research.html",
            uriel_page::demo::department_research(),
        ),
    ] {
        uriel_page::validate(&page).expect("demo page is valid");
        let html = uriel_page::render(&page);
        fs::write(dir.join(name), &html).expect("write page");
        println!("{name} {} bytes {}", html.len(), uriel_page::digest(&html));
    }
}
