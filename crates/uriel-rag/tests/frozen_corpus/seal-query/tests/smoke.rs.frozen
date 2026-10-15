mod common;
use common::*;

#[test]
fn smoke_fixture_counts() {
    let e = fixture();
    assert_eq!(count_nodes(&e), 9);
    assert_eq!(count_rels(&e), 10);
    check(&e, "MATCH (p:Person) RETURN count(*) AS c", &["[6]"]);
    check(
        &e,
        "MATCH (a:Person {name:'Alice'})-[:KNOWS]->(b) RETURN b.name ORDER BY b.name",
        &["[\"Bob\"]", "[\"Carol\"]"],
    );
}
