use std::collections::BTreeMap;

use uriel_core::{NodeId, Props, Value};
use uriel_query::{AccessMode, Engine, QueryResult};

pub const POINT_LOOKUP: &str = "MATCH (p:Person {id: $id}) RETURN p.name AS name";
pub const TWO_HOP: &str =
    "MATCH (p:Person {id: $id})-[:KNOWS]->(:Person)-[:KNOWS]->(f:Person) RETURN f.id AS id";

pub fn engine(nodes: usize) -> Engine {
    let engine = Engine::open_in_memory().expect("engine");
    let graph = engine.graph();
    graph.create_index("Person", "id").expect("index");
    let mut tx = graph.begin_write().expect("begin write");
    let ids: Vec<NodeId> = (0..nodes)
        .map(|i| {
            let mut props = Props::new();
            props.insert("id".to_string(), Value::Int(i as i64));
            props.insert("name".to_string(), Value::from(format!("person {i}")));
            tx.create_node(&["Person"], props).expect("create node")
        })
        .collect();
    for i in 0..nodes {
        for hop in [1, 7] {
            tx.create_rel(ids[i], ids[(i + hop) % nodes], "KNOWS", Props::new())
                .expect("create rel");
        }
    }
    tx.commit().expect("commit");
    engine
}

pub fn run(engine: &Engine, query: &str, id: usize) -> QueryResult {
    let mut params = BTreeMap::new();
    params.insert("id".to_string(), Value::Int(id as i64));
    engine
        .execute(query, &params, AccessMode::Write)
        .expect("execute")
}
