//! Generic, schema-driven settings so the UI can render any method's parameters.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub enum ParamKind {
    Int { min: i64, max: i64, step: i64, unit: &'static str },
    Float { min: f64, max: f64, step: f64, unit: &'static str },
    /// Fixed list of integer values with their labels.
    Choice(Vec<(i64, &'static str)>),
    Bool,
}

#[derive(Clone, Debug)]
pub struct ParamSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub help: &'static str,
    pub kind: ParamKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings(pub BTreeMap<String, Value>);

impl Settings {
    pub fn with(mut self, key: &str, value: Value) -> Self {
        self.0.insert(key.to_owned(), value);
        self
    }

    pub fn set(&mut self, key: &str, value: Value) {
        self.0.insert(key.to_owned(), value);
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        self.0.get(key).copied()
    }

    pub fn int(&self, key: &str, default: i64) -> i64 {
        match self.get(key) {
            Some(Value::Int(v)) => v,
            Some(Value::Float(v)) => v as i64,
            _ => default,
        }
    }

    pub fn float(&self, key: &str, default: f64) -> f64 {
        match self.get(key) {
            Some(Value::Float(v)) => v,
            Some(Value::Int(v)) => v as f64,
            _ => default,
        }
    }

    pub fn bool(&self, key: &str, default: bool) -> bool {
        match self.get(key) {
            Some(Value::Bool(v)) => v,
            _ => default,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub values: Settings,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_keys_fall_back_to_defaults() {
        let s = Settings::default();
        assert_eq!(s.get("x"), None);
        assert_eq!(s.int("x", 7), 7);
        assert_eq!(s.float("x", 1.5), 1.5);
        assert!(s.bool("x", true));
    }

    #[test]
    fn numbers_are_coerced_between_int_and_float() {
        let s = Settings::default().with("i", Value::Int(3)).with("f", Value::Float(2.9));
        assert_eq!(s.int("i", 0), 3);
        assert_eq!(s.float("i", 0.0), 3.0);
        // Float to int truncates toward zero.
        assert_eq!(s.int("f", 0), 2);
        assert_eq!(s.float("f", 0.0), 2.9);
    }

    #[test]
    fn type_mismatch_uses_the_default() {
        let s = Settings::default().with("b", Value::Bool(true)).with("i", Value::Int(1));
        assert_eq!(s.int("b", -1), -1);
        assert_eq!(s.float("b", -1.0), -1.0);
        // An integer is not a boolean.
        assert!(!s.bool("i", false));
        assert!(s.bool("b", false));
    }

    #[test]
    fn set_overwrites_the_previous_value() {
        let mut s = Settings::default().with("k", Value::Int(1));
        s.set("k", Value::Int(2));
        assert_eq!(s.get("k"), Some(Value::Int(2)));
        assert_eq!(s.0.len(), 1);
    }

    #[test]
    fn json_roundtrip() {
        let s = Settings::default()
            .with("a", Value::Int(4))
            .with("b", Value::Float(0.25))
            .with("c", Value::Bool(false));
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), s);
    }
}
