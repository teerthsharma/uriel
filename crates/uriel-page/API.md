# uriel-page API

Deterministic hospital-chart pages: one spec in, one self-contained HTML file out. No JavaScript, no external requests, no web fonts, no timestamps (a date appears only if the spec contains it). Same spec, same bytes on every machine. Digests are pinned in `tests/golden/`.

## Functions

| Item | Use |
|---|---|
| `render(&Page) -> String` | Draw a page. Total: never panics. Escapes all text and attributes, drops links whose href fails the policy, stops at the nesting cap. Call `validate` first for caps and shape errors. |
| `validate(&Page) -> Result<(), PageError>` | Hard caps and shape checks (below). |
| `render_json(&str) -> Result<String, PageError>` | Size cap (1 MiB), parse (unknown fields are errors), `validate`, `render`. Use this for CLI input. |
| `digest(&str) -> String` | Lowercase hex SHA-256 of the HTML. |
| `href_allowed(&str) -> bool`, `escape`, `slug` | Policy and helpers. |
| `demo::index()`, `demo::department_research()` | The sample pages. `cargo run -p uriel-page --example demo [dir]` writes `index.html` and `department-research.html` (default `docs/pages`). `tests/docs_pages.rs` fails if the committed files drift. |

## Spec (JSON; Rust types in `uriel_page::spec`, also re-exported at the crate root)

```json
{ "title": "..", "subtitle": "..", "chart_id": "..", "ward": "..", "attending": "..",
  "theme": "auto|light|dark",
  "nav": [{ "label": "..", "href": "..", "current": true }],
  "blocks": [ ... ], "footer": ".." }
```

Only `title` is required. Blocks use `"type"` in snake_case:

| type | fields |
|---|---|
| `vitals` | `tiles: [{label, value, unit?, state?, hint?}]` |
| `table` | `caption`, `columns: [String]`, `rows: [[Cell]]` (every row has one cell per column) |
| `checklist` | `title`, `items: [{text, done?, note?}]` |
| `progress` | `label`, `done`, `total` (`done <= total`); shows "x of y (n%)" as text |
| `timeline` | `title`, `entries: [{at, who, text, state?}]` |
| `notes` | `title`, `paragraphs: [String]` |
| `section` | `title`, `blocks` (nested; heading level follows depth, clamped at h6) |
| `columns` | `blocks` (responsive 2-3 columns) |
| `rule` | none |

`state`: `ok | watch | alert | idle`. A `Cell` is `"text"`, `{"text", "chip": "ok|watch|alert|idle|done"}` or `{"text", "href"}`. The first column of a table is the row header.

Rust helpers: `Page::new(title)`, `Tile::new(label, value)`, `Cell::text/chip/link`.

## Policy and caps

- href: relative path, `#anchor`, `http(s)://host`, `mailto:addr`. Everything else (`javascript:`, `data:`, `//host`, control characters, spaces, backslashes) fails `validate` and is rendered as plain text by `render`.
- Caps: 6 nesting levels, 500 blocks, 2000 chars per string, 256 KiB total text, 16 nav links, 24 tiles, 16 columns, 500 rows, 5000 cells, 300 items or entries, 200 paragraphs, 12 blocks per `columns`.
- Errors: `PageError::{Parse, InputTooLarge, Limit, UnsafeHref, Shape}`.

## Output contract

`<!doctype html>`, `lang="en"`, one `h1`, skip link, `nav aria-label`, one `main`, `section aria-labelledby`, tables with `caption` and `th scope`. Status is a shape (CSS glyph) plus a word, never colour alone. Ids are `c{n}-{slug}` by document order. Light, dark (`prefers-color-scheme`) or forced via `theme`; print stylesheet for A4; text direction is `auto` per element. CSS is a shared constant (`CSS`) of about 9 KB; a typical board is 15 to 16 KB.
