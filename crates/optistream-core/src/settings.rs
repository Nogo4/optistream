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
