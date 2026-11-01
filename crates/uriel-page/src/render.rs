use crate::spec::{Block, Cell, Chip, Entry, Item, NavLink, Page, State, Theme, Tile};
use crate::validate::{href_allowed, MAX_DEPTH};

pub const CSS: &str = include_str!("style.css");
const DARK: &str = include_str!("dark.css");

pub fn escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '`' => out.push_str("&#96;"),
            '\n' | '\t' => out.push(c),
            c if c.is_control() => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.chars() {
        if out.len() >= 32 {
            break;
        }
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push('x');
    }
    out
}

fn state_class(s: State) -> &'static str {
    match s {
        State::Ok => "ok",
        State::Watch => "wa",
        State::Alert => "al",
        State::Idle => "id",
    }
}

fn state_word(s: State) -> &'static str {
    match s {
        State::Ok => "OK",
        State::Watch => "Watch",
        State::Alert => "Alert",
        State::Idle => "Idle",
    }
}

fn chip_class(c: Chip) -> &'static str {
    match c {
        Chip::Ok => "ok",
        Chip::Watch => "wa",
        Chip::Alert => "al",
        Chip::Idle => "id",
        Chip::Done => "dn",
    }
}

fn chip_word(c: Chip) -> &'static str {
    match c {
        Chip::Ok => "ok",
        Chip::Watch => "watch",
        Chip::Alert => "alert",
        Chip::Idle => "idle",
        Chip::Done => "done",
    }
}

struct R {
    out: String,
    n: usize,
}

impl R {
    fn raw(&mut self, s: &str) {
        self.out.push_str(s);
    }

    fn esc(&mut self, s: &str) {
        escape(s, &mut self.out);
    }

    fn id(&mut self, text: &str) -> String {
        self.n += 1;
        format!("c{}-{}", self.n, slug(text))
    }

    fn glyph(&mut self, class: &str) {
        self.raw("<i class=\"g g-");
        self.raw(class);
        self.raw("\" aria-hidden=\"true\"></i>");
    }

    fn vh(&mut self, text: &str) {
        self.raw("<span class=\"vh\">");
        self.raw(text);
        self.raw("</span>");
    }

    fn heading(&mut self, level: usize, id: &str, text: &str) {
        let l = level.clamp(2, 6);
        self.raw(&format!("<h{l} id=\"{id}\" dir=\"auto\">"));
        self.esc(text);
        self.raw(&format!("</h{l}>"));
    }

    fn link_open(&mut self, href: &str, extra: &str) -> bool {
        if href_allowed(href) {
            self.raw("<a");
            self.raw(extra);
            self.raw(" href=\"");
            self.esc(href);
            self.raw("\">");
            true
        } else {
            false
        }
    }

    fn blocks(&mut self, list: &[Block], depth: usize, level: usize) {
        for b in list {
            self.block(b, depth, level);
        }
    }

    fn block(&mut self, b: &Block, depth: usize, level: usize) {
        match b {
            Block::Vitals { tiles } => self.vitals(tiles),
            Block::Table {
                caption,
                columns,
                rows,
            } => self.table(caption, columns, rows),
            Block::Checklist { title, items } => self.checklist(title, items, level),
            Block::Progress { label, done, total } => self.progress(label, *done, *total),
            Block::Timeline { title, entries } => self.timeline(title, entries, level),
            Block::Notes { title, paragraphs } => self.notes(title, paragraphs, level),
            Block::Section { title, blocks } => {
                if depth >= MAX_DEPTH {
                    return;
                }
                let id = self.id(title);
                self.raw(&format!("<section class=\"sec\" aria-labelledby=\"{id}\">"));
                self.heading(level, &id, title);
                self.blocks(blocks, depth + 1, level + 1);
                self.raw("</section>");
            }
            Block::Columns { blocks } => {
                if depth >= MAX_DEPTH {
                    return;
                }
                self.raw("<div class=\"cols\">");
                self.blocks(blocks, depth + 1, level);
                self.raw("</div>");
            }
            Block::Rule => self.raw("<hr>"),
        }
    }

    fn vitals(&mut self, tiles: &[Tile]) {
        self.raw("<section aria-label=\"Vitals\"><dl class=\"vt\">");
        for t in tiles {
            match t.state {
                Some(s) => self.raw(&format!("<div class=\"s-{}\">", state_class(s))),
                None => self.raw("<div>"),
            }
            self.raw("<dt dir=\"auto\">");
            self.esc(&t.label);
            self.raw("</dt><dd class=\"v\" dir=\"auto\">");
            self.esc(&t.value);
            if let Some(u) = &t.unit {
                self.raw("<span class=\"u\">");
                self.esc(u);
                self.raw("</span>");
            }
            self.raw("</dd>");
            if let Some(s) = t.state {
                let c = state_class(s);
                self.raw(&format!("<dd class=\"st s-{c}\">"));
                self.glyph(c);
                self.raw(state_word(s));
                self.raw("</dd>");
            }
            if let Some(h) = &t.hint {
                self.raw("<dd class=\"h\" dir=\"auto\">");
                self.esc(h);
                self.raw("</dd>");
            }
            self.raw("</div>");
        }
        self.raw("</dl></section>");
    }

    fn table(&mut self, caption: &str, columns: &[String], rows: &[Vec<Cell>]) {
        let id = self.id(caption);
        self.raw(&format!(
            "<div class=\"tw\" role=\"region\" aria-labelledby=\"{id}\" tabindex=\"0\"><table><caption id=\"{id}\" dir=\"auto\">"
        ));
        self.esc(caption);
        self.raw("</caption><thead><tr>");
        for c in columns {
            self.raw("<th scope=\"col\" dir=\"auto\">");
            self.esc(c);
            self.raw("</th>");
        }
        self.raw("</tr></thead><tbody>");
        for row in rows {
            self.raw("<tr>");
            for (i, cell) in row.iter().enumerate() {
                let tag = if i == 0 { "th scope=\"row\"" } else { "td" };
                let close = if i == 0 { "th" } else { "td" };
                self.raw(&format!("<{tag} dir=\"auto\">"));
                self.cell(cell);
                self.raw(&format!("</{close}>"));
            }
            self.raw("</tr>");
        }
        self.raw("</tbody></table></div>");
    }

    fn cell(&mut self, cell: &Cell) {
        match cell {
            Cell::Text(t) => self.esc(t),
            Cell::Chip(c) => {
                let k = chip_class(c.chip);
                self.raw(&format!("<span class=\"chip s-{k}\">"));
                self.glyph(k);
                self.esc(&c.text);
                self.raw(" ");
                self.vh(&format!("({})", chip_word(c.chip)));
                self.raw("</span>");
            }
            Cell::Link(l) => {
                let linked = self.link_open(&l.href, "");
                self.esc(&l.text);
                if linked {
                    self.raw("</a>");
                }
            }
        }
    }

    fn checklist(&mut self, title: &str, items: &[Item], level: usize) {
        let id = self.id(title);
        self.raw(&format!("<section class=\"sec\" aria-labelledby=\"{id}\">"));
        self.heading(level, &id, title);
        self.raw("<ul class=\"chk\">");
        for i in items {
            self.raw(if i.done {
                "<li class=\"done\">"
            } else {
                "<li>"
            });
            self.raw("<span class=\"box\" aria-hidden=\"true\"></span><span dir=\"auto\">");
            self.vh(if i.done { "Done: " } else { "Open: " });
            self.esc(&i.text);
            self.raw("</span>");
            if let Some(n) = &i.note {
                self.raw("<span class=\"n\" dir=\"auto\">");
                self.esc(n);
                self.raw("</span>");
            }
            self.raw("</li>");
        }
        self.raw("</ul></section>");
    }

    fn progress(&mut self, label: &str, done: u32, total: u32) {
        let done = done.min(total);
        let pct = if total == 0 {
            0
        } else {
            (u64::from(done) * 100 / u64::from(total)) as u32
        };
        self.raw("<div class=\"prog\"><p class=\"prog-t\"><span dir=\"auto\">");
        self.esc(label);
        self.raw(&format!(
            "</span><span>{done} of {total} ({pct}%)</span></p>"
        ));
        self.raw("<div class=\"bar\" role=\"img\" aria-label=\"");
        self.esc(label);
        self.raw(&format!(
            ": {done} of {total}\"><i style=\"width:{pct}%\"></i></div></div>"
        ));
    }

    fn timeline(&mut self, title: &str, entries: &[Entry], level: usize) {
        let id = self.id(title);
        self.raw(&format!("<section class=\"sec\" aria-labelledby=\"{id}\">"));
        self.heading(level, &id, title);
        self.raw("<ol class=\"tl\">");
        for e in entries {
            let k = e.state.map_or("id", state_class);
            self.raw(&format!(
                "<li class=\"s-{k}\"><span class=\"at\" dir=\"auto\">"
            ));
            self.esc(&e.at);
            self.raw("</span><span class=\"nd\">");
            self.glyph(k);
            self.raw("</span><div class=\"bd\"><span class=\"who\" dir=\"auto\">");
            self.esc(&e.who);
            self.raw("</span>");
            if let Some(s) = e.state {
                self.raw(" ");
                self.vh(&format!("({})", state_word(s)));
            }
            self.raw("<p dir=\"auto\">");
            self.esc(&e.text);
            self.raw("</p></div></li>");
        }
        self.raw("</ol></section>");
    }

    fn notes(&mut self, title: &str, paragraphs: &[String], level: usize) {
        let id = self.id(title);
        self.raw(&format!("<section class=\"sec\" aria-labelledby=\"{id}\">"));
        self.heading(level, &id, title);
        self.raw("<div class=\"notes\">");
        for p in paragraphs {
            self.raw("<p dir=\"auto\">");
            self.esc(p);
            self.raw("</p>");
        }
        self.raw("<div class=\"pad\"></div></div></section>");
    }

    fn nav(&mut self, nav: &[NavLink]) {
        if nav.is_empty() {
            return;
        }
        self.raw("<nav class=\"tabs\" aria-label=\"Chart sections\"><ul>");
        for n in nav {
            self.raw("<li>");
            let extra = if n.current {
                " aria-current=\"page\""
            } else {
                ""
            };
            if self.link_open(&n.href, extra) {
                self.esc(&n.label);
                self.raw("</a>");
            } else {
                self.esc(&n.label);
            }
            self.raw("</li>");
        }
        self.raw("</ul></nav>");
    }

    fn header(&mut self, page: &Page) {
        self.raw("<header class=\"hd\"><div><h1 dir=\"auto\">");
        self.esc(&page.title);
        self.raw("</h1>");
        if let Some(s) = &page.subtitle {
            self.raw("<p class=\"sub\" dir=\"auto\">");
            self.esc(s);
            self.raw("</p>");
        }
        self.raw("</div>");
        let fields = [
            ("Chart no.", &page.chart_id),
            ("Ward", &page.ward),
            ("Attending", &page.attending),
        ];
        if fields.iter().any(|(_, v)| v.is_some()) {
            self.raw("<dl class=\"fields\">");
            for (k, v) in fields {
                if let Some(v) = v {
                    self.raw(&format!("<div><dt>{k}</dt><dd dir=\"auto\">"));
                    self.esc(v);
                    self.raw("</dd></div>");
                }
            }
            self.raw("</dl>");
        }
        self.raw("</header>");
    }
}

pub fn render(page: &Page) -> String {
    let mut r = R {
        out: String::with_capacity(CSS.len() + 4096),
        n: 0,
    };
    let (theme_attr, scheme) = match page.theme {
        Theme::Auto => ("", "light dark"),
        Theme::Light => (" data-theme=\"light\"", "light"),
        Theme::Dark => (" data-theme=\"dark\"", "dark"),
    };
    r.raw(&format!(
        "<!doctype html>\n<html lang=\"en\"{theme_attr}>\n<head>\n<meta charset=\"utf-8\">\n"
    ));
    r.raw("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    r.raw(&format!(
        "<meta name=\"color-scheme\" content=\"{scheme}\">\n"
    ));
    if let Some(s) = &page.subtitle {
        r.raw("<meta name=\"description\" content=\"");
        r.esc(s);
        r.raw("\">\n");
    }
    r.raw("<title>");
    r.esc(&page.title);
    r.raw("</title>\n<style>\n");
    r.raw(CSS);
    match page.theme {
        Theme::Auto => r.raw(&format!(
            "@media(prefers-color-scheme:dark){{:root{{{DARK}}}}}\n"
        )),
        Theme::Light => {}
        Theme::Dark => r.raw(&format!(":root{{{DARK}}}\n")),
    }
    r.raw("</style>\n</head>\n<body>\n<a class=\"skip\" href=\"#main\">Skip to chart</a>\n<div class=\"sheet\">\n");
    r.header(page);
    r.nav(&page.nav);
    r.raw("<main id=\"main\">");
    if page.blocks.is_empty() {
        r.raw("<p class=\"empty\">Nothing is charted on this page yet.</p>");
    } else {
        r.blocks(&page.blocks, 0, 2);
    }
    r.raw("</main>\n");
    if let Some(f) = &page.footer {
        r.raw("<footer dir=\"auto\">");
        r.esc(f);
        r.raw("</footer>\n");
    }
    r.raw("</div>\n</body>\n</html>\n");
    r.out
}
