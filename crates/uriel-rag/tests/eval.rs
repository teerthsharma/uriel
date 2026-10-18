use std::path::Path;

#[test]
#[ignore = "measurement, run explicitly"]
fn recall_on_the_frozen_corpus() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let report = uriel_rag::eval::run_frozen(
        &tests.join("frozen_corpus"),
        &tests.join("eval_queries.json"),
    )
    .expect("eval");
    println!("{report}");
    assert_eq!(report.queries, 100);
    for r in [
        report.baseline_recall_at_5,
        report.baseline_recall_at_10,
        report.rag_recall_at_5,
        report.rag_recall_at_10,
    ] {
        assert!((0.0..=1.0).contains(&r));
    }
    assert!((report.ladder_recall_at_5 - report.rag_recall_at_5).abs() < 1e-9);
    assert!((report.ladder_recall_at_10 - report.rag_recall_at_10).abs() < 1e-9);
    assert!(report.baseline_recall_at_5 <= report.baseline_recall_at_10);
    assert!(report.rag_recall_at_5 <= report.rag_recall_at_10);
}

#[test]
#[ignore = "measurement, run explicitly"]
fn soul_guide_coverage_on_the_frozen_corpus() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let report = uriel_rag::eval::run_soul_frozen(
        &tests.join("frozen_corpus"),
        &tests.join("eval_queries.json"),
    )
    .expect("soul eval");
    println!("{report}");
    assert_eq!(report.queries, 100);
    assert_eq!(report.gold_resolved, 100);
    assert!(report.repeats_identical);
    for r in &report.rows {
        assert!(r.guide_tokens <= r.budget);
        assert!(r.baseline_tokens <= r.guide_tokens);
        assert!(r.diffusion_hits <= 100 && r.baseline_hits <= 100);
    }
}
