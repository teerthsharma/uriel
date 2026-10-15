use serde_json::Value as Json;

use crate::compact::{encode_table, Table};
use crate::config::Format;

pub(crate) fn render(value: &Json, format: Format) -> String {
    match (format, value) {
        (Format::Compact, Json::Object(_)) => match Table::from_rows(std::slice::from_ref(value)) {
            Some(table) => encode_table(&table),
            None => value.to_string(),
        },
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::compact::decode_table;

    #[test]
    fn an_object_becomes_a_one_row_table_that_decodes_back() {
        let v = json!({"a": 1, "b": "x", "c": [1, 2], "d": null});
        let text = render(&v, Format::Compact);
        assert!(text.starts_with("#1"), "{text}");
        assert_eq!(decode_table(&text).expect("decode").to_rows(), vec![v]);
    }

    #[test]
    fn json_format_is_the_plain_serialisation() {
        let v = json!({"a": 1});
        assert_eq!(render(&v, Format::Json), v.to_string());
    }

    #[test]
    fn a_non_object_stays_json_in_compact_format() {
        assert_eq!(render(&json!([1, 2]), Format::Compact), "[1,2]");
    }
}
