mod common;
use std::collections::BTreeMap;

use common::*;
use uriel_cache::{CacheNode, NodeConfig};
use uriel_core::Value;
use uriel_query::{AccessMode, Engine};

fn cached(e: &Engine, q: &str) -> (Vec<String>, bool) {
    let r = read(e, q);
    (render(&r), r.stats.from_cache)
}

#[test]
fn hit_then_invalidated_by_write() {
    let e = fixture();
    let q = "MATCH (n:Person) RETURN count(n)";
    assert_eq!(cached(&e, q), (vec!["[6]".to_string()], false));
    assert_eq!(cached(&e, q), (vec!["[6]".to_string()], true));
    assert_eq!(cached(&e, q), (vec!["[6]".to_string()], true));
    write(&e, "CREATE (:Person {name:'Zed'})");
    assert_eq!(cached(&e, q), (vec!["[7]".to_string()], false));
    assert_eq!(cached(&e, q), (vec!["[7]".to_string()], true));
}

#[test]
fn unrelated_labels_keep_their_entries() {
    let e = fixture();
    let q = "MATCH (n:Person) RETURN count(n)";
    cached(&e, q);
    assert!(cached(&e, q).1);
    write(&e, "CREATE (:Movie {title:'Other'})");
    assert!(
        cached(&e, q).1,
        "a Movie write must not evict a Person-only query"
    );
    write(&e, "MATCH (m:Movie {title:'Other'}) SET m.year = 1");
    assert!(cached(&e, q).1);
    write(&e, "MATCH (p:Person {name:'Frank'}) SET p.age = 1");
    let (rows, hit) = cached(&e, q);
    assert!(!hit, "a Person property write must evict");
    assert_eq!(rows, vec!["[6]"]);
}

#[test]
fn property_results_never_go_stale() {
    let e = fixture();
    let q = "MATCH (p:Person {name:'Frank'}) RETURN p.age";
    assert_eq!(cached(&e, q), (vec!["[null]".to_string()], false));
    assert!(cached(&e, q).1);
    write(&e, "MATCH (p:Person {name:'Frank'}) SET p.age = 77");
    assert_eq!(cached(&e, q), (vec!["[77]".to_string()], false));
    write(&e, "MATCH (p:Person {name:'Frank'}) REMOVE p.age");
    assert_eq!(cached(&e, q), (vec!["[null]".to_string()], false));
    write(&e, "MATCH (p:Person {name:'Frank'}) SET p:Extra");
    let q2 = "MATCH (p:Person {name:'Frank'}) RETURN labels(p)";
    assert_eq!(
        cached(&e, q2),
        (vec!["[[\"Extra\",\"Person\"]]".to_string()], false)
    );
    write(&e, "MATCH (p:Extra) REMOVE p:Extra");
    assert_eq!(cached(&e, q2), (vec!["[[\"Person\"]]".to_string()], false));
}

#[test]
fn unlabeled_and_untyped_queries_depend_on_everything() {
    let e = fixture();
    for q in [
        "MATCH (n) RETURN count(n)",
        "MATCH (a:Person {name:'Alice'})-[r]->(b) RETURN count(r)",
        "MATCH (a:Person {name:'Alice'})-[:KNOWS]->(b) RETURN count(b)",
        "CALL db.labels() YIELD label RETURN count(label)",
    ] {
        let first = cached(&e, q);
        assert!(!first.1);
        assert!(cached(&e, q).1, "{q}");
        write(&e, "CREATE (:Unrelated {x: 1})");
        let after = cached(&e, q);
        assert!(!after.1, "{q} should be invalidated by any commit");
        write(&e, "MATCH (u:Unrelated) DELETE u");
        let _ = first;
    }
    let q = "MATCH (n) RETURN count(n)";
    cached(&e, q);
    write(&e, "CREATE (:Movie {title:'Added'})");
    assert_eq!(cached(&e, q), (vec!["[10]".to_string()], false));
}

#[test]
fn relationship_type_tags() {
    let e = fixture();
    let q = "MATCH (:Person)-[r:DIRECTED]->(:Movie) RETURN count(r)";
    cached(&e, q);
    assert!(cached(&e, q).1);
    write(
        &e,
        "MATCH (a:Person {name:'Eve'}), (m:Movie {title:'Dune'}) CREATE (a)-[:DIRECTED]->(m)",
    );
    assert_eq!(cached(&e, q), (vec!["[2]".to_string()], false));
    write(
        &e,
        "MATCH ()-[r:DIRECTED]->(:Movie {title:'Dune'}) DELETE r",
    );
    assert_eq!(cached(&e, q), (vec!["[1]".to_string()], false));
}

#[test]
fn keys_depend_on_params_and_mode() {
    let e = fixture();
    let q = "MATCH (n:Person {name:$name}) RETURN n.age";
    let mut p1 = BTreeMap::new();
    p1.insert("name".to_string(), Value::from("Alice"));
    let mut p2 = BTreeMap::new();
    p2.insert("name".to_string(), Value::from("Bob"));
    let a = e.execute(q, &p1, AccessMode::Read).unwrap();
    let b = e.execute(q, &p2, AccessMode::Read).unwrap();
    assert!(!a.stats.from_cache && !b.stats.from_cache);
    assert_eq!(render(&a), vec!["[30]"]);
    assert_eq!(render(&b), vec!["[25]"]);
    let a2 = e.execute(q, &p1, AccessMode::Read).unwrap();
    let b2 = e.execute(q, &p2, AccessMode::Read).unwrap();
    assert!(a2.stats.from_cache && b2.stats.from_cache);
    assert_eq!(render(&a2), vec!["[30]"]);
    assert_eq!(render(&b2), vec!["[25]"]);
    // write mode never reads or fills the cache
    let r = e
        .execute(
            "MATCH (n:Person) RETURN count(n)",
            &no_params(),
            AccessMode::Write,
        )
        .unwrap();
    assert!(!r.stats.from_cache);
    let r = e
        .execute(
            "MATCH (n:Person) RETURN count(n)",
            &no_params(),
            AccessMode::Write,
        )
        .unwrap();
    assert!(!r.stats.from_cache);
    // PROFILE / EXPLAIN are not cached
    assert!(
        !read(&e, "PROFILE MATCH (n:Person) RETURN count(n)")
            .stats
            .from_cache
    );
    assert!(
        !read(&e, "PROFILE MATCH (n:Person) RETURN count(n)")
            .stats
            .from_cache
    );
}

#[test]
fn nondeterministic_and_failed_queries_are_not_cached() {
    let e = fixture();
    let q = "RETURN rand() AS r";
    assert!(!cached(&e, q).1);
    assert!(!cached(&e, q).1);
    assert!(!cached(&e, "RETURN timestamp()").1);
    assert!(!cached(&e, "RETURN timestamp()").1);
    assert!(e
        .execute("RETURN nope()", &no_params(), AccessMode::Read)
        .is_err());
    assert!(e
        .execute("RETURN nope()", &no_params(), AccessMode::Read)
        .is_err());
    // a query without graph access is cached and never invalidated
    assert!(!cached(&e, "RETURN 1 + 1").1);
    write(&e, "CREATE (:Any)");
    assert!(cached(&e, "RETURN 1 + 1").1);
    // cached rows keep their exact values, including nodes and nulls
    let q = "MATCH (m:Movie {title:'Dune'}) RETURN m, null AS z, 1.5 AS f, [1,'a'] AS l";
    let fresh = read(&e, q);
    let hit = read(&e, q);
    assert!(hit.stats.from_cache);
    assert_eq!(fresh.rows, hit.rows);
    assert_eq!(fresh.columns, hit.columns);
}

#[test]
fn index_creation_invalidates() {
    let e = fixture();
    let q = "MATCH (p:Person {name:'Alice'}) RETURN p.age";
    cached(&e, q);
    assert!(cached(&e, q).1);
    write(&e, "CREATE INDEX FOR (p:Person) ON (p.name)");
    assert!(!cached(&e, q).1);
    assert_eq!(cached(&e, q), (vec!["[30]".to_string()], true));
}

#[test]
fn distributed_cache_bridge_with_one_node() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let node = rt
        .block_on(CacheNode::spawn(NodeConfig::default()))
        .unwrap();
    let dist = node.cache().clone();
    let e = fixture().with_dist_cache(dist.clone());
    let q = "MATCH (n:Person) RETURN count(n)";
    assert_eq!(cached(&e, q), (vec!["[6]".to_string()], false));
    assert!(
        !dist.local().is_empty(),
        "result must be stored in the distributed cache"
    );
    assert!(cached(&e, q).1);

    // a second engine with an empty graph is served the shared entry
    let other = Engine::open_in_memory(uriel_core::MemoryBudget::mib(80))
        .unwrap()
        .with_dist_cache(dist.clone());
    assert_eq!(cached(&other, q), (vec!["[6]".to_string()], true));

    // a commit invalidates the distributed entry too
    write(&e, "CREATE (:Person {name:'Zed'})");
    assert_eq!(cached(&e, q), (vec!["[7]".to_string()], false));
    assert_eq!(cached(&e, q), (vec!["[7]".to_string()], true));
    let before = dist.local().len();
    write(&e, "CREATE (:Person {name:'Zed2'})");
    assert_eq!(cached(&e, q), (vec!["[8]".to_string()], false));
    assert!(dist.local().len() <= before + 1);
    rt.block_on(node.shutdown());
}
