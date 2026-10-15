//! Keyword tables shared by the parser and the AST printer.

/// Words that may not be used as bare variable names (they must be
/// backtick-quoted). Property names, labels, relationship types and map keys
/// may be any keyword.
const RESERVED: &[&str] = &[
    "AND",
    "AS",
    "ASC",
    "ASCENDING",
    "BY",
    "CALL",
    "CASE",
    "CONTAINS",
    "CREATE",
    "DELETE",
    "DESC",
    "DESCENDING",
    "DETACH",
    "DISTINCT",
    "ELSE",
    "END",
    "ENDS",
    "EXPLAIN",
    "FALSE",
    "IN",
    "IS",
    "LIMIT",
    "MATCH",
    "MERGE",
    "NOT",
    "NULL",
    "ON",
    "OPTIONAL",
    "OR",
    "ORDER",
    "PROFILE",
    "REMOVE",
    "RETURN",
    "SET",
    "SKIP",
    "STARTS",
    "THEN",
    "TRUE",
    "UNION",
    "UNWIND",
    "WHEN",
    "WHERE",
    "WITH",
    "XOR",
    "YIELD",
];

/// Returns true if `word` (case-insensitive) is reserved as a variable name.
pub(crate) fn is_reserved(word: &str) -> bool {
    RESERVED.iter().any(|k| k.eq_ignore_ascii_case(word))
}
