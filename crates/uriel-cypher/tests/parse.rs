//! Feature, precedence and error tests for the Cypher parser.

use uriel_cypher::*;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn parse_ok(q: &str) -> Query {
    parse(q).unwrap_or_else(|e| panic!("`{q}` failed to parse: {e}"))
}

/// Parses, prints, re-parses and asserts the trees are equal.
fn roundtrip(q: &str) -> Query {
    let a = parse_ok(q);
    let printed = a.to_string();
    let b = parse(&printed)
        .unwrap_or_else(|e| panic!("re-parse of `{printed}` (from `{q}`) failed: {e}"));
    assert_eq!(
        a, b,
        "round trip changed the tree for `{q}` (printed `{printed}`)"
    );
    assert_eq!(
        printed,
        b.to_string(),
        "display is not idempotent for `{q}`"
    );
    a
}

fn regular(q: &str) -> RegularQuery {
    match roundtrip(q).body {
        QueryBody::Regular(r) => r,
        other => panic!("expected regular query, got {other:?}"),
    }
}

fn clauses(q: &str) -> Vec<Clause> {
    regular(q).first.clauses
}

fn schema(q: &str) -> SchemaCommand {
    match roundtrip(q).body {
        QueryBody::Schema(s) => s,
        other => panic!("expected schema command, got {other:?}"),
    }
}

fn expr(src: &str) -> Expr {
    match clauses(&format!("RETURN {src}")).remove(0) {
        Clause::Return(r) => r.body.items.into_iter().next().unwrap().expr,
        other => panic!("expected RETURN, got {other:?}"),
    }
}

fn err(q: &str) -> ParseError {
    match parse(q) {
        Err(e) => e,
        Ok(ast) => panic!("`{q}` unexpectedly parsed: {ast:?}"),
    }
}

fn int(i: i64) -> Expr {
    Expr::Literal(Literal::Integer(i))
}
fn s(x: &str) -> Expr {
    Expr::Literal(Literal::String(x.into()))
}
fn v(x: &str) -> Expr {
    Expr::Variable(x.into())
}
fn bin(op: BinaryOp, l: Expr, r: Expr) -> Expr {
    Expr::Binary {
        op,
        lhs: Box::new(l),
        rhs: Box::new(r),
    }
}
fn un(op: UnaryOp, e: Expr) -> Expr {
    Expr::Unary {
        op,
        expr: Box::new(e),
    }
}
fn prop(base: Expr, key: &str) -> Expr {
    Expr::Property {
        base: Box::new(base),
        key: key.into(),
    }
}

fn match_clause(q: &str) -> MatchClause {
    match clauses(q).remove(0) {
        Clause::Match(m) => m,
        other => panic!("expected MATCH, got {other:?}"),
    }
}

fn first_rel(q: &str) -> RelPattern {
    let m = match_clause(q);
    m.pattern.parts[0].element.steps[0].rel.clone()
}

// ---------------------------------------------------------------------------
// one round-trip test per supported query shape
// ---------------------------------------------------------------------------

macro_rules! valid_queries {
    ($($name:ident: $q:expr;)*) => {
        $( #[test] fn $name() { roundtrip($q); } )*

        /// Every query above, for the aggregate round-trip test.
        const ALL_VALID: &[&str] = &[ $($q),* ];
    };
}

valid_queries! {
    // --- MATCH / patterns ---
    match_simple: "MATCH (n) RETURN n";
    match_label: "MATCH (n:Person) RETURN n";
    match_multi_labels: "MATCH (n:Person:Employee) RETURN n";
    match_props: "MATCH (n:Person {name: 'Alice', age: 30}) RETURN n";
    match_props_param: "MATCH (n:Person $props) RETURN n";
    match_props_no_label: "MATCH (n {id: $id}) RETURN n";
    match_empty_node: "MATCH () RETURN 1";
    match_right: "MATCH (a)-[r]->(b) RETURN r";
    match_left: "MATCH (a)<-[r]-(b) RETURN r";
    match_undirected: "MATCH (a)-[r]-(b) RETURN r";
    match_anonymous_arrows: "MATCH (a)-->(b)<--(c)--(d) RETURN a";
    match_both_arrows: "MATCH (a)<-->(b) RETURN a";
    match_rel_type: "MATCH (a)-[:KNOWS]->(b) RETURN a";
    match_rel_types: "MATCH (a)-[r:KNOWS|LIKES|LOVES]->(b) RETURN r";
    match_rel_types_legacy: "MATCH (a)-[r:KNOWS|:LIKES]->(b) RETURN r";
    match_rel_props: "MATCH (a)-[r:KNOWS {since: 2020}]->(b) RETURN r";
    match_rel_props_only: "MATCH (a)-[{w: 1}]->(b) RETURN a";
    match_varlen_star: "MATCH (a)-[*]->(b) RETURN b";
    match_varlen_fixed: "MATCH (a)-[*2]->(b) RETURN b";
    match_varlen_range: "MATCH (a)-[:R*1..3]->(b) RETURN b";
    match_varlen_upper: "MATCH (a)-[*..5]->(b) RETURN b";
    match_varlen_lower: "MATCH (a)-[*2..]->(b) RETURN b";
    match_varlen_with_props: "MATCH (a)-[r:R*1..3 {w: 1}]->(b) RETURN b";
    match_named_path: "MATCH p = (a)-->(b) RETURN p";
    match_shortest_path: "MATCH p = shortestPath((a)-[*]-(b)) RETURN p";
    match_all_shortest_paths: "MATCH p = allShortestPaths((a)-[*..6]-(b)) RETURN p";
    match_multiple_parts: "MATCH (a), (b)-->(c) RETURN a, b";
    match_where: "MATCH (n) WHERE n.age > 30 RETURN n";
    match_long_chain: "MATCH (a)-[:A]->(b)-[:B]->(c)<-[:C]-(d) RETURN d";
    match_where_pattern_predicate: "MATCH (a) WHERE (a)-->() RETURN a";
    match_where_not_pattern: "MATCH (a) WHERE NOT (a)-[:R]->(:L) RETURN a";
    optional_match: "MATCH (a) OPTIONAL MATCH (a)-->(b) RETURN a, b";
    optional_match_where: "MATCH (a) OPTIONAL MATCH (a)-->(b) WHERE b.x = 1 RETURN b";
    two_matches: "MATCH (a) MATCH (b) RETURN a, b";
    // --- WITH / RETURN ---
    return_star: "MATCH (n) RETURN *";
    return_star_extra: "MATCH (n) RETURN *, n.x AS x";
    return_distinct: "MATCH (n) RETURN DISTINCT n.name";
    return_aliases: "MATCH (n) RETURN n.name AS name, n.age AS age";
    return_order_default: "MATCH (n) RETURN n ORDER BY n.name";
    return_order_asc: "MATCH (n) RETURN n ORDER BY n.name ASC";
    return_order_desc: "MATCH (n) RETURN n ORDER BY n.name DESC";
    return_order_multi: "MATCH (n) RETURN n ORDER BY n.a DESC, n.b ASC, n.c";
    return_skip_limit: "MATCH (n) RETURN n SKIP 5 LIMIT 10";
    return_param_limit: "MATCH (n) RETURN n SKIP $s LIMIT $l";
    return_everything: "MATCH (n) RETURN DISTINCT n.x AS x ORDER BY x DESC SKIP 1 LIMIT 2";
    return_literal_only: "RETURN 1";
    with_simple: "MATCH (n) WITH n RETURN n";
    with_distinct: "MATCH (n) WITH DISTINCT n.x AS x RETURN x";
    with_star: "MATCH (n) WITH *, 1 AS one RETURN n, one";
    with_order_skip_limit: "MATCH (n) WITH n ORDER BY n.x DESC SKIP 1 LIMIT 3 RETURN n";
    with_where: "MATCH (n) WITH n, count(*) AS c WHERE c > 1 RETURN n";
    with_aggregation: "MATCH (n)-->(m) WITH n, collect(m) AS ms RETURN n, size(ms)";
    unwind_list: "UNWIND [1, 2, 3] AS x RETURN x";
    unwind_param: "UNWIND $rows AS row RETURN row";
    unwind_create: "UNWIND $rows AS row CREATE (:N {v: row.v})";
    // --- updating clauses ---
    create_node: "CREATE (n:Person {name: 'Bob'})";
    create_rel: "CREATE (a:A)-[:R {w: 1}]->(b:B)";
    create_multiple: "CREATE (a), (b), (a)-[:R]->(b)";
    create_return: "CREATE (n:N) RETURN n";
    create_named_path: "CREATE p = (a)-[:R]->(b) RETURN p";
    merge_node: "MERGE (n:Person {id: 1})";
    merge_on_create: "MERGE (n:P {id: 1}) ON CREATE SET n.created = 1";
    merge_on_match: "MERGE (n:P {id: 1}) ON MATCH SET n.seen = n.seen + 1";
    merge_both: "MERGE (n:P {id: 1}) ON CREATE SET n.c = 1, n.d = 2 ON MATCH SET n.m = 3";
    merge_rel: "MATCH (a), (b) MERGE (a)-[r:R]->(b) RETURN r";
    set_property: "MATCH (n) SET n.x = 1";
    set_multiple: "MATCH (n) SET n.x = 1, n.y = 'two'";
    set_replace_map: "MATCH (n) SET n = {a: 1, b: 2}";
    set_merge_map: "MATCH (n) SET n += {a: 1}";
    set_replace_param: "MATCH (n) SET n = $props";
    set_merge_param: "MATCH (n) SET n += $props";
    set_labels: "MATCH (n) SET n:Admin:Staff";
    set_mixed: "MATCH (n) SET n.x = 1, n:L, n += {y: 2}";
    set_nested_property: "MATCH (n) SET n.a.b = 1";
    remove_property: "MATCH (n) REMOVE n.x";
    remove_labels: "MATCH (n) REMOVE n:Old:Legacy";
    remove_mixed: "MATCH (n) REMOVE n.x, n:L";
    delete_node: "MATCH (n) DELETE n";
    delete_multiple: "MATCH (n)-[r]->(m) DELETE r, m";
    detach_delete: "MATCH (n) DETACH DELETE n";
    // --- UNION / CALL ---
    union_simple: "MATCH (a:A) RETURN a.x AS x UNION MATCH (b:B) RETURN b.x AS x";
    union_all: "MATCH (a:A) RETURN a.x AS x UNION ALL MATCH (b:B) RETURN b.x AS x";
    union_three: "RETURN 1 AS x UNION RETURN 2 AS x UNION ALL RETURN 3 AS x";
    call_proc: "CALL db.labels()";
    call_proc_no_parens: "CALL db.labels";
    call_proc_args: "CALL apoc.path.expand($start, 'KNOWS>', '', 1, 3)";
    call_yield: "CALL db.labels() YIELD label RETURN label";
    call_yield_alias_where: "CALL db.stats() YIELD a AS x, b WHERE x > 1 RETURN x, b";
    call_yield_star: "CALL db.stats() YIELD * RETURN 1";
    call_subquery: "MATCH (n) CALL { WITH n MATCH (n)-->(m) RETURN count(m) AS c } RETURN n, c";
    call_subquery_union: "CALL { RETURN 1 AS x UNION RETURN 2 AS x } RETURN x";
    call_subquery_scope: "MATCH (n) CALL (n) { MATCH (n)-->(m) RETURN m } RETURN m";
    call_subquery_update: "CALL { CREATE (n:N) } RETURN 1";
    // --- schema ---
    create_index: "CREATE INDEX FOR (n:Person) ON (n.name)";
    create_index_named: "CREATE INDEX person_name FOR (n:Person) ON (n.name)";
    create_index_if_not_exists: "CREATE INDEX idx IF NOT EXISTS FOR (n:Person) ON (n.name)";
    create_index_composite: "CREATE INDEX FOR (n:Person) ON (n.name, n.age)";
    create_index_legacy: "CREATE INDEX ON :Person(name)";
    create_index_rel: "CREATE INDEX r_since FOR ()-[r:KNOWS]-() ON (r.since)";
    create_range_index: "CREATE RANGE INDEX FOR (n:Person) ON (n.age)";
    create_text_index: "CREATE TEXT INDEX t FOR (n:Person) ON (n.bio)";
    drop_index: "DROP INDEX person_name";
    drop_index_if_exists: "DROP INDEX person_name IF EXISTS";
    create_constraint_unique: "CREATE CONSTRAINT FOR (n:Person) REQUIRE n.email IS UNIQUE";
    create_constraint_named: "CREATE CONSTRAINT c1 IF NOT EXISTS FOR (n:P) REQUIRE n.id IS UNIQUE";
    create_constraint_exists: "CREATE CONSTRAINT FOR (n:Person) REQUIRE n.name IS NOT NULL";
    create_constraint_rel: "CREATE CONSTRAINT FOR ()-[r:R]-() REQUIRE r.w IS NOT NULL";
    create_constraint_node_key: "CREATE CONSTRAINT FOR (n:P) REQUIRE (n.a, n.b) IS NODE KEY";
    create_constraint_composite: "CREATE CONSTRAINT FOR (n:P) REQUIRE (n.a, n.b) IS UNIQUE";
    create_constraint_legacy_unique: "CREATE CONSTRAINT ON (n:Person) ASSERT n.email IS UNIQUE";
    create_constraint_legacy_exists: "CREATE CONSTRAINT ON (n:Person) ASSERT exists(n.name)";
    drop_constraint: "DROP CONSTRAINT c1";
    drop_constraint_if_exists: "DROP CONSTRAINT c1 IF EXISTS";
    show_indexes: "SHOW INDEXES";
    show_index_singular: "SHOW INDEX";
    show_constraints: "SHOW CONSTRAINTS";
    // --- prefixes, trivia, case ---
    explain_prefix: "EXPLAIN MATCH (n) RETURN n";
    profile_prefix: "PROFILE MATCH (n) RETURN n";
    explain_schema: "EXPLAIN SHOW INDEXES";
    trailing_semicolon: "MATCH (n) RETURN n;";
    lowercase_keywords: "match (n:Person) where n.age > 1 return n order by n.age desc skip 1 limit 2";
    mixed_case_keywords: "Match (n) WhErE n.x Is Not NuLl ReTuRn n";
    line_comment: "MATCH (n) // comment\nRETURN n";
    block_comment: "MATCH /* inline */ (n) /* multi\nline */ RETURN n";
    comment_at_end: "RETURN 1 // trailing";
    multiline_query: "MATCH (n)\n  WHERE n.x = 1\n  RETURN n\n";
    // --- keywords as identifiers ---
    kw_as_property: "MATCH (n) RETURN n.match, n.return, n.order, n.where, n.end, n.set";
    kw_as_label: "MATCH (n:Order:Index:Set) RETURN n";
    kw_as_rel_type: "MATCH (n)-[:MATCH|CREATE|DELETE]->(m) RETURN n";
    kw_as_map_key: "RETURN {end: 1, set: 2, match: 3, `with`: 4}";
    kw_as_prop_map_key: "CREATE (n:N {order: 1, limit: 2})";
    kw_nonreserved_variable: "WITH 1 AS count, 2 AS type, 3 AS id, 4 AS index RETURN count, type, id, index";
    kw_backtick_variable: "MATCH (`match`) RETURN `match`";
    kw_backtick_label: "MATCH (n:`weird label`) RETURN n";
    kw_function_names: "RETURN size([1]), type(null), id(null), exists(null), coalesce(null, 1)";
    // --- expressions ---
    expr_int: "RETURN 42";
    expr_hex: "RETURN 0xFF, 0X1a";
    expr_octal: "RETURN 0o17, 017";
    expr_float: "RETURN 3.14, .5, 2.0";
    expr_float_exponent: "RETURN 1e10, 1.5E-3, 2e+2";
    expr_string_single: "RETURN 'hi'";
    expr_string_double: "RETURN \"hi\"";
    expr_string_escapes: "RETURN 'a\\nb\\t\\'q\\' \\\\ \\u00e9 \\\"'";
    expr_bool_null: "RETURN true, FALSE, null, NULL";
    expr_param: "RETURN $name, $0, $`odd name`";
    expr_neg_int_min: "RETURN -9223372036854775808";
    expr_arith: "RETURN 1 + 2 * 3 - 4 / 5 % 6 ^ 7";
    expr_unary: "RETURN -a, +a, - -a, -(-a)";
    expr_logic: "RETURN a OR b AND c XOR d";
    expr_not: "RETURN NOT a AND NOT (b OR c)";
    expr_comparison: "RETURN a = b, a <> b, a < b, a <= b, a > b, a >= b";
    expr_in: "RETURN x IN [1, 2, 3]";
    expr_string_ops: "RETURN a STARTS WITH 'x', b ENDS WITH 'y', c CONTAINS 'z'";
    expr_regex: "RETURN a =~ '.*foo'";
    expr_is_null: "RETURN a IS NULL, b IS NOT NULL";
    expr_property_chain: "RETURN a.b.c.d";
    expr_index: "RETURN a[0], a[-1], a[i + 1]";
    expr_slice: "RETURN a[1..3], a[..2], a[1..], a[..]";
    expr_postfix_chain: "RETURN a.b[0].c[1..2][3]";
    expr_function: "RETURN toUpper(n.name), substring('abc', 1, 2)";
    expr_function_nested: "RETURN f(g(h(1)), [x])";
    expr_function_namespaced: "RETURN apoc.text.join(['a'], ',')";
    expr_function_distinct: "RETURN count(DISTINCT n), collect(DISTINCT n.x)";
    expr_count_star: "RETURN count(*)";
    expr_case_simple: "RETURN CASE n.x WHEN 1 THEN 'a' WHEN 2 THEN 'b' ELSE 'c' END";
    expr_case_generic: "RETURN CASE WHEN n.x > 1 THEN 'big' WHEN n.x > 0 THEN 'small' END";
    expr_case_nested: "RETURN CASE WHEN a THEN CASE WHEN b THEN 1 ELSE 2 END ELSE 3 END";
    expr_list_literal: "RETURN [], [1], [1, 'a', [2, 3], {k: 1}]";
    expr_map_literal: "RETURN {}, {a: 1, b: {c: [1, 2]}}";
    expr_list_comp_basic: "RETURN [x IN [1, 2, 3] | x * 2]";
    expr_list_comp_filter: "RETURN [x IN range(1, 10) WHERE x % 2 = 0]";
    expr_list_comp_full: "RETURN [x IN range(1, 10) WHERE x > 3 | x ^ 2]";
    expr_pattern_comp: "MATCH (a) RETURN [(a)-->(b) | b.name]";
    expr_pattern_comp_where: "MATCH (a) RETURN [(a)-[:R]->(b:B) WHERE b.x > 1 | b.x]";
    expr_pattern_comp_path: "MATCH (a) RETURN [p = (a)-->(b) | length(p)]";
    expr_exists_pattern: "MATCH (n) WHERE EXISTS { (n)-[:R]->(:L) } RETURN n";
    expr_exists_where: "MATCH (n) WHERE EXISTS { (n)-->(m) WHERE m.x = 1 } RETURN n";
    expr_exists_subquery: "MATCH (n) WHERE EXISTS { MATCH (n)-->(m) RETURN m } RETURN n";
    expr_exists_function: "MATCH (n) WHERE exists(n.name) RETURN n";
    expr_label_predicate: "MATCH (n) WHERE n:Person RETURN n";
    expr_label_predicate_multi: "MATCH (n) RETURN n:A:B, n:C";
    expr_quantifiers: "RETURN all(x IN l WHERE x > 1), any(x IN l WHERE x), none(x IN l WHERE x), single(x IN l WHERE x)";
    expr_reduce: "RETURN reduce(acc = 0, x IN [1, 2, 3] | acc + x)";
    expr_parenthesised: "RETURN ((1 + 2) * (3 - 4))";
    expr_paren_property: "RETURN (a).b, ({x: 1}).x";
    expr_parameter_in_list: "RETURN [$a, $b]";
    expr_backtick_property: "RETURN n.`first name`";
    expr_unicode_string: "RETURN 'héllo wörld ✓'";
    expr_deep_but_ok: "RETURN ((((((((((1))))))))))";
}

#[test]
fn every_valid_query_round_trips() {
    assert!(ALL_VALID.len() >= 100, "only {} queries", ALL_VALID.len());
    for q in ALL_VALID {
        roundtrip(q);
    }
}

// ---------------------------------------------------------------------------
// precedence trees
// ---------------------------------------------------------------------------

#[test]
fn prec_add_mul() {
    assert_eq!(
        expr("1 + 2 * 3"),
        bin(BinaryOp::Add, int(1), bin(BinaryOp::Mul, int(2), int(3)))
    );
}

#[test]
fn prec_mul_add() {
    assert_eq!(
        expr("1 * 2 + 3"),
        bin(BinaryOp::Add, bin(BinaryOp::Mul, int(1), int(2)), int(3))
    );
}

#[test]
fn prec_parens_override() {
    assert_eq!(
        expr("(1 + 2) * 3"),
        bin(BinaryOp::Mul, bin(BinaryOp::Add, int(1), int(2)), int(3))
    );
}

#[test]
fn prec_left_assoc_sub() {
    assert_eq!(
        expr("1 - 2 - 3"),
        bin(BinaryOp::Sub, bin(BinaryOp::Sub, int(1), int(2)), int(3))
    );
}

#[test]
fn prec_not_and() {
    assert_eq!(
        expr("NOT a AND b"),
        bin(BinaryOp::And, un(UnaryOp::Not, v("a")), v("b"))
    );
}

#[test]
fn prec_not_binds_looser_than_comparison() {
    assert_eq!(
        expr("NOT a = b"),
        un(UnaryOp::Not, bin(BinaryOp::Eq, v("a"), v("b")))
    );
}

#[test]
fn prec_or_and() {
    assert_eq!(
        expr("a OR b AND c"),
        bin(BinaryOp::Or, v("a"), bin(BinaryOp::And, v("b"), v("c")))
    );
}

#[test]
fn prec_or_xor_and() {
    // OR < XOR < AND
    assert_eq!(
        expr("a OR b XOR c AND d"),
        bin(
            BinaryOp::Or,
            v("a"),
            bin(BinaryOp::Xor, v("b"), bin(BinaryOp::And, v("c"), v("d")))
        )
    );
}

#[test]
fn prec_comparison_below_and() {
    assert_eq!(
        expr("a = 1 AND b <> 2"),
        bin(
            BinaryOp::And,
            bin(BinaryOp::Eq, v("a"), int(1)),
            bin(BinaryOp::Ne, v("b"), int(2))
        )
    );
}

#[test]
fn prec_pow_left_assoc() {
    assert_eq!(
        expr("2 ^ 3 ^ 2"),
        bin(BinaryOp::Pow, bin(BinaryOp::Pow, int(2), int(3)), int(2))
    );
}

#[test]
fn prec_pow_above_mul() {
    assert_eq!(
        expr("2 * 3 ^ 2"),
        bin(BinaryOp::Mul, int(2), bin(BinaryOp::Pow, int(3), int(2)))
    );
}

#[test]
fn prec_pow_above_unary_minus() {
    assert_eq!(
        expr("-a ^ 2"),
        un(UnaryOp::Neg, bin(BinaryOp::Pow, v("a"), int(2)))
    );
}

#[test]
fn prec_unary_minus_above_mul() {
    assert_eq!(
        expr("-a * b"),
        bin(BinaryOp::Mul, un(UnaryOp::Neg, v("a")), v("b"))
    );
}

#[test]
fn prec_is_null_above_comparison_below_add() {
    assert_eq!(
        expr("a + b IS NULL"),
        Expr::IsNull {
            expr: Box::new(bin(BinaryOp::Add, v("a"), v("b"))),
            negated: false
        }
    );
    assert_eq!(
        expr("a = b IS NOT NULL"),
        bin(
            BinaryOp::Eq,
            v("a"),
            Expr::IsNull {
                expr: Box::new(v("b")),
                negated: true
            }
        )
    );
}

#[test]
fn prec_in_binds_looser_than_add() {
    assert_eq!(
        expr("a IN b + c"),
        bin(BinaryOp::In, v("a"), bin(BinaryOp::Add, v("b"), v("c")))
    );
}

#[test]
fn prec_string_predicates_below_comparison_chain() {
    assert_eq!(
        expr("a STARTS WITH 'x' AND b CONTAINS 'y'"),
        bin(
            BinaryOp::And,
            bin(BinaryOp::StartsWith, v("a"), s("x")),
            bin(BinaryOp::Contains, v("b"), s("y"))
        )
    );
}

#[test]
fn prec_property_binds_tightest() {
    assert_eq!(
        expr("a.b + c.d"),
        bin(BinaryOp::Add, prop(v("a"), "b"), prop(v("c"), "d"))
    );
}

#[test]
fn prec_regex_and_ends_with() {
    assert_eq!(expr("a =~ 'x'"), bin(BinaryOp::RegexMatch, v("a"), s("x")));
    assert_eq!(
        expr("a ENDS WITH 'x'"),
        bin(BinaryOp::EndsWith, v("a"), s("x"))
    );
}

// ---------------------------------------------------------------------------
// expression structure
// ---------------------------------------------------------------------------

#[test]
fn expr_literals_values() {
    assert_eq!(expr("0xFF"), int(255));
    assert_eq!(expr("0o17"), int(15));
    assert_eq!(expr("017"), int(15));
    assert_eq!(expr("1e3"), Expr::Literal(Literal::Float(1000.0)));
    assert_eq!(expr("1.5e-1"), Expr::Literal(Literal::Float(0.15)));
    assert_eq!(expr("true"), Expr::Literal(Literal::Bool(true)));
    assert_eq!(expr("NULL"), Expr::Literal(Literal::Null));
    assert_eq!(expr("-9223372036854775808"), int(i64::MIN));
}

#[test]
fn expr_string_escape_values() {
    assert_eq!(expr(r"'a\nb\u0041\'x'"), s("a\nbA'x"));
    assert_eq!(expr(r#""q\"q""#), s("q\"q"));
}

#[test]
fn expr_parameter_value() {
    assert_eq!(expr("$abc"), Expr::Parameter("abc".into()));
}

#[test]
fn expr_backtick_variable_value() {
    assert_eq!(expr("`my var`"), v("my var"));
}

#[test]
fn expr_index_and_slice_structure() {
    assert_eq!(
        expr("a[0]"),
        Expr::Index {
            base: Box::new(v("a")),
            index: Box::new(int(0))
        }
    );
    assert_eq!(
        expr("a[1..3]"),
        Expr::Slice {
            base: Box::new(v("a")),
            from: Some(Box::new(int(1))),
            to: Some(Box::new(int(3)))
        }
    );
    assert_eq!(
        expr("a[..3]"),
        Expr::Slice {
            base: Box::new(v("a")),
            from: None,
            to: Some(Box::new(int(3)))
        }
    );
    assert_eq!(
        expr("a[1..]"),
        Expr::Slice {
            base: Box::new(v("a")),
            from: Some(Box::new(int(1))),
            to: None
        }
    );
}

#[test]
fn expr_function_call_structure() {
    assert_eq!(
        expr("count(DISTINCT n.x)"),
        Expr::FunctionCall {
            namespace: vec![],
            name: "count".into(),
            distinct: true,
            args: vec![prop(v("n"), "x")]
        }
    );
    assert_eq!(
        expr("apoc.text.join(1)"),
        Expr::FunctionCall {
            namespace: vec!["apoc".into(), "text".into()],
            name: "join".into(),
            distinct: false,
            args: vec![int(1)]
        }
    );
}

#[test]
fn expr_count_star_structure() {
    assert_eq!(expr("COUNT(*)"), Expr::CountStar);
}

#[test]
fn expr_case_simple_structure() {
    assert_eq!(
        expr("CASE x WHEN 1 THEN 'a' ELSE 'b' END"),
        Expr::Case {
            scrutinee: Some(Box::new(v("x"))),
            whens: vec![(int(1), s("a"))],
            else_expr: Some(Box::new(s("b")))
        }
    );
}

#[test]
fn expr_case_generic_structure() {
    assert_eq!(
        expr("CASE WHEN a THEN 1 END"),
        Expr::Case {
            scrutinee: None,
            whens: vec![(v("a"), int(1))],
            else_expr: None
        }
    );
}

#[test]
fn expr_list_and_map_structure() {
    assert_eq!(expr("[1, 2]"), Expr::List(vec![int(1), int(2)]));
    assert_eq!(
        expr("{a: 1, b: 'x'}"),
        Expr::Map(vec![("a".into(), int(1)), ("b".into(), s("x"))])
    );
}

#[test]
fn expr_list_comprehension_structure() {
    assert_eq!(
        expr("[x IN l WHERE x > 1 | x * 2]"),
        Expr::ListComprehension {
            variable: "x".into(),
            list: Box::new(v("l")),
            predicate: Some(Box::new(bin(BinaryOp::Gt, v("x"), int(1)))),
            projection: Some(Box::new(bin(BinaryOp::Mul, v("x"), int(2))))
        }
    );
}

#[test]
fn expr_pattern_comprehension_structure() {
    match expr("[p = (a)-->(b) WHERE b.x | b.y]") {
        Expr::PatternComprehension {
            path_variable,
            pattern,
            predicate,
            projection,
        } => {
            assert_eq!(path_variable.as_deref(), Some("p"));
            assert_eq!(pattern.steps.len(), 1);
            assert!(predicate.is_some());
            assert_eq!(*projection, prop(v("b"), "y"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn expr_exists_structure() {
    match expr("EXISTS { (a)-->(b) WHERE b.x }") {
        Expr::Exists(body) => match *body {
            ExistsBody::Pattern {
                pattern,
                where_clause,
            } => {
                assert_eq!(pattern.parts.len(), 1);
                assert!(where_clause.is_some());
            }
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        expr("EXISTS { MATCH (a) RETURN a }"),
        Expr::Exists(b) if matches!(*b, ExistsBody::Subquery(_))
    ));
}

#[test]
fn expr_label_predicate_structure() {
    assert_eq!(
        expr("n:A:B"),
        Expr::HasLabels {
            expr: Box::new(v("n")),
            labels: vec!["A".into(), "B".into()]
        }
    );
}

#[test]
fn expr_pattern_predicate_structure() {
    assert!(matches!(expr("(a)-[:R]->(b)"), Expr::PatternPredicate(_)));
    // A parenthesised subtraction is not a pattern.
    assert_eq!(expr("(a) - b"), bin(BinaryOp::Sub, v("a"), v("b")));
}

#[test]
fn expr_quantifier_and_reduce_structure() {
    assert!(matches!(
        expr("any(x IN l WHERE x)"),
        Expr::ListPredicate {
            kind: Quantifier::Any,
            ..
        }
    ));
    assert!(matches!(
        expr("reduce(a = 0, x IN l | a + x)"),
        Expr::Reduce { .. }
    ));
}

// ---------------------------------------------------------------------------
// clause / pattern structure
// ---------------------------------------------------------------------------

#[test]
fn pattern_node_structure() {
    let m = match_clause("MATCH (n:A:B {k: 1}) RETURN n");
    let node = &m.pattern.parts[0].element.start;
    assert_eq!(node.variable.as_deref(), Some("n"));
    assert_eq!(node.labels, vec!["A", "B"]);
    assert_eq!(node.properties, Some(Expr::Map(vec![("k".into(), int(1))])));
}

#[test]
fn pattern_directions() {
    assert_eq!(
        first_rel("MATCH (a)-->(b) RETURN a").direction,
        Direction::Right
    );
    assert_eq!(
        first_rel("MATCH (a)<--(b) RETURN a").direction,
        Direction::Left
    );
    assert_eq!(
        first_rel("MATCH (a)--(b) RETURN a").direction,
        Direction::Both
    );
    assert_eq!(
        first_rel("MATCH (a)-[r]->(b) RETURN a").direction,
        Direction::Right
    );
    assert_eq!(
        first_rel("MATCH (a)<-[r]-(b) RETURN a").direction,
        Direction::Left
    );
    assert_eq!(
        first_rel("MATCH (a)-[r]-(b) RETURN a").direction,
        Direction::Both
    );
}

#[test]
fn pattern_rel_types_and_variable() {
    let r = first_rel("MATCH (a)-[r:A|B]->(b) RETURN a");
    assert_eq!(r.variable.as_deref(), Some("r"));
    assert_eq!(r.types, vec!["A", "B"]);
}

#[test]
fn pattern_variable_length_ranges() {
    let len = |q: &str| first_rel(q).length.unwrap();
    assert_eq!(
        len("MATCH (a)-[*]->(b) RETURN a"),
        LengthRange {
            min: None,
            max: None
        }
    );
    assert_eq!(
        len("MATCH (a)-[*2]->(b) RETURN a"),
        LengthRange {
            min: Some(2),
            max: Some(2)
        }
    );
    assert_eq!(
        len("MATCH (a)-[*1..3]->(b) RETURN a"),
        LengthRange {
            min: Some(1),
            max: Some(3)
        }
    );
    assert_eq!(
        len("MATCH (a)-[*..5]->(b) RETURN a"),
        LengthRange {
            min: None,
            max: Some(5)
        }
    );
    assert_eq!(
        len("MATCH (a)-[*2..]->(b) RETURN a"),
        LengthRange {
            min: Some(2),
            max: None
        }
    );
    assert!(first_rel("MATCH (a)-[r]->(b) RETURN a").length.is_none());
}

#[test]
fn pattern_named_and_shortest_paths() {
    let m = match_clause("MATCH p = shortestPath((a)-[*]-(b)) RETURN p");
    assert_eq!(m.pattern.parts[0].variable.as_deref(), Some("p"));
    assert_eq!(m.pattern.parts[0].kind, PathKind::Shortest);
    let m = match_clause("MATCH allShortestPaths((a)-[*]-(b)) RETURN 1");
    assert_eq!(m.pattern.parts[0].kind, PathKind::AllShortest);
    let m = match_clause("MATCH (a)-->(b) RETURN 1");
    assert_eq!(m.pattern.parts[0].kind, PathKind::Plain);
}

#[test]
fn clause_optional_match_flag() {
    assert!(!match_clause("MATCH (a) RETURN a").optional);
    let c = clauses("OPTIONAL MATCH (a) RETURN a");
    assert!(matches!(&c[0], Clause::Match(m) if m.optional));
}

#[test]
fn clause_return_body_structure() {
    match clauses("MATCH (n) RETURN DISTINCT n.x AS x, n.y ORDER BY x DESC, n.y SKIP 1 LIMIT 2")
        .remove(1)
    {
        Clause::Return(r) => {
            assert!(r.body.distinct);
            assert!(!r.body.include_existing);
            assert_eq!(r.body.items.len(), 2);
            assert_eq!(r.body.items[0].alias.as_deref(), Some("x"));
            assert_eq!(r.body.items[1].alias, None);
            assert_eq!(r.body.order_by.len(), 2);
            assert!(r.body.order_by[0].descending);
            assert!(!r.body.order_by[1].descending);
            assert_eq!(r.body.skip, Some(int(1)));
            assert_eq!(r.body.limit, Some(int(2)));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn clause_return_star_flag() {
    match clauses("MATCH (n) RETURN *, 1 AS one").remove(1) {
        Clause::Return(r) => {
            assert!(r.body.include_existing);
            assert_eq!(r.body.items.len(), 1);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn clause_with_where_structure() {
    match clauses("MATCH (n) WITH n WHERE n.x > 1 RETURN n").remove(1) {
        Clause::With(w) => assert!(w.where_clause.is_some()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn clause_set_item_kinds() {
    match clauses("MATCH (n) SET n.x = 1, n = {a: 1}, n += {b: 2}, n:L").remove(1) {
        Clause::Set(set) => {
            assert!(matches!(set.items[0], SetItem::Property { .. }));
            assert!(matches!(set.items[1], SetItem::Replace { .. }));
            assert!(matches!(set.items[2], SetItem::Merge { .. }));
            assert!(matches!(&set.items[3], SetItem::Labels { labels, .. } if labels == &["L"]));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn clause_merge_actions_preserve_order() {
    match clauses("MERGE (n:P) ON MATCH SET n.a = 1 ON CREATE SET n.b = 2").remove(0) {
        Clause::Merge(m) => {
            assert_eq!(m.actions.len(), 2);
            assert_eq!(m.actions[0].event, MergeEvent::Match);
            assert_eq!(m.actions[1].event, MergeEvent::Create);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn clause_delete_detach_flag() {
    assert!(matches!(&clauses("MATCH (n) DELETE n")[1], Clause::Delete(d) if !d.detach));
    assert!(matches!(&clauses("MATCH (n) DETACH DELETE n")[1], Clause::Delete(d) if d.detach));
}

#[test]
fn clause_remove_item_kinds() {
    match clauses("MATCH (n) REMOVE n.x, n:A:B").remove(1) {
        Clause::Remove(r) => {
            assert!(matches!(r.items[0], RemoveItem::Property(_)));
            assert!(matches!(&r.items[1], RemoveItem::Labels { labels, .. } if labels.len() == 2));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn clause_unwind_structure() {
    match clauses("UNWIND [1, 2] AS x RETURN x").remove(0) {
        Clause::Unwind(u) => {
            assert_eq!(u.variable, "x");
            assert_eq!(u.expr, Expr::List(vec![int(1), int(2)]));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn union_flags() {
    let q = regular("RETURN 1 AS x UNION RETURN 2 AS x UNION ALL RETURN 3 AS x");
    assert_eq!(q.unions.len(), 2);
    assert!(!q.unions[0].all);
    assert!(q.unions[1].all);
}

#[test]
fn call_structure() {
    match clauses("CALL db.stats(1, 2) YIELD a AS x, b WHERE x > 1 RETURN x").remove(0) {
        Clause::Call(c) => {
            assert_eq!(c.namespace, vec!["db"]);
            assert_eq!(c.name, "stats");
            assert_eq!(c.args.len(), 2);
            let y = c.yield_clause.unwrap();
            assert!(!y.star);
            assert_eq!(
                y.items[0],
                YieldItem {
                    name: "a".into(),
                    alias: Some("x".into())
                }
            );
            assert_eq!(
                y.items[1],
                YieldItem {
                    name: "b".into(),
                    alias: None
                }
            );
            assert!(y.where_clause.is_some());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn call_subquery_structure() {
    match clauses("CALL { MATCH (n) RETURN n } RETURN n").remove(0) {
        Clause::Subquery(s) => {
            assert!(s.scope.is_none());
            assert_eq!(s.query.first.clauses.len(), 2);
        }
        other => panic!("{other:?}"),
    }
    match clauses("CALL (a, b) { RETURN 1 AS x } RETURN x").remove(0) {
        Clause::Subquery(s) => assert_eq!(s.scope, Some(vec!["a".into(), "b".into()])),
        other => panic!("{other:?}"),
    }
}

#[test]
fn query_mode_prefix() {
    assert_eq!(parse_ok("EXPLAIN RETURN 1").mode, Some(QueryMode::Explain));
    assert_eq!(parse_ok("profile RETURN 1").mode, Some(QueryMode::Profile));
    assert_eq!(parse_ok("RETURN 1").mode, None);
}

#[test]
fn schema_create_index_structure() {
    assert_eq!(
        schema("CREATE INDEX idx IF NOT EXISTS FOR (n:Person) ON (n.name, n.age)"),
        SchemaCommand::CreateIndex(CreateIndex {
            name: Some("idx".into()),
            kind: None,
            if_not_exists: true,
            target: SchemaTarget::Node {
                variable: "n".into(),
                label: "Person".into()
            },
            properties: vec!["name".into(), "age".into()],
        })
    );
}

#[test]
fn schema_legacy_index_normalises() {
    match schema("CREATE INDEX ON :Person(name)") {
        SchemaCommand::CreateIndex(c) => {
            assert_eq!(c.properties, vec!["name"]);
            assert!(matches!(c.target, SchemaTarget::Node { ref label, .. } if label == "Person"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn schema_constraint_kinds() {
    let kind = |q: &str| match schema(q) {
        SchemaCommand::CreateConstraint(c) => c.kind,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        kind("CREATE CONSTRAINT FOR (n:P) REQUIRE n.a IS UNIQUE"),
        ConstraintKind::Unique
    );
    assert_eq!(
        kind("CREATE CONSTRAINT FOR (n:P) REQUIRE n.a IS NOT NULL"),
        ConstraintKind::NotNull
    );
    assert_eq!(
        kind("CREATE CONSTRAINT ON (n:P) ASSERT exists(n.a)"),
        ConstraintKind::NotNull
    );
    assert_eq!(
        kind("CREATE CONSTRAINT FOR (n:P) REQUIRE (n.a, n.b) IS NODE KEY"),
        ConstraintKind::NodeKey
    );
}

#[test]
fn schema_relationship_target() {
    match schema("CREATE CONSTRAINT FOR ()-[r:R]-() REQUIRE r.w IS NOT NULL") {
        SchemaCommand::CreateConstraint(c) => assert_eq!(
            c.target,
            SchemaTarget::Relationship {
                variable: "r".into(),
                rel_type: "R".into()
            }
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn schema_drop_and_show() {
    assert_eq!(
        schema("DROP INDEX i IF EXISTS"),
        SchemaCommand::DropIndex {
            name: "i".into(),
            if_exists: true
        }
    );
    assert_eq!(
        schema("DROP CONSTRAINT c"),
        SchemaCommand::DropConstraint {
            name: "c".into(),
            if_exists: false
        }
    );
    assert_eq!(schema("SHOW INDEXES"), SchemaCommand::ShowIndexes);
    assert_eq!(schema("SHOW CONSTRAINTS"), SchemaCommand::ShowConstraints);
}

// ---------------------------------------------------------------------------
// canonical display
// ---------------------------------------------------------------------------

#[test]
fn display_is_canonical_and_uppercases_keywords() {
    let q = parse_ok("match (n:Person)-[r:KNOWS]->(m) where n.age>30 and m.x is null return distinct n.name as name order by name desc skip 1 limit 5");
    assert_eq!(
        q.to_string(),
        "MATCH (n:Person)-[r:KNOWS]->(m) WHERE n.age > 30 AND m.x IS NULL RETURN DISTINCT n.name AS name ORDER BY name DESC SKIP 1 LIMIT 5"
    );
}

#[test]
fn display_adds_parentheses_only_where_needed() {
    assert_eq!(expr("(1 + 2) * 3").to_string(), "(1 + 2) * 3");
    assert_eq!(expr("(1 * 2) + 3").to_string(), "1 * 2 + 3");
    assert_eq!(expr("NOT (a AND b)").to_string(), "NOT (a AND b)");
    assert_eq!(expr("(a OR b) AND c").to_string(), "(a OR b) AND c");
    assert_eq!(expr("a - (b - c)").to_string(), "a - (b - c)");
    assert_eq!(expr("(-a) ^ 2").to_string(), "(-a) ^ 2");
}

#[test]
fn display_escapes_strings_and_identifiers() {
    assert_eq!(expr(r"'it\'s'").to_string(), r"'it\'s'");
    assert_eq!(expr("`a b`").to_string(), "`a b`");
    assert_eq!(expr("`match`").to_string(), "`match`");
    assert_eq!(expr("n.`x y`").to_string(), "n.`x y`");
    assert_eq!(expr("n.match").to_string(), "n.match");
}

#[test]
fn display_floats_keep_their_type() {
    assert_eq!(expr("2.0").to_string(), "2.0");
    assert_eq!(expr("1e300").to_string(), "1e300");
}

#[test]
fn parse_error_implements_std_error_and_displays_position() {
    let e = err("RETURN 'abc");
    let text = e.to_string();
    assert!(text.contains("line 1"), "{text}");
    assert!(text.contains("column 8"), "{text}");
    let _boxed: Box<dyn std::error::Error> = Box::new(e);
}

// ---------------------------------------------------------------------------
// errors: exact positions
// ---------------------------------------------------------------------------

#[test]
fn error_unterminated_string() {
    let e = err("RETURN 'abc");
    assert_eq!((e.line, e.col), (1, 8));
    assert_eq!(e.span, Span { start: 7, end: 11 });
    assert!(e.message.contains("unterminated string"), "{}", e.message);
}

#[test]
fn error_unterminated_string_second_line() {
    let e = err("RETURN 1,\n  \"abc");
    assert_eq!((e.line, e.col), (2, 3));
}

#[test]
fn error_unbalanced_paren_at_eof() {
    let e = err("RETURN (1 + 2");
    assert_eq!((e.line, e.col), (1, 14));
    assert!(e.message.contains("`)`"), "{}", e.message);
    assert!(e.message.contains("end of input"), "{}", e.message);
}

#[test]
fn error_unbalanced_paren_in_pattern() {
    let e = err("MATCH (n RETURN n");
    assert_eq!((e.line, e.col), (1, 10));
    assert!(e.message.contains("`)`"), "{}", e.message);
    assert!(e.message.contains("RETURN"), "{}", e.message);
}

#[test]
fn error_extra_closing_paren() {
    let e = err("RETURN (1))");
    assert_eq!((e.line, e.col), (1, 11));
}

#[test]
fn error_missing_expression_on_later_line() {
    let e = err("MATCH (n)\nWHERE n.age >\nRETURN n");
    assert_eq!((e.line, e.col), (3, 1));
    assert!(e.message.contains("expression"), "{}", e.message);
}

#[test]
fn error_unexpected_character() {
    let e = err("RETURN #");
    assert_eq!((e.line, e.col), (1, 8));
    assert!(e.message.contains('#'), "{}", e.message);
}

#[test]
fn error_unterminated_block_comment() {
    let e = err("RETURN 1 /* oops");
    assert_eq!((e.line, e.col), (1, 10));
}

#[test]
fn error_empty_input() {
    let e = err("");
    assert_eq!((e.line, e.col), (1, 1));
    let e = err("  \n  ");
    assert_eq!((e.line, e.col), (2, 3));
}

#[test]
fn error_trailing_garbage() {
    let e = err("RETURN 1 2");
    assert_eq!((e.line, e.col), (1, 10));
}

#[test]
fn error_misspelled_keyword_is_named() {
    let e = err("MATCH (n) RETRUN n");
    assert_eq!((e.line, e.col), (1, 11));
    assert!(e.message.contains("RETRUN"), "{}", e.message);
}

#[test]
fn error_unterminated_list_and_map() {
    assert_eq!((err("RETURN [1, 2").line, err("RETURN [1, 2").col), (1, 13));
    let e = err("RETURN {a: 1");
    assert_eq!((e.line, e.col), (1, 13));
    assert!(e.message.contains("`}`"), "{}", e.message);
}

#[test]
fn error_unclosed_rel_bracket() {
    let e = err("MATCH (a)-[r->(b) RETURN a");
    assert_eq!((e.line, e.col), (1, 13));
}

#[test]
fn error_column_counts_characters_not_bytes() {
    let e = err("RETURN 'éé' + #");
    assert_eq!(e.col, 15);
    assert_eq!(e.span.start, 16);
}

#[test]
fn error_invalid_escape() {
    let e = err(r"RETURN '\q'");
    assert_eq!((e.line, e.col), (1, 9));
}

#[test]
fn error_bad_numbers() {
    assert!(err("RETURN 0xZZ").message.contains("hexadecimal"));
    assert!(err("RETURN 9223372036854775808")
        .message
        .contains("out of range"));
    assert!(err("RETURN 1e999").message.contains("out of range"));
    assert!(err("RETURN 12abc").message.contains("numeric"));
    assert!(err("RETURN 09").message.contains("octal"));
}

#[test]
fn error_return_must_be_last() {
    let e = err("RETURN 1 MATCH (n) RETURN n");
    assert!(e.message.contains("RETURN"), "{}", e.message);
    assert_eq!((e.line, e.col), (1, 1));
}

#[test]
fn error_query_cannot_end_with_match() {
    let e = err("MATCH (n)");
    assert!(e.message.contains("MATCH"), "{}", e.message);
}

#[test]
fn error_reserved_word_as_variable() {
    let e = err("MATCH (return) RETURN 1");
    assert_eq!((e.line, e.col), (1, 8));
}

#[test]
fn error_schema_variable_mismatch() {
    let e = err("CREATE INDEX FOR (n:L) ON (m.p)");
    assert_eq!((e.line, e.col), (1, 28));
}

#[test]
fn error_case_requires_when() {
    assert!(err("RETURN CASE END").message.contains("WHEN"));
}

#[test]
fn error_deep_nesting_is_rejected_not_overflowed() {
    for open in ["(", "[", "{a: ", "NOT ", "-", "f("] {
        let q = format!("RETURN {}", open.repeat(20_000));
        let e = err(&q);
        assert!(!e.message.is_empty());
    }
}

#[test]
fn long_flat_queries_are_fine() {
    let list = (0..5_000)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    parse_ok(&format!("RETURN [{list}]"));
    let chain = "(a)-[:R]->".repeat(500) + "(z)";
    parse_ok(&format!("MATCH {chain} RETURN z"));
}
