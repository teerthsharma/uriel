mod common;
use common::*;

#[test]
fn match_filters_and_nulls() {
    let e = fixture();
    check(
        &e,
        "MATCH (n:Movie) RETURN n.title ORDER BY n.title",
        &["[\"Dune\"]", "[\"Inception\"]", "[\"The Matrix\"]"],
    );
    check(
        &e,
        "MATCH (n:Person) WHERE n.age > 28 RETURN n.name ORDER BY n.name",
        &["[\"Alice\"]", "[\"Carol\"]", "[\"Eve\"]"],
    );
    check(
        &e,
        "MATCH (n:Person) WHERE n.age >= 28 AND n.city = 'London' RETURN n.name ORDER BY n.name",
        &["[\"Alice\"]", "[\"Carol\"]"],
    );
    check(
        &e,
        "MATCH (n:Person) WHERE n.age < 30 OR n.city = 'London' RETURN n.name ORDER BY n.name",
        &["[\"Alice\"]", "[\"Bob\"]", "[\"Carol\"]", "[\"Dave\"]"],
    );
    check(
        &e,
        "MATCH (n:Person) WHERE NOT n.age > 28 RETURN n.name ORDER BY n.name",
        &["[\"Bob\"]", "[\"Dave\"]"],
    );
    check(
        &e,
        "MATCH (n:Person) WHERE n.age IS NULL RETURN n.name",
        &["[\"Frank\"]"],
    );
    check(
        &e,
        "MATCH (n:Person) WHERE n.city IS NULL RETURN n.name ORDER BY n.name",
        &["[\"Eve\"]", "[\"Frank\"]"],
    );
    check(
        &e,
        "MATCH (n:Person) WHERE n.city IS NOT NULL RETURN count(*)",
        &["[4]"],
    );
    check(
        &e,
        "MATCH (n:Person {city:'London'}) RETURN n.name ORDER BY n.name",
        &["[\"Alice\"]", "[\"Carol\"]"],
    );
    check(
        &e,
        "MATCH (n:Person) WHERE n.city = 'Paris' XOR n.age > 33 RETURN n.name ORDER BY n.name",
        &["[\"Bob\"]", "[\"Carol\"]"],
    );
    check(
        &e,
        "MATCH (n:Person) WHERE n.name IN ['Alice','Zed','Eve'] RETURN n.name ORDER BY n.name",
        &["[\"Alice\"]", "[\"Eve\"]"],
    );
    check(&e, "MATCH (n:Person) WHERE n.name STARTS WITH 'A' OR n.name ENDS WITH 'e' RETURN n.name ORDER BY n.name", &["[\"Alice\"]", "[\"Dave\"]", "[\"Eve\"]"]);
    check(
        &e,
        "MATCH (n:Person) WHERE n.name =~ '[AB].*' RETURN n.name ORDER BY n.name",
        &["[\"Alice\"]", "[\"Bob\"]"],
    );
    check(
        &e,
        "MATCH (n:Person) WHERE n.name CONTAINS 'a' RETURN n.name ORDER BY n.name",
        &["[\"Carol\"]", "[\"Dave\"]", "[\"Frank\"]"],
    );
}

#[test]
fn match_patterns() {
    let e = fixture();
    check(&e, "MATCH (a:Person)-[:KNOWS]->(b)-[:KNOWS]->(c) RETURN a.name, c.name ORDER BY a.name, c.name",
        &["[\"Alice\",\"Carol\"]", "[\"Alice\",\"Dave\"]", "[\"Bob\",\"Dave\"]", "[\"Carol\",\"Eve\"]"]);
    check(
        &e,
        "MATCH (m:Movie {title:'The Matrix'})<-[:ACTED_IN]-(p) RETURN p.name ORDER BY p.name",
        &["[\"Alice\"]", "[\"Bob\"]"],
    );
    check(
        &e,
        "MATCH (a:Person {name:'Carol'})-[:KNOWS]-(b) RETURN b.name ORDER BY b.name",
        &["[\"Alice\"]", "[\"Bob\"]", "[\"Dave\"]"],
    );
    check(&e, "MATCH (a)-[r:KNOWS]->(b) WHERE r.since > 2017 RETURN a.name, b.name, r.since ORDER BY r.since",
        &["[\"Bob\",\"Carol\",2018]", "[\"Carol\",\"Dave\",2020]", "[\"Dave\",\"Eve\",2021]"]);
    check(
        &e,
        "MATCH (a)-[r:KNOWS {since:2015}]->(b) RETURN a.name, b.name",
        &["[\"Alice\",\"Bob\"]"],
    );
    check(&e, "MATCH (a:Person {name:'Bob'})-[r:KNOWS|ACTED_IN]->(x) RETURN type(r), count(*) ORDER BY type(r)", &["[\"ACTED_IN\",2]", "[\"KNOWS\",1]"]);
    check(
        &e,
        "MATCH (a:Person {name:'Dave'})-[r]->(x) RETURN type(r) ORDER BY type(r)",
        &["[\"DIRECTED\"]", "[\"KNOWS\"]"],
    );
    check(
        &e,
        "MATCH (a:Person {name:'Alice'}), (m:Movie {title:'Dune'}) RETURN a.name, m.title",
        &["[\"Alice\",\"Dune\"]"],
    );
    check(&e, "MATCH (a:Person)-[:ACTED_IN]->(m)<-[:ACTED_IN]-(b) WHERE a.name < b.name RETURN a.name, b.name, m.title", &["[\"Alice\",\"Bob\",\"The Matrix\"]"]);
    // relationship uniqueness: the second hop may not reuse the first relationship
    check(&e, "MATCH (a)-[r1:KNOWS]-(b)-[r2:KNOWS]-(c) WHERE a.name='Alice' AND b.name='Bob' RETURN c.name", &["[\"Carol\"]"]);
    check(
        &e,
        "MATCH (n:Person)-[:KNOWS]->(:Person {name:'Carol'}) RETURN n.name ORDER BY n.name",
        &["[\"Alice\"]", "[\"Bob\"]"],
    );
    check(
        &e,
        "MATCH (n) WHERE n.year = 2010 RETURN n.title",
        &["[\"Inception\"]"],
    );
    check(&e, "MATCH (a:Person {name:'Alice'})-[:KNOWS]->(b)-[:ACTED_IN]->(m) RETURN b.name, m.title ORDER BY b.name, m.title",
        &["[\"Bob\",\"Inception\"]", "[\"Bob\",\"The Matrix\"]", "[\"Carol\",\"Dune\"]"]);
    check(
        &e,
        "MATCH (n:Person) WHERE (n)-[:DIRECTED]->() RETURN n.name",
        &["[\"Dave\"]"],
    );
    check(
        &e,
        "MATCH (n:Person) WHERE NOT (n)-[:KNOWS]->() RETURN n.name ORDER BY n.name",
        &["[\"Eve\"]", "[\"Frank\"]"],
    );
    check(
        &e,
        "MATCH (p:Person) WHERE EXISTS { (p)-[:ACTED_IN]->() } RETURN p.name ORDER BY p.name",
        &["[\"Alice\"]", "[\"Bob\"]", "[\"Carol\"]"],
    );
    check(
        &e,
        "MATCH (p:Person) WHERE exists(p.city) RETURN count(*)",
        &["[4]"],
    );
}

#[test]
fn optional_match() {
    let e = fixture();
    check(&e, "MATCH (p:Person) OPTIONAL MATCH (p)-[:ACTED_IN]->(m) RETURN p.name, m.title ORDER BY p.name, m.title",
        &["[\"Alice\",\"The Matrix\"]", "[\"Bob\",\"Inception\"]", "[\"Bob\",\"The Matrix\"]", "[\"Carol\",\"Dune\"]",
          "[\"Dave\",null]", "[\"Eve\",null]", "[\"Frank\",null]"]);
    check(&e, "MATCH (p:Person {name:'Bob'}) OPTIONAL MATCH (p)-[:ACTED_IN]->(m) WHERE m.year > 2000 RETURN p.name, m.title", &["[\"Bob\",\"Inception\"]"]);
    check(
        &e,
        "MATCH (p:Person {name:'Frank'}) OPTIONAL MATCH (p)-[:KNOWS]->(x) RETURN p.name, x",
        &["[\"Frank\",null]"],
    );
    check(&e, "MATCH (p:Person) OPTIONAL MATCH (p)-[:KNOWS]->(x) RETURN p.name, count(x) AS c ORDER BY c DESC, p.name",
        &["[\"Alice\",2]", "[\"Bob\",1]", "[\"Carol\",1]", "[\"Dave\",1]", "[\"Eve\",0]", "[\"Frank\",0]"]);
    check(&e, "OPTIONAL MATCH (x:Nothing) RETURN x", &["[null]"]);
}

#[test]
fn variable_length_paths() {
    let e = fixture();
    // chains from Alice: len1 = 2, len2 = 2, len3 = 2, len4 = 1
    check(
        &e,
        "MATCH (a:Person {name:'Alice'})-[:KNOWS*1..4]->(x) RETURN count(*)",
        &["[7]"],
    );
    check(
        &e,
        "MATCH (a:Person {name:'Alice'})-[:KNOWS*]->(x) RETURN count(*)",
        &["[7]"],
    );
    check(
        &e,
        "MATCH (a:Person {name:'Alice'})-[:KNOWS*2]->(x) RETURN count(*)",
        &["[2]"],
    );
    check(
        &e,
        "MATCH (a:Person {name:'Alice'})-[:KNOWS*..2]->(x) RETURN count(*)",
        &["[4]"],
    );
    check(
        &e,
        "MATCH (a:Person {name:'Alice'})-[:KNOWS*3..]->(x) RETURN count(*)",
        &["[3]"],
    );
    check(
        &e,
        "MATCH (a:Person {name:'Alice'})-[:KNOWS*0..1]->(x) RETURN count(*)",
        &["[3]"],
    );
    check(
        &e,
        "MATCH (a:Person {name:'Alice'})-[:KNOWS*2]-(x) RETURN count(*)",
        &["[3]"],
    );
    check(&e, "MATCH p=(a:Person {name:'Alice'})-[:KNOWS*]->(e:Person {name:'Eve'}) RETURN length(p) ORDER BY length(p)", &["[3]", "[4]"]);
    check(&e, "MATCH p=(a:Person {name:'Alice'})-[:KNOWS*3]->(x) RETURN [n IN nodes(p) | n.name] AS names ORDER BY x.name",
        &["[[\"Alice\",\"Bob\",\"Carol\",\"Dave\"]]", "[[\"Alice\",\"Carol\",\"Dave\",\"Eve\"]]"]);
    check(&e, "MATCH (a:Person {name:'Alice'})-[r:KNOWS*2]->(x:Person {name:'Dave'}) RETURN size(r), [x IN r | x.since] AS s", &["[2,[2010,2020]]"]);
    check(
        &e,
        "MATCH (a:Person {name:'Eve'})-[:KNOWS*]->(x) RETURN count(*)",
        &["[0]"],
    );
    check(
        &e,
        "MATCH (a:Person {name:'Eve'})<-[:KNOWS*1..2]-(x) RETURN x.name ORDER BY x.name",
        &["[\"Carol\"]", "[\"Dave\"]"],
    );
}

#[test]
fn shortest_paths() {
    let e = fixture();
    check(&e, "MATCH p=shortestPath((a:Person {name:'Alice'})-[:KNOWS*]->(e:Person {name:'Eve'})) RETURN length(p)", &["[3]"]);
    check(&e, "MATCH p=allShortestPaths((a:Person {name:'Alice'})-[:KNOWS*]->(d:Person {name:'Dave'})) RETURN length(p), [n IN nodes(p) | n.name]",
        &["[2,[\"Alice\",\"Carol\",\"Dave\"]]"]);
    check(&e, "MATCH p=shortestPath((a:Person {name:'Bob'})-[:KNOWS*]-(d:Person {name:'Dave'})) RETURN length(p)", &["[2]"]);
    check(&e, "MATCH p=shortestPath((a:Person {name:'Eve'})-[:KNOWS*]->(d:Person {name:'Alice'})) RETURN p", &[]);
    check(&e, "MATCH p=shortestPath((a:Person {name:'Alice'})-[*]-(m:Movie {title:'Dune'})) RETURN length(p)", &["[2]"]);
    write(&e, "CREATE (a:D {n:'a'})-[:R]->(b:D {n:'b'})-[:R]->(d:D {n:'d'}), (a)-[:R]->(c:D {n:'c'})-[:R]->(d)");
    check(
        &e,
        "MATCH p=allShortestPaths((a:D {n:'a'})-[:R*]->(d:D {n:'d'})) RETURN count(p)",
        &["[2]"],
    );
    check(
        &e,
        "MATCH p=shortestPath((a:D {n:'a'})-[:R*]->(d:D {n:'d'})) RETURN count(p)",
        &["[1]"],
    );
}

#[test]
fn return_order_skip_limit_distinct() {
    let e = fixture();
    check(
        &e,
        "MATCH (n:Person) RETURN n.name ORDER BY n.age DESC",
        &[
            "[\"Frank\"]",
            "[\"Eve\"]",
            "[\"Carol\"]",
            "[\"Alice\"]",
            "[\"Dave\"]",
            "[\"Bob\"]",
        ],
    );
    check(
        &e,
        "MATCH (n:Person) RETURN n.name ORDER BY n.age",
        &[
            "[\"Bob\"]",
            "[\"Dave\"]",
            "[\"Alice\"]",
            "[\"Carol\"]",
            "[\"Eve\"]",
            "[\"Frank\"]",
        ],
    );
    check(
        &e,
        "MATCH (n:Person) RETURN n.name ORDER BY n.name SKIP 1 LIMIT 2",
        &["[\"Bob\"]", "[\"Carol\"]"],
    );
    check(
        &e,
        "MATCH (n:Person) RETURN DISTINCT n.city ORDER BY n.city",
        &["[\"Berlin\"]", "[\"London\"]", "[\"Paris\"]", "[null]"],
    );
    check(
        &e,
        "MATCH (n:Person) RETURN n.name ORDER BY n.city, n.name DESC",
        &[
            "[\"Dave\"]",
            "[\"Carol\"]",
            "[\"Alice\"]",
            "[\"Bob\"]",
            "[\"Frank\"]",
            "[\"Eve\"]",
        ],
    );
    check(
        &e,
        "MATCH (n:Person) RETURN n.name AS who ORDER BY who LIMIT 1",
        &["[\"Alice\"]"],
    );
    check(
        &e,
        "MATCH (n:Person) RETURN n.name ORDER BY n.name SKIP 10",
        &[],
    );
    check(
        &e,
        "MATCH (n:Person) RETURN n.name ORDER BY n.name LIMIT 0",
        &[],
    );
    let r = read(
        &e,
        "MATCH (a:Person {name:'Alice'}) RETURN a.name AS who, a.age, 1 + 1",
    );
    assert_eq!(r.columns, vec!["who", "a.age", "1 + 1"]);
    let r = read(&e, "MATCH (a:Person {name:'Alice'}) RETURN *");
    assert_eq!(r.columns, vec!["a"]);
    check(
        &e,
        "MATCH (a:Person {name:'Alice'}) RETURN a.name, a.city ORDER BY a.age",
        &["[\"Alice\",\"London\"]"],
    );
}

#[test]
fn aggregation() {
    let e = fixture();
    check(&e, "MATCH (n:Person) RETURN count(*)", &["[6]"]);
    check(&e, "MATCH (n:Person) RETURN count(n.age)", &["[5]"]);
    check(
        &e,
        "MATCH (n:Person) RETURN count(DISTINCT n.city)",
        &["[3]"],
    );
    check(&e, "MATCH (n:Person) RETURN sum(n.age)", &["[158]"]);
    check(&e, "MATCH (n:Person) RETURN avg(n.age)", &["[31.6]"]);
    check(
        &e,
        "MATCH (n:Person) RETURN min(n.age), max(n.age)",
        &["[25,40]"],
    );
    check(
        &e,
        "MATCH (n:Person) RETURN round(stDev(n.age), 2)",
        &["[5.94]"],
    );
    check(
        &e,
        "MATCH (n:Person) WITH n ORDER BY n.name RETURN collect(n.name)",
        &["[[\"Alice\",\"Bob\",\"Carol\",\"Dave\",\"Eve\",\"Frank\"]]"],
    );
    check(
        &e,
        "MATCH (n:Person) RETURN n.city AS city, count(*) AS c ORDER BY c DESC, city",
        &[
            "[\"London\",2]",
            "[null,2]",
            "[\"Berlin\",1]",
            "[\"Paris\",1]",
        ],
    );
    check(
        &e,
        "MATCH (n:Person) RETURN n.city AS city, count(*) * 10 AS c ORDER BY city LIMIT 1",
        &["[\"Berlin\",10]"],
    );
    check(
        &e,
        "MATCH (n:Nothing) RETURN count(*), sum(n.x), max(n.x), collect(n.x)",
        &["[0,0,null,[]]"],
    );
    check(&e, "MATCH (n:Nothing) RETURN n.x, count(*)", &[]);
    check(
        &e,
        "MATCH (p:Person)-[:KNOWS]->(f) WITH p, count(f) AS c WHERE c > 1 RETURN p.name, c",
        &["[\"Alice\",2]"],
    );
    check(
        &e,
        "MATCH (n:Person) RETURN size(collect(DISTINCT n.city))",
        &["[3]"],
    );
    check(
        &e,
        "UNWIND [1,2,3,4] AS x RETURN avg(x), sum(x)",
        &["[2.5,10]"],
    );
    check(&e, "UNWIND [1, 2.5] AS x RETURN sum(x)", &["[3.5]"]);
    check(
        &e,
        "UNWIND ['b','a','c'] AS x RETURN min(x), max(x)",
        &["[\"a\",\"c\"]"],
    );
    check(
        &e,
        "UNWIND [1, 1.0, 2] AS x RETURN count(DISTINCT x)",
        &["[2]"],
    );
    check(
        &e,
        "UNWIND [1, null, 3] AS x RETURN count(x), count(*), collect(x)",
        &["[2,3,[1,3]]"],
    );
    check(
        &e,
        "MATCH (n:Person) RETURN sum(n.age) / count(n.age) AS m",
        &["[31]"],
    );
    check(&e, "MATCH (m:Movie)<-[:ACTED_IN]-(p) RETURN m.title, count(p) AS actors, collect(p.name) AS names ORDER BY actors DESC, m.title LIMIT 1",
        &["[\"The Matrix\",2,[\"Alice\",\"Bob\"]]"]);
    check(
        &e,
        "MATCH (n:Person) RETURN percentileDisc(n.age, 0.5), percentileCont(n.age, 0.5)",
        &["[30.0,30.0]"],
    );
}

#[test]
fn expressions_scalar() {
    let e = fixture();
    check(
        &e,
        "RETURN 1 + 2 * 3, 10 / 3, 10 % 3, 2 ^ 10, -5 + 2, 10 / 4.0, 7 / 2",
        &["[7,3,1,1024.0,-3,2.5,3]"],
    );
    check(
        &e,
        "RETURN 'a' + 'b', 'n' + 1, 1 + 'n'",
        &["[\"ab\",\"n1\",\"1n\"]"],
    );
    check(&e, "RETURN 'hello' STARTS WITH 'he', 'hello' ENDS WITH 'lo', 'hello' CONTAINS 'ell', 'hello' CONTAINS 'xyz'", &["[true,true,true,false]"]);
    check(
        &e,
        "RETURN null STARTS WITH 'a', 'a' CONTAINS null",
        &["[null,null]"],
    );
    check(
        &e,
        "RETURN 'hello' =~ 'h.*o', 'hello' =~ 'h', 'Hello' =~ '(?i)hello'",
        &["[true,false,true]"],
    );
    check(&e, "RETURN 2 IN [1,2,3], 5 IN [1,2,3], null IN [1,2], 1 IN [null,2], 2 IN [null,2], 1 IN [], null IN []", &["[true,false,null,null,true,false,false]"]);
    check(&e, "RETURN [1,2,3][0], [1,2,3][-1], [1,2,3][1..], [1,2,3][..2], [1,2,3][5], size([1,2,3]), [1,2]+[3]", &["[1,3,[2,3],[1,2],null,3,[1,2,3]]"]);
    check(&e, "RETURN head([1,2]), last([1,2]), tail([1,2,3]), range(1,5), range(0,10,5), reverse([1,2,3]), range(5,1,-2)", &["[1,2,[2,3],[1,2,3,4,5],[0,5,10],[3,2,1],[5,3,1]]"]);
    check(
        &e,
        "RETURN {a:1,b:2}.a, {a:1}['a'], keys({b:1,a:2})",
        &["[1,1,[\"a\",\"b\"]]"],
    );
    check(&e, "RETURN CASE WHEN 1>2 THEN 'x' WHEN 2>1 THEN 'y' ELSE 'z' END, CASE 3 WHEN 1 THEN 'a' WHEN 3 THEN 'c' END, CASE 9 WHEN 1 THEN 'a' END", &["[\"y\",\"c\",null]"]);
    check(
        &e,
        "RETURN [x IN range(1,5) WHERE x % 2 = 1 | x * x]",
        &["[[1,9,25]]"],
    );
    check(
        &e,
        "RETURN [x IN [1,2,3] | x + 1], [x IN [1,2,3] WHERE x > 1]",
        &["[[2,3,4],[2,3]]"],
    );
    check(&e, "RETURN all(x IN [1,2,3] WHERE x > 0), any(x IN [1,2,3] WHERE x > 2), none(x IN [1,2] WHERE x > 5), single(x IN [1,2,3] WHERE x = 2)", &["[true,true,true,true]"]);
    check(&e, "RETURN all(x IN [1,null] WHERE x > 0), any(x IN [1,null] WHERE x > 0), any(x IN [0,null] WHERE x > 0)", &["[null,true,null]"]);
    check(&e, "RETURN reduce(s = 0, x IN [1,2,3] | s + x)", &["[6]"]);
    check(
        &e,
        "RETURN coalesce(null, 2, 3), coalesce(null, null)",
        &["[2,null]"],
    );
    check(&e, "RETURN toInteger('42'), toInteger(3.9), toInteger('x'), toFloat('1.5'), toFloat(2), toString(12), toString(1.0), toBoolean('true'), toBoolean('x')",
        &["[42,3,null,1.5,2.0,\"12\",\"1.0\",true,null]"]);
    check(&e, "RETURN toLower('ABC'), toUpper('abc'), trim('  a '), ltrim('  a'), rtrim('a  '), replace('banana','a','o'), split('a,b,c', ',')",
        &["[\"abc\",\"ABC\",\"a\",\"a\",\"a\",\"bonono\",[\"a\",\"b\",\"c\"]]"]);
    check(&e, "RETURN substring('hello',1,3), substring('hello',2), left('hello',2), right('hello',2), reverse('abc'), size('héllo')",
        &["[\"ell\",\"llo\",\"he\",\"lo\",\"cba\",5]"]);
    check(&e, "RETURN abs(-3), abs(-2.5), ceil(1.2), floor(1.8), round(2.5), round(-2.5), sqrt(16), sign(-7), sign(0.0)", &["[3,2.5,2.0,1.0,3.0,-3.0,4.0,-1,0]"]);
    check(
        &e,
        "RETURN exp(0), log(1), log10(100), sin(0), cos(0), tan(0), pi() > 3.14, e() > 2.7",
        &["[1.0,0.0,2.0,0.0,1.0,0.0,true,true]"],
    );
    check(
        &e,
        "RETURN timestamp() > 0, rand() >= 0 AND rand() < 1",
        &["[true,true]"],
    );
    check(
        &e,
        "RETURN isEmpty([]), isEmpty([1]), isEmpty('')",
        &["[true,false,true]"],
    );
    check(
        &e,
        "RETURN round(3.14159, 3), 5 / 2.0, 2 ^ 0.5 > 1.41",
        &["[3.142,2.5,true]"],
    );
}

#[test]
fn three_valued_logic() {
    let e = fixture();
    check(
        &e,
        "RETURN null + 1, null = null, null <> 1, 1 = null",
        &["[null,null,null,null]"],
    );
    check(
        &e,
        "RETURN null AND false, null OR true, NOT null, null AND true, null OR false",
        &["[false,true,null,null,null]"],
    );
    check(
        &e,
        "RETURN true XOR false, null XOR true, false XOR false",
        &["[true,null,false]"],
    );
    check(
        &e,
        "RETURN 1 < 2, 'a' < 'b', 1 = 1.0, 'a' = 'a', [1,2] = [1,2], [1,null] = [1,null], 1 < 'a'",
        &["[true,true,true,true,true,null,null]"],
    );
    check(
        &e,
        "RETURN {a:1} = {a:1}, {a:1} = {a:2}, [1,2] = [1,3], [1,null] = [2,null], 1 = 'a'",
        &["[true,false,false,false,false]"],
    );
    check(
        &e,
        "RETURN null IS NULL, 1 IS NULL, null IS NOT NULL, 0/1 IS NOT NULL",
        &["[true,false,false,true]"],
    );
    check(
        &e,
        "WITH null AS x RETURN x < 1, x >= 1, x IN [1], x STARTS WITH 'a'",
        &["[null,null,null,null]"],
    );
    check(
        &e,
        "RETURN 0.1 + 0.2 = 0.3, 1 <= 1, 2 >= 3, 'b' > 'a'",
        &["[false,true,false,true]"],
    );
    check(
        &e,
        "UNWIND [1,null,3] AS x WITH x WHERE x > 1 RETURN x",
        &["[3]"],
    );
    check(
        &e,
        "UNWIND [true, false, null] AS x RETURN x AND true",
        &["[true]", "[false]", "[null]"],
    );
    check(
        &e,
        "RETURN null = null OR true, false AND null",
        &["[true,false]"],
    );
    check(&e, "RETURN CASE WHEN null THEN 1 ELSE 2 END", &["[2]"]);
}

#[test]
fn order_by_nulls_and_mixed() {
    let e = fixture();
    check(
        &e,
        "UNWIND [3, null, 1, 2] AS x RETURN x ORDER BY x",
        &["[1]", "[2]", "[3]", "[null]"],
    );
    check(
        &e,
        "UNWIND [3, null, 1, 2] AS x RETURN x ORDER BY x DESC",
        &["[null]", "[3]", "[2]", "[1]"],
    );
    check(
        &e,
        "UNWIND ['b', 'a', null] AS x RETURN x ORDER BY x",
        &["[\"a\"]", "[\"b\"]", "[null]"],
    );
    check(
        &e,
        "UNWIND [true, false] AS x RETURN x ORDER BY x",
        &["[false]", "[true]"],
    );
    check(
        &e,
        "UNWIND [2, 1.5, 1] AS x RETURN x ORDER BY x",
        &["[1]", "[1.5]", "[2]"],
    );
    check(
        &e,
        "UNWIND ['a', 1, true] AS x RETURN x ORDER BY x",
        &["[\"a\"]", "[true]", "[1]"],
    );
    check(
        &e,
        "UNWIND [[2], [1, 5], [1]] AS x RETURN x ORDER BY x",
        &["[[1]]", "[[1,5]]", "[[2]]"],
    );
}

#[test]
fn graph_functions_and_shapes() {
    let e = fixture();
    check(&e, "MATCH (a:Person {name:'Alice'})-[r:KNOWS]->(b {name:'Bob'}) RETURN type(r), startNode(r) = a, endNode(r) = b, properties(r), keys(a), labels(a)",
        &["[\"KNOWS\",true,true,{\"since\":2015},[\"age\",\"city\",\"name\"],[\"Person\"]]"]);
    check(
        &e,
        "MATCH (a:Person {name:'Alice'}) RETURN properties(a)",
        &["[{\"age\":30,\"city\":\"London\",\"name\":\"Alice\"}]"],
    );
    check(
        &e,
        "MATCH (a:Person {name:'Alice'}) RETURN id(a) >= 0, a:Person, a:Movie, a:Person:Movie",
        &["[true,true,false,false]"],
    );
    let r = read(&e, "MATCH (m:Movie {title:'Dune'}) RETURN m");
    let j = r.to_json_rows();
    assert_eq!(j[0]["m"]["title"], "Dune");
    assert_eq!(j[0]["m"]["year"], 2021);
    assert_eq!(j[0]["m"]["_labels"], serde_json::json!(["Movie"]));
    assert!(j[0]["m"]["_id"].is_i64());
    let r = read(&e, "MATCH ()-[r:DIRECTED]->() RETURN r");
    let j = r.to_json_rows();
    assert_eq!(j[0]["r"]["_type"], "DIRECTED");
    assert!(j[0]["r"]["_start"].is_i64() && j[0]["r"]["_end"].is_i64());
    let r = read(
        &e,
        "MATCH p=(a:Person {name:'Dave'})-[:DIRECTED]->(m) RETURN p",
    );
    let j = r.to_json_rows();
    let path = j[0]["p"].as_array().unwrap();
    assert_eq!(path.len(), 3);
    assert_eq!(path[0]["name"], "Dave");
    assert_eq!(path[1]["_type"], "DIRECTED");
    assert_eq!(path[2]["title"], "Inception");
    check(&e, "MATCH p=(a:Person {name:'Alice'})-[:KNOWS]->(b)-[:KNOWS]->(c {name:'Carol'}) RETURN length(p), size(nodes(p)), size(relationships(p))", &["[2,3,2]"]);
}

#[test]
fn with_unwind_union_subquery() {
    let e = fixture();
    check(
        &e,
        "UNWIND [1,2,3] AS x RETURN x * 2",
        &["[2]", "[4]", "[6]"],
    );
    check(&e, "UNWIND [] AS x RETURN x", &[]);
    check(&e, "UNWIND null AS x RETURN x", &[]);
    check(
        &e,
        "UNWIND [[1,2],[3]] AS l UNWIND l AS x RETURN x",
        &["[1]", "[2]", "[3]"],
    );
    check(
        &e,
        "MATCH (n:Person) WITH n.city AS c, count(*) AS k WHERE k > 1 RETURN c ORDER BY c",
        &["[\"London\"]", "[null]"],
    );
    check(&e, "WITH 1 AS a, 2 AS b RETURN a + b", &["[3]"]);
    check(&e, "MATCH (n:Movie) WITH * RETURN count(n)", &["[3]"]);
    check(
        &e,
        "MATCH (n:Person) WHERE n.age IS NOT NULL WITH n ORDER BY n.age DESC LIMIT 2 RETURN n.name",
        &["[\"Eve\"]", "[\"Carol\"]"],
    );
    check(&e, "RETURN 1 AS x UNION RETURN 1 AS x", &["[1]"]);
    check(&e, "RETURN 1 AS x UNION ALL RETURN 1 AS x", &["[1]", "[1]"]);
    check_set(&e, "MATCH (p:Person {name:'Alice'}) RETURN p.name AS n UNION MATCH (m:Movie {title:'Dune'}) RETURN m.title AS n", &["[\"Alice\"]", "[\"Dune\"]"]);
    check(&e, "MATCH (p:Person {name:'Alice'}) CALL { WITH p MATCH (p)-[:KNOWS]->(f) RETURN count(f) AS c } RETURN p.name, c", &["[\"Alice\",2]"]);
    check(
        &e,
        "UNWIND range(1,3) AS i WITH i WHERE i <> 2 RETURN collect(i)",
        &["[[1,3]]"],
    );
    check(
        &e,
        "WITH [1,2,3] AS xs UNWIND xs AS x WITH x, x * x AS sq RETURN sum(sq)",
        &["[14]"],
    );
}

#[test]
fn procedures() {
    let e = fixture();
    check(
        &e,
        "CALL db.labels() YIELD label RETURN label ORDER BY label",
        &["[\"Movie\"]", "[\"Person\"]"],
    );
    check(&e, "CALL db.relationshipTypes() YIELD relationshipType RETURN relationshipType ORDER BY relationshipType", &["[\"ACTED_IN\"]", "[\"DIRECTED\"]", "[\"KNOWS\"]"]);
    check(
        &e,
        "CALL db.propertyKeys() YIELD propertyKey RETURN propertyKey ORDER BY propertyKey",
        &[
            "[\"age\"]",
            "[\"city\"]",
            "[\"name\"]",
            "[\"role\"]",
            "[\"since\"]",
            "[\"title\"]",
            "[\"year\"]",
        ],
    );
    let r = read(&e, "CALL db.labels()");
    assert_eq!(r.columns, vec!["label"]);
    assert_eq!(r.rows.len(), 2);
    let r = read(&e, "CALL db.schema.visualization()");
    assert_eq!(r.columns, vec!["nodes", "relationships"]);
    let j = r.to_json_rows();
    assert_eq!(j[0]["nodes"].as_array().unwrap().len(), 2);
    assert!(j[0]["relationships"].as_array().unwrap().len() >= 3);
    check(&e, "CALL seal.pagerank() YIELD nodeId, score RETURN count(*), abs(sum(score) - 1.0) < 0.000001, max(score) > min(score)", &["[9,true,true]"]);
    check(
        &e,
        "CALL seal.wcc() YIELD nodeId, componentId RETURN count(DISTINCT componentId)",
        &["[2]"],
    );
    check(&e, "CALL seal.wcc() YIELD nodeId, componentId WITH componentId, count(*) AS size RETURN size ORDER BY size", &["[1]", "[8]"]);
    check(&e, "CALL seal.louvain() YIELD nodeId, communityId RETURN count(*), count(DISTINCT communityId) >= 2", &["[9,true]"]);
    check(
        &e,
        "CALL seal.betweenness() YIELD nodeId, score RETURN count(*), max(score) > 0",
        &["[9,true]"],
    );
    check(&e, "CALL seal.pagerank() YIELD nodeId, score MATCH (n:Person) WHERE id(n) = nodeId RETURN count(*)", &["[6]"]);
    check(
        &e,
        "CALL db.labels() YIELD label AS l WHERE l STARTS WITH 'P' RETURN l",
        &["[\"Person\"]"],
    );
}
