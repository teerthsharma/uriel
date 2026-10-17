use uriel_core::MemoryBudget;
use uriel_query::Engine;

#[test]
fn the_cypher_result_cache_is_sized_from_the_budget() {
    let small = MemoryBudget::mib(32);
    let large = MemoryBudget::mib(256);
    let a = Engine::open_in_memory(small).unwrap();
    let b = Engine::open_in_memory(large).unwrap();
    assert_eq!(a.cache_capacity(), small.query_cache());
    assert_eq!(b.cache_capacity(), large.query_cache());
    assert!(a.cache_capacity() < b.cache_capacity());
}

#[test]
fn an_engine_built_from_a_file_uses_the_budget_for_the_graph_and_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let budget = MemoryBudget::mib(50);
    let e = Engine::open(dir.path().join("g.uriel"), budget).unwrap();
    assert_eq!(e.graph().budget(), budget);
    assert_eq!(e.cache_capacity(), budget.query_cache());
}
