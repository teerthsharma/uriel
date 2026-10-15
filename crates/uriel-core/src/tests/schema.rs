//! Schema introspection.

use serde_json::json;

use super::{mem, p};
use crate::{ConstraintKind, Schema};

#[test]
fn empty_graph_has_an_empty_schema() {
    let g = mem();
    let s = g.schema().unwrap();
    assert_eq!(s.epoch, 0);
    assert_eq!((s.node_count, s.rel_count), (0, 0));
    assert!(s.labels.is_empty() && s.rel_types.is_empty());
    assert!(s.indexes.is_empty() && s.constraints.is_empty());
}

fn populated() -> crate::Graph {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    let a = tx
        .create_node(
            &["Person"],
            p(&[("name", "Ada".into()), ("age", 36.into())]),
        )
        .unwrap();
    let b = tx
        .create_node(
            &["Person"],
            p(&[("name", "Bob".into()), ("age", 1.5.into())]),
        )
        .unwrap();
    let _c = tx
        .create_node(&["Person", "Admin"], p(&[("name", "Cy".into())]))
        .unwrap();
    let co = tx
        .create_node(
            &["Company"],
            p(&[
                ("name", "Acme".into()),
                ("tags", crate::Value::List(vec![])),
            ]),
        )
        .unwrap();
    tx.create_rel(a, co, "WORKS_AT", p(&[("since", 2020.into())]))
        .unwrap();
    tx.create_rel(a, b, "KNOWS", p(&[])).unwrap();
    tx.commit().unwrap();
    g.create_index("Person", "name").unwrap();
    g.create_unique_constraint("Company", "name").unwrap();
    g.create_exists_constraint("Company", "name").unwrap();
    g
}

#[test]
fn schema_reports_labels_types_properties_indexes_and_constraints() {
    let g = populated();
    let s = g.schema().unwrap();
    assert_eq!(s.node_count, 4);
    assert_eq!(s.rel_count, 2);
    assert_eq!(s.epoch, g.epoch());

    let names: Vec<&str> = s.labels.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, vec!["Admin", "Company", "Person"]);
    let person = s.labels.iter().find(|l| l.name == "Person").unwrap();
    assert_eq!(person.count, 3);
    assert_eq!(person.sampled, 3);
    assert_eq!(person.properties["name"], vec!["String"]);
    assert_eq!(
        person.properties["age"],
        vec!["Float", "Integer"],
        "mixed types are all listed, sorted"
    );
    let company = s.labels.iter().find(|l| l.name == "Company").unwrap();
    assert_eq!(company.properties["tags"], vec!["List"]);

    let types: Vec<&str> = s.rel_types.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(types, vec!["KNOWS", "WORKS_AT"]);
    let works = s.rel_types.iter().find(|t| t.name == "WORKS_AT").unwrap();
    assert_eq!(works.count, 1);
    assert_eq!(works.properties["since"], vec!["Integer"]);
    assert!(s
        .rel_types
        .iter()
        .find(|t| t.name == "KNOWS")
        .unwrap()
        .properties
        .is_empty());

    assert_eq!(
        s.indexes.len(),
        2,
        "user index plus the unique constraint's backing index"
    );
    assert!(s
        .indexes
        .iter()
        .any(|i| i.label == "Person" && !i.backs_constraint));
    assert!(s
        .indexes
        .iter()
        .any(|i| i.label == "Company" && i.backs_constraint));
    assert_eq!(s.constraints.len(), 2);
    assert!(s
        .constraints
        .iter()
        .any(|c| c.kind == ConstraintKind::Unique));
}

#[test]
fn schema_serializes_to_json_and_back() {
    let g = populated();
    let s = g.schema().unwrap();
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(v["node_count"], json!(4));
    let person = v["labels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["name"] == "Person")
        .unwrap();
    assert_eq!(person["properties"]["age"], json!(["Float", "Integer"]));
    assert_eq!(v["constraints"][0]["kind"], json!("unique"));
    assert!(v["indexes"].is_array());
    let text = serde_json::to_string(&s).unwrap();
    let back: Schema = serde_json::from_str(&text).unwrap();
    assert_eq!(back, s);
}

#[test]
fn schema_samples_a_bounded_prefix_but_counts_exactly() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    for i in 0..10 {
        let key = if i < 3 { "early" } else { "late" };
        tx.create_node(&["S"], p(&[(key, i.into())])).unwrap();
    }
    tx.commit().unwrap();
    let r = g.begin_read().unwrap();
    let s = r.schema_sampled(3).unwrap();
    let l = &s.labels[0];
    assert_eq!(l.count, 10);
    assert_eq!(l.sampled, 3);
    assert!(l.properties.contains_key("early") && !l.properties.contains_key("late"));
    let full = r.schema_sampled(100).unwrap();
    assert_eq!(full.labels[0].sampled, 10);
    assert_eq!(full.labels[0].properties.len(), 2);
}

#[test]
fn schema_follows_deletes() {
    let g = populated();
    let mut tx = g.begin_write().unwrap();
    let ids: Vec<_> = tx.node_ids_by_label("Company").unwrap();
    for id in ids {
        tx.detach_delete_node(id).unwrap();
    }
    tx.commit().unwrap();
    let s = g.schema().unwrap();
    assert!(s.labels.iter().all(|l| l.name != "Company"));
    assert!(s.rel_types.iter().all(|t| t.name != "WORKS_AT"));
    assert_eq!(s.rel_count, 1);
}
