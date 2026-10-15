#![allow(dead_code)]

use std::collections::BTreeMap;

use uriel_core::Value;
use uriel_query::{AccessMode, Engine, QueryError, QueryResult};

pub const FIXTURE: &str = "CREATE
 (alice:Person {name:'Alice', age:30, city:'London'}),
 (bob:Person {name:'Bob', age:25, city:'Paris'}),
 (carol:Person {name:'Carol', age:35, city:'London'}),
 (dave:Person {name:'Dave', age:28, city:'Berlin'}),
 (eve:Person {name:'Eve', age:40}),
 (frank:Person {name:'Frank'}),
 (alice)-[:KNOWS {since:2015}]->(bob),
 (bob)-[:KNOWS {since:2018}]->(carol),
 (carol)-[:KNOWS {since:2020}]->(dave),
 (alice)-[:KNOWS {since:2010}]->(carol),
 (dave)-[:KNOWS {since:2021}]->(eve),
 (matrix:Movie {title:'The Matrix', year:1999}),
 (inception:Movie {title:'Inception', year:2010}),
 (dune:Movie {title:'Dune', year:2021}),
 (alice)-[:ACTED_IN {role:'Neo'}]->(matrix),
 (bob)-[:ACTED_IN {role:'Morpheus'}]->(matrix),
 (bob)-[:ACTED_IN {role:'Cobb'}]->(inception),
 (carol)-[:ACTED_IN {role:'Paul'}]->(dune),
 (dave)-[:DIRECTED]->(inception)";

pub fn no_params() -> BTreeMap<String, Value> {
    BTreeMap::new()
}

pub fn fixture() -> Engine {
    let e = Engine::open_in_memory().unwrap();
    e.execute(FIXTURE, &no_params(), AccessMode::Write).unwrap();
    e
}

pub fn run(e: &Engine, q: &str) -> Result<QueryResult, QueryError> {
    e.execute(q, &no_params(), AccessMode::Write)
}

pub fn read(e: &Engine, q: &str) -> QueryResult {
    match e.execute(q, &no_params(), AccessMode::Read) {
        Ok(r) => r,
        Err(err) => panic!("query failed: {q}\n  {err}"),
    }
}

pub fn write(e: &Engine, q: &str) -> QueryResult {
    match e.execute(q, &no_params(), AccessMode::Write) {
        Ok(r) => r,
        Err(err) => panic!("query failed: {q}\n  {err}"),
    }
}

/// Rows rendered as compact JSON arrays, in result order.
pub fn render(r: &QueryResult) -> Vec<String> {
    r.rows
        .iter()
        .map(|row| {
            let cells: Vec<String> = row.iter().map(|v| v.to_json().to_string()).collect();
            format!("[{}]", cells.join(","))
        })
        .collect()
}

pub fn rows(e: &Engine, q: &str) -> Vec<String> {
    render(&read(e, q))
}

pub fn rows_sorted(e: &Engine, q: &str) -> Vec<String> {
    let mut v = rows(e, q);
    v.sort();
    v
}

#[track_caller]
pub fn check(e: &Engine, q: &str, want: &[&str]) {
    let got = rows(e, q);
    let want: Vec<String> = want.iter().map(|s| s.to_string()).collect();
    assert_eq!(got, want, "ordered result of: {q}");
}

#[track_caller]
pub fn check_set(e: &Engine, q: &str, want: &[&str]) {
    let got = rows_sorted(e, q);
    let mut want: Vec<String> = want.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(got, want, "unordered result of: {q}");
}

pub fn count_nodes(e: &Engine) -> i64 {
    match &read(e, "MATCH (n) RETURN count(n)").rows[0][0] {
        Value::Int(i) => *i,
        other => panic!("{other:?}"),
    }
}

pub fn count_rels(e: &Engine) -> i64 {
    match &read(e, "MATCH ()-[r]->() RETURN count(r)").rows[0][0] {
        Value::Int(i) => *i,
        other => panic!("{other:?}"),
    }
}
