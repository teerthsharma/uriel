//! TCK-style table-driven conformance cases: (setup, mode, query, expected rows).
//! `R` = ordered read, `S` = unordered read (sorted before comparing), `W` = ordered write.

mod common;
use common::*;
use uriel_query::Engine;

const S1: &str = "CREATE (a:A {name:'a', v:1})-[:T {w:1}]->(b:B {name:'b', v:2})-[:T {w:2}]->(c:C {name:'c', v:3}), (a)-[:U]->(c)";
const NONE: &str = "";

type Case = (&'static str, char, &'static str, &'static [&'static str]);

const CASES: &[Case] = &[
    // --- scalar expressions
    (NONE, 'R', "RETURN 1", &["[1]"]),
    (NONE, 'R', "RETURN 'x' AS s", &["[\"x\"]"]),
    (NONE, 'R', "RETURN 3 > 2 AND 2 > 1", &["[true]"]),
    (NONE, 'R', "RETURN [1,2,3] + 4", &["[[1,2,3,4]]"]),
    (NONE, 'R', "RETURN 4 + [1,2]", &["[[4,1,2]]"]),
    (
        NONE,
        'R',
        "RETURN size([x IN range(1,10) WHERE x % 3 = 0])",
        &["[3]"],
    ),
    (NONE, 'R', "RETURN {a: 1, b: {c: 2}}.b.c", &["[2]"]),
    (NONE, 'R', "RETURN [[1,2],[3,4]][1][0]", &["[3]"]),
    (NONE, 'R', "RETURN 5 % 3, -5 % 3, 5.5 % 2", &["[2,-2,1.5]"]),
    (
        NONE,
        'R',
        "RETURN toString(true), toString(null)",
        &["[\"true\",null]"],
    ),
    (
        NONE,
        'R',
        "RETURN toInteger(null), toFloat(null)",
        &["[null,null]"],
    ),
    (
        NONE,
        'R',
        "RETURN size(null), head([]), last([])",
        &["[null,null,null]"],
    ),
    (NONE, 'R', "RETURN range(3,1)", &["[[]]"]),
    (NONE, 'R', "RETURN substring('abc', 1, 0)", &["[\"\"]"]),
    (
        NONE,
        'R',
        "RETURN left('abc', 10), right('abc', 0)",
        &["[\"abc\",\"\"]"],
    ),
    (
        NONE,
        'R',
        "RETURN split('abc', '')",
        &["[[\"a\",\"b\",\"c\"]]"],
    ),
    (NONE, 'R', "RETURN replace('aaa','a','')", &["[\"\"]"]),
    (NONE, 'R', "RETURN reverse([])", &["[[]]"]),
    (NONE, 'R', "RETURN 1 IN [1.0]", &["[true]"]),
    (NONE, 'R', "RETURN 'a' < 'B', 'a' > 'B'", &["[false,true]"]),
    (
        NONE,
        'R',
        "RETURN true AND null, false AND null, true OR null, false OR null",
        &["[null,false,true,null]"],
    ),
    (NONE, 'R', "RETURN NOT true, NOT false", &["[false,true]"]),
    (NONE, 'R', "RETURN null IS NULL AND true", &["[true]"]),
    (NONE, 'R', "RETURN [null] = [null]", &["[null]"]),
    (NONE, 'R', "RETURN {} = {}, [] = []", &["[true,true]"]),
    (
        NONE,
        'R',
        "RETURN CASE null WHEN null THEN 1 ELSE 2 END",
        &["[2]"],
    ),
    (
        NONE,
        'R',
        "WITH 5 AS x RETURN CASE WHEN x < 3 THEN 'low' WHEN x < 10 THEN 'mid' ELSE 'high' END",
        &["[\"mid\"]"],
    ),
    (NONE, 'R', "RETURN 7 / 2, -7 / 2, 7 % -3", &["[3,-3,1]"]),
    (
        NONE,
        'R',
        "RETURN 1.5 + 1, 2 * 1.5, 3 - 0.5",
        &["[2.5,3.0,2.5]"],
    ),
    (NONE, 'R', "RETURN size('')", &["[0]"]),
    (
        NONE,
        'R',
        "RETURN trim(null), toUpper(null)",
        &["[null,null]"],
    ),
    (NONE, 'R', "RETURN [x IN [] | x]", &["[[]]"]),
    (
        NONE,
        'R',
        "RETURN [x IN [1,null,3] WHERE x IS NOT NULL | x]",
        &["[[1,3]]"],
    ),
    (
        NONE,
        'R',
        "RETURN reduce(acc = '', s IN ['a','b','c'] | acc + s)",
        &["[\"abc\"]"],
    ),
    (
        NONE,
        'R',
        "RETURN any(x IN [] WHERE x > 1), all(x IN [] WHERE x > 1)",
        &["[false,true]"],
    ),
    (
        NONE,
        'R',
        "RETURN 1 = 1.0, 'a' = 'a', true = true, null = 1",
        &["[true,true,true,null]"],
    ),
    // --- unwind / with / ordering
    (
        NONE,
        'R',
        "UNWIND range(1,5) AS x RETURN sum(x), avg(x), min(x), max(x), count(x)",
        &["[15,3.0,1,5,5]"],
    ),
    (
        NONE,
        'R',
        "UNWIND range(1,10) AS x WITH x WHERE x % 2 = 0 RETURN collect(x)",
        &["[[2,4,6,8,10]]"],
    ),
    (
        NONE,
        'R',
        "UNWIND [1,2,3] AS x UNWIND ['a','b'] AS y RETURN count(*)",
        &["[6]"],
    ),
    (
        NONE,
        'R',
        "UNWIND [1,2,3] AS x RETURN x ORDER BY x DESC LIMIT 2",
        &["[3]", "[2]"],
    ),
    (
        NONE,
        'R',
        "UNWIND [1,2,3,4,5] AS x RETURN x SKIP 1 LIMIT 3",
        &["[2]", "[3]", "[4]"],
    ),
    (
        NONE,
        'R',
        "UNWIND [3,1,2] AS x WITH x ORDER BY x RETURN collect(x)",
        &["[[1,2,3]]"],
    ),
    (
        NONE,
        'R',
        "UNWIND ['a','b','a','c','a'] AS x RETURN x, count(*) AS c ORDER BY c DESC, x",
        &["[\"a\",3]", "[\"b\",1]", "[\"c\",1]"],
    ),
    (
        NONE,
        'S',
        "UNWIND [1,2,3] AS x RETURN DISTINCT x % 2",
        &["[0]", "[1]"],
    ),
    (
        NONE,
        'R',
        "WITH [1,2,3] AS l RETURN l[1], l[-1], size(l)",
        &["[2,3,3]"],
    ),
    (
        NONE,
        'R',
        "WITH 'abc' AS s RETURN s STARTS WITH 'a' AND s ENDS WITH 'c' AND s CONTAINS 'b'",
        &["[true]"],
    ),
    (
        NONE,
        'R',
        "UNWIND [1,2,3] AS x WITH x AS y, x * 2 AS z WHERE z > 2 RETURN y, z",
        &["[2,4]", "[3,6]"],
    ),
    (
        NONE,
        'R',
        "UNWIND [[1,2],[3,4]] AS p RETURN p[0] + p[1] AS s ORDER BY s",
        &["[3]", "[7]"],
    ),
    (
        NONE,
        'R',
        "UNWIND [{a:1},{a:2}] AS m RETURN sum(m.a)",
        &["[3]"],
    ),
    // --- read patterns over S1
    (S1, 'R', "MATCH (n) RETURN count(n)", &["[3]"]),
    (
        S1,
        'R',
        "MATCH (a:A)-[r:T]->(b) RETURN r.w, b.name",
        &["[1,\"b\"]"],
    ),
    (
        S1,
        'R',
        "MATCH (a:A)-[*]->(x) RETURN x.name ORDER BY x.name",
        &["[\"b\"]", "[\"c\"]", "[\"c\"]"],
    ),
    (
        S1,
        'R',
        "MATCH (a:A)-[:T*1..2]->(x) RETURN x.name ORDER BY x.name",
        &["[\"b\"]", "[\"c\"]"],
    ),
    (
        S1,
        'R',
        "MATCH (a:A)-->(x) RETURN x.name ORDER BY x.name",
        &["[\"b\"]", "[\"c\"]"],
    ),
    (S1, 'R', "MATCH (a:A)--(x) RETURN count(*)", &["[2]"]),
    (
        S1,
        'R',
        "MATCH (c:C)<--(x) RETURN x.name ORDER BY x.name",
        &["[\"a\"]", "[\"b\"]"],
    ),
    (
        S1,
        'R',
        "MATCH (n) WHERE n.v IN [1,3] RETURN n.name ORDER BY n.name",
        &["[\"a\"]", "[\"c\"]"],
    ),
    (
        S1,
        'R',
        "MATCH (n) WHERE n:A OR n:B RETURN count(n)",
        &["[2]"],
    ),
    (S1, 'R', "MATCH (n) WHERE NOT n:A RETURN count(n)", &["[2]"]),
    (
        S1,
        'R',
        "MATCH (a)-[r]->(b) RETURN type(r), count(*) ORDER BY type(r)",
        &["[\"T\",2]", "[\"U\",1]"],
    ),
    (
        S1,
        'R',
        "MATCH (a:A) OPTIONAL MATCH (a)-[:NONE]->(x) RETURN x",
        &["[null]"],
    ),
    (
        S1,
        'R',
        "MATCH (n) RETURN labels(n)[0] AS l ORDER BY l",
        &["[\"A\"]", "[\"B\"]", "[\"C\"]"],
    ),
    (
        S1,
        'R',
        "MATCH (n) WITH n.v AS v ORDER BY v DESC RETURN collect(v)",
        &["[[3,2,1]]"],
    ),
    (
        S1,
        'R',
        "MATCH (a:A)-[r:T]->(b)-[s:T]->(c) RETURN r.w + s.w",
        &["[3]"],
    ),
    (
        S1,
        'R',
        "MATCH p=(a:A)-[:T]->(b) RETURN nodes(p)[0].name, relationships(p)[0].w",
        &["[\"a\",1]"],
    ),
    (
        S1,
        'R',
        "MATCH (n) RETURN n.missing IS NULL, count(*)",
        &["[true,3]"],
    ),
    (
        S1,
        'R',
        "MATCH (a:A), (b:B) RETURN a.name + b.name",
        &["[\"ab\"]"],
    ),
    (
        S1,
        'R',
        "MATCH (a:A) MATCH (a)-[:T]->(b) RETURN b.name",
        &["[\"b\"]"],
    ),
    (
        S1,
        'R',
        "MATCH (x) WITH x.v AS v WHERE v > 1 RETURN v ORDER BY v",
        &["[2]", "[3]"],
    ),
    (
        S1,
        'R',
        "MATCH (a)-[r:T]->(b) RETURN a.name AS src, b.name AS dst ORDER BY src",
        &["[\"a\",\"b\"]", "[\"b\",\"c\"]"],
    ),
    (
        S1,
        'R',
        "MATCH (n) RETURN count(DISTINCT labels(n))",
        &["[3]"],
    ),
    (
        S1,
        'R',
        "MATCH (n) RETURN sum(n.v) AS s, avg(n.v) AS a",
        &["[6,2.0]"],
    ),
    (S1, 'R', "MATCH (a:A)-[r]-(b) RETURN count(r)", &["[2]"]),
    (
        "CREATE (x:L {n:1})-[:S]->(x)",
        'R',
        "MATCH (x:L)-[r:S]-(y) RETURN count(r)",
        &["[1]"],
    ),
    (
        "CREATE (x:L {n:1})-[:S]->(x)",
        'R',
        "MATCH (x:L)-[:S]->(y) RETURN x = y",
        &["[true]"],
    ),
    (
        "CREATE (:A:B {k:1})",
        'R',
        "MATCH (n:A:B) RETURN n.k",
        &["[1]"],
    ),
    (
        "CREATE (:A:B {k:1})",
        'R',
        "MATCH (n:B) RETURN labels(n)",
        &["[[\"A\",\"B\"]]"],
    ),
    (
        "CREATE (:A:B {k:1}), (:A {k:2})",
        'R',
        "MATCH (n:A:B) RETURN count(n)",
        &["[1]"],
    ),
    // --- write statements
    (
        NONE,
        'W',
        "CREATE (a:P {n:1}), (b:P {n:2}) RETURN count(*)",
        &["[1]"],
    ),
    (
        NONE,
        'W',
        "MERGE (n:Q {k:1}) MERGE (m:Q {k:1}) RETURN id(n) = id(m)",
        &["[true]"],
    ),
    (
        NONE,
        'W',
        "UNWIND [1,2,3] AS x CREATE (:R {v:x}) WITH count(*) AS c RETURN c",
        &["[3]"],
    ),
    (
        NONE,
        'W',
        "CREATE (a:S {n:1}) SET a.n = a.n + 1 RETURN a.n",
        &["[2]"],
    ),
    (
        NONE,
        'W',
        "CREATE (a:S {n:1}) REMOVE a.n RETURN a.n",
        &["[null]"],
    ),
    (
        NONE,
        'W',
        "CREATE (a:S)-[r:T {w:5}]->(b:S) RETURN r.w",
        &["[5]"],
    ),
    (NONE, 'W', "CREATE (a:S) DELETE a RETURN 1", &["[1]"]),
    (
        S1,
        'W',
        "MATCH (a:A) SET a.v = a.v * 10 RETURN a.v",
        &["[10]"],
    ),
    (
        S1,
        'W',
        "MATCH (a:A)-[r:T]->() SET r.w = 100 RETURN r.w",
        &["[100]"],
    ),
    (
        S1,
        'W',
        "MATCH (n) WHERE n.v > 1 DETACH DELETE n RETURN count(*)",
        &["[2]"],
    ),
    (
        S1,
        'W',
        "MATCH (a:A) CREATE (a)-[:NEW]->(x:N {v:9}) RETURN x.v",
        &["[9]"],
    ),
    (
        S1,
        'W',
        "MATCH (a:A) MERGE (a)-[:T]->(b:B) RETURN b.name",
        &["[\"b\"]"],
    ),
    (
        S1,
        'W',
        "MATCH (a:A) MERGE (a)-[:V]->(b:B2 {name:'new'}) ON CREATE SET b.flag = true RETURN b.flag",
        &["[true]"],
    ),
    (
        NONE,
        'W',
        "CREATE (a:Z {l: [1,2,3]}) RETURN a.l",
        &["[[1,2,3]]"],
    ),
    (
        NONE,
        'W',
        "CREATE (a:Z {m: 1}) SET a += {x: 2, y: 3} RETURN keys(a)",
        &["[[\"m\",\"x\",\"y\"]]"],
    ),
    (
        NONE,
        'W',
        "CREATE (a:Z {m: 1}) SET a = {x: 2} RETURN keys(a)",
        &["[[\"x\"]]"],
    ),
    (
        NONE,
        'W',
        "CREATE (a:Z) SET a:Y:W RETURN labels(a)",
        &["[[\"W\",\"Y\",\"Z\"]]"],
    ),
    (
        NONE,
        'W',
        "CREATE (a:Z:Y) REMOVE a:Y RETURN labels(a)",
        &["[[\"Z\"]]"],
    ),
    (
        S1,
        'W',
        "MATCH (a:A) WITH a CREATE (a)-[:X]->(:Y) WITH a MATCH (a)-[:X]->(y) RETURN count(y)",
        &["[1]"],
    ),
    (
        S1,
        'W',
        "MATCH (n) WITH collect(n) AS ns UNWIND ns AS m RETURN count(m)",
        &["[3]"],
    ),
    (NONE, 'R', "RETURN 10 / 4, 10 / 4.0, 10 % 4", &["[2,2.5,2]"]),
    (
        NONE,
        'R',
        "RETURN [1,2,3][1..2], [1,2,3][-2..]",
        &["[[2],[2,3]]"],
    ),
    (
        NONE,
        'R',
        "RETURN toInteger('  7 '), toFloat('x')",
        &["[7,null]"],
    ),
    (
        S1,
        'R',
        "MATCH (n:A) RETURN n.name, n.v + 1",
        &["[\"a\",2]"],
    ),
    (
        S1,
        'R',
        "MATCH (a:A)-[:T*2]->(c) RETURN c.name",
        &["[\"c\"]"],
    ),
    (
        S1,
        'W',
        "MATCH (n:B) SET n.name = 'bb' RETURN n.name",
        &["[\"bb\"]"],
    ),
    (
        S1,
        'W',
        "MATCH (n:B) REMOVE n:B SET n:BB RETURN labels(n)",
        &["[[\"BB\"]]"],
    ),
];

#[test]
fn tck_table() {
    let mut failures: Vec<String> = Vec::new();
    for (i, (setup, mode, query, want)) in CASES.iter().enumerate() {
        let e = Engine::open_in_memory(uriel_core::MemoryBudget::mib(80)).unwrap();
        if !setup.is_empty() {
            write(&e, setup);
        }
        let got = match mode {
            'W' => run(&e, query).map(|r| render(&r)),
            _ => e
                .execute(query, &no_params(), uriel_query::AccessMode::Read)
                .map(|r| render(&r)),
        };
        let mut got = match got {
            Ok(g) => g,
            Err(err) => {
                failures.push(format!("#{i} {query}\n    error: {err}"));
                continue;
            }
        };
        let mut want: Vec<String> = want.iter().map(|s| s.to_string()).collect();
        if *mode == 'S' {
            got.sort();
            want.sort();
        }
        if got != want {
            failures.push(format!("#{i} {query}\n    got  {got:?}\n    want {want:?}"));
        }
    }
    assert!(CASES.len() >= 100, "table has {} cases", CASES.len());
    assert!(
        failures.is_empty(),
        "{} of {} cases failed:\n{}",
        failures.len(),
        CASES.len(),
        failures.join("\n")
    );
}
