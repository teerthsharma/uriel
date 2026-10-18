use crate::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn parse(_text: &str) -> Result<Value, Error> {
        unimplemented!()
    }

    pub fn to_json(&self) -> String {
        unimplemented!()
    }
}
