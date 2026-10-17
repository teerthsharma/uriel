#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;

use uriel_host::{
    ContextPack, ErrorCode, Hit, HitVia, HostConfig, HostStats, QueryResult, QueryStats, Request,
    Response, SearchOptions, Value,
};
use uriel_host::AccessMode;

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }

    pub fn draw(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.draw() % n
    }

    pub fn index(&mut self, n: usize) -> usize {
        (self.draw() % n as u64) as usize
    }

    pub fn flag(&mut self) -> bool {
        self.draw() & 1 == 1
    }

    pub fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.draw() as u8).collect()
    }
}

const GLYPHS: [char; 12] = ['a', 'b', 'z', 'Q', '7', ' ', '_', '\u{e9}', '\u{df}', '\u{6f22}', '\u{1f980}', '\n'];

pub fn random_string(rng: &mut Rng, max_len: usize) -> String {
    let len = rng.index(max_len + 1);
    (0..len).map(|_| GLYPHS[rng.index(GLYPHS.len())]).collect()
}

fn random_f64(rng: &mut Rng) -> f64 {
    rng.below(2_000_001) as f64 / 8.0 - 125_000.0
}

fn random_f32(rng: &mut Rng) -> f32 {
    rng.below(200_001) as f32 / 4.0 - 25_000.0
}

pub fn random_value(rng: &mut Rng, depth: usize) -> Value {
    let kinds = if depth >= 3 { 6 } else { 9 };
    match rng.below(kinds) {
        0 => Value::Null,
        1 => Value::Bool(rng.flag()),
        2 => Value::Int(rng.draw() as i64),
        3 => Value::Float(random_f64(rng)),
        4 => Value::String(random_string(rng, 24)),
        5 => {
            let len = rng.index(24);
            Value::Bytes(rng.bytes(len))
        }
        6 => {
            let len = rng.index(4);
            Value::List((0..len).map(|_| random_value(rng, depth + 1)).collect())
        }
        7 => {
            let mut map = BTreeMap::new();
            for _ in 0..rng.index(4) {
                let key = random_string(rng, 8);
                let value = random_value(rng, depth + 1);
                map.insert(key, value);
            }
            Value::Map(map)
        }
        _ => {
            let len = rng.index(8);
            Value::Vector((0..len).map(|_| random_f32(rng)).collect())
        }
    }
}

pub fn random_request(rng: &mut Rng) -> Request {
    match rng.below(4) {
        0 => {
            let mut params = BTreeMap::new();
            for _ in 0..rng.index(3) {
                let key = random_string(rng, 6);
                let value = random_value(rng, 0);
                params.insert(key, value);
            }
            Request::Query {
                cypher: random_string(rng, 40),
                params,
                mode: if rng.flag() {
                    AccessMode::Read
                } else {
                    AccessMode::Write
                },
            }
        }
        1 => Request::Search {
            query: random_string(rng, 40),
            opts: SearchOptions {
                k: rng.index(100),
                budget_tokens: rng.index(10_000),
                expand_hops: rng.index(5),
                use_utility: rng.flag(),
            },
        },
        2 => Request::Stats,
        _ => Request::Ping { nonce: rng.draw() },
    }
}

fn random_hit(rng: &mut Rng) -> Hit {
    Hit {
        chunk_id: rng.draw(),
        file: random_string(rng, 20),
        start_line: rng.draw() as u32,
        end_line: rng.draw() as u32,
        memory: random_string(rng, 60),
        score: random_f32(rng),
        via: if rng.flag() {
            HitVia::Direct
        } else {
            HitVia::GraphNeighbour
        },
    }
}

pub fn random_response(rng: &mut Rng) -> Response {
    match rng.below(6) {
        0 => {
            let width = rng.index(4);
            let columns = (0..width).map(|_| random_string(rng, 8)).collect();
            let height = rng.index(4);
            let rows = (0..height)
                .map(|_| (0..width).map(|_| random_value(rng, 0)).collect())
                .collect();
            Response::Query(QueryResult {
                columns,
                rows,
                stats: QueryStats {
                    nodes_created: rng.draw(),
                    nodes_deleted: rng.draw(),
                    rels_created: rng.draw(),
                    rels_deleted: rng.draw(),
                    properties_set: rng.draw(),
                    labels_added: rng.draw(),
                    labels_removed: rng.draw(),
                    indexes_added: rng.draw(),
                    constraints_added: rng.draw(),
                    from_cache: rng.flag(),
                    rows_returned: rng.draw(),
                },
            })
        }
        1 => {
            let hits = rng.index(4);
            Response::Search(ContextPack {
                hits: (0..hits).map(|_| random_hit(rng)).collect(),
                text: random_string(rng, 80),
                tokens: rng.index(100_000),
                budget_tokens: rng.index(100_000),
                omitted: rng.index(100),
            })
        }
        2 => Response::Stats(HostStats {
            arena_cap: rng.draw(),
            arena_used: rng.draw(),
            epoch: rng.draw(),
            requests_served: rng.draw(),
            requests_failed: rng.draw(),
            requests_busy: rng.draw(),
            in_flight: rng.draw(),
            queue_len: rng.draw(),
            connections: rng.draw(),
            chunks: rng.draw(),
            files: rng.draw(),
        }),
        3 => Response::Pong { nonce: rng.draw() },
        4 => Response::Error {
            code: [
                ErrorCode::Query,
                ErrorCode::Search,
                ErrorCode::Malformed,
                ErrorCode::ShuttingDown,
                ErrorCode::TooLarge,
                ErrorCode::Internal,
            ][rng.index(6)],
            message: random_string(rng, 50),
        },
        _ => Response::Busy,
    }
}

pub const CHECKSUM_SOURCE: &str = "#[doc = \"Compute a rolling adler checksum over a byte buffer.\"]
pub fn compute_checksum(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for byte in data {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

#[doc = \"Render a table of rows as aligned plain text columns.\"]
pub fn render_table(rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    for row in rows {
        out.push_str(&row.join(\"  \"));
        out.push('\\n');
    }
    out
}
";

pub const NOTES_SOURCE: &str = "# Gardening notes

Tomatoes need full sunlight and regular watering during the summer months.
Compost enriches the soil with nutrients and improves drainage.

## Pruning

Prune roses in late winter before new growth begins to appear on the stems.
Remove dead wood and crossing branches to keep the plant healthy.
";

pub fn fixture(sock_name: &str) -> (tempfile::TempDir, HostConfig) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("src");
    fs::create_dir_all(&root).expect("mkdir");
    fs::write(root.join("a.rs"), CHECKSUM_SOURCE).expect("write a.rs");
    fs::write(root.join("notes.md"), NOTES_SOURCE).expect("write notes.md");
    let config = HostConfig {
        db_path: dir.path().join("host.redb"),
        root,
        hot_cap_bytes: 1 << 20,
        memory: uriel_core::MemoryBudget::mib(80),
        sock_name: sock_name.to_owned(),
    };
    (dir, config)
}

pub fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !condition() {
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}
