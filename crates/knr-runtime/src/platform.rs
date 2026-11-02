use knr_core::Effect;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use thiserror::Error;

/// Runtime representation of knr SSA values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Value {
    I64(i64),
    F64(f64),
    Bool(bool),
    Str(String),
    Buf(Vec<u8>),
    Vec(Vec<Value>),
    Map(BTreeMap<String, Value>),
    Chan(usize),
    Struct {
        schema: String,
        fields: Vec<Value>,
    },
    Void,
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::I64(a), Value::I64(b)) => a == b,
            (Value::F64(a), Value::F64(b)) => {
                if a.is_nan() && b.is_nan() {
                    true
                } else {
                    a == b
                }
            }
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Buf(a), Value::Buf(b)) => a == b,
            (Value::Vec(a), Value::Vec(b)) => a == b,
            (Value::Map(a), Value::Map(b)) => a == b,
            (Value::Chan(a), Value::Chan(b)) => a == b,
            (
                Value::Struct {
                    schema: s1,
                    fields: f1,
                },
                Value::Struct {
                    schema: s2,
                    fields: f2,
                },
            ) => s1 == s2 && f1 == f2,
            (Value::Void, Value::Void) => true,
            _ => false,
        }
    }
}

impl Eq for Value {}

impl Value {
    /// Returns the inner i64 if self is `Value::I64`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::I64(v) => Some(*v),
            _ => None,
        }
    }

    /// Returns the inner f64 if self is `Value::F64`.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::F64(v) => Some(*v),
            _ => None,
        }
    }

    /// Returns the inner bool if self is `Value::Bool`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(v) => Some(*v),
            _ => None,
        }
    }

    /// Returns the inner string slice if self is `Value::Str`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Returns the inner byte slice if self is `Value::Buf`.
    pub fn as_buf(&self) -> Option<&[u8]> {
        match self {
            Value::Buf(b) => Some(b.as_slice()),
            _ => None,
        }
    }

    /// Returns the inner mutable byte vector if self is `Value::Buf`.
    pub fn as_buf_mut(&mut self) -> Option<&mut Vec<u8>> {
        match self {
            Value::Buf(b) => Some(b),
            _ => None,
        }
    }

    /// Returns the inner value slice if self is `Value::Vec`.
    pub fn as_vec(&self) -> Option<&[Value]> {
        match self {
            Value::Vec(v) => Some(v.as_slice()),
            _ => None,
        }
    }

    /// Returns the inner mutable vector if self is `Value::Vec`.
    pub fn as_vec_mut(&mut self) -> Option<&mut Vec<Value>> {
        match self {
            Value::Vec(v) => Some(v),
            _ => None,
        }
    }

    /// Returns the inner BTreeMap if self is `Value::Map`.
    pub fn as_map(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }

    /// Returns the inner mutable BTreeMap if self is `Value::Map`.
    pub fn as_map_mut(&mut self) -> Option<&mut BTreeMap<String, Value>> {
        match self {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }

    /// Returns the channel ID if self is `Value::Chan`.
    pub fn as_chan(&self) -> Option<usize> {
        match self {
            Value::Chan(c) => Some(*c),
            _ => None,
        }
    }

    /// Returns true if self is `Value::Void`.
    pub fn is_void(&self) -> bool {
        matches!(self, Value::Void)
    }

    /// Returns a reference to the struct schema and fields if self is `Value::Struct`.
    pub fn as_struct(&self) -> Option<(&str, &[Value])> {
        match self {
            Value::Struct { schema, fields } => Some((schema.as_str(), fields.as_slice())),
            _ => None,
        }
    }

    /// Returns a mutable reference to the struct schema and fields if self is `Value::Struct`.
    pub fn as_struct_mut(&mut self) -> Option<(&str, &mut Vec<Value>)> {
        match self {
            Value::Struct { schema, fields } => Some((schema.as_str(), fields)),
            _ => None,
        }
    }

    /// Returns the human-readable type name of this value variant.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::I64(_) => "i64",
            Value::F64(_) => "f64",
            Value::Bool(_) => "bool",
            Value::Str(_) => "str",
            Value::Buf(_) => "buf",
            Value::Vec(_) => "vec",
            Value::Map(_) => "map",
            Value::Chan(_) => "chan",
            Value::Struct { .. } => "struct",
            Value::Void => "void",
        }
    }

    /// Converts this `Value` into a `serde_json::Value`.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Value::I64(n) => serde_json::json!(*n),
            Value::F64(n) => {
                if n.is_nan() || n.is_infinite() {
                    serde_json::Value::Null
                } else {
                    serde_json::json!(*n)
                }
            }
            Value::Bool(b) => serde_json::json!(*b),
            Value::Str(s) => serde_json::json!(s),
            Value::Buf(b) => serde_json::Value::Array(
                b.iter().map(|byte| serde_json::json!(*byte)).collect(),
            ),
            Value::Vec(v) => {
                serde_json::Value::Array(v.iter().map(|item| item.to_json()).collect())
            }
            Value::Map(m) => {
                let mut map = serde_json::Map::new();
                for (k, v) in m {
                    map.insert(k.clone(), v.to_json());
                }
                serde_json::Value::Object(map)
            }
            Value::Chan(id) => serde_json::json!({ "chan": id }),
            Value::Struct { schema, fields } => serde_json::json!({
                "schema": schema,
                "fields": fields.iter().map(|f| f.to_json()).collect::<Vec<_>>()
            }),
            Value::Void => serde_json::Value::Null,
        }
    }

    /// Constructs a `Value` from a `serde_json::Value`.
    pub fn from_json(json: &serde_json::Value) -> Self {
        match json {
            serde_json::Value::Null => Value::Void,
            serde_json::Value::Bool(b) => Value::Bool(*b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Value::I64(i)
                } else if let Some(f) = n.as_f64() {
                    Value::F64(f)
                } else {
                    Value::Void
                }
            }
            serde_json::Value::String(s) => Value::Str(s.clone()),
            serde_json::Value::Array(arr) => {
                Value::Vec(arr.iter().map(Value::from_json).collect())
            }
            serde_json::Value::Object(obj) => {
                if let (Some(serde_json::Value::String(schema)), Some(serde_json::Value::Array(fields))) =
                    (obj.get("schema"), obj.get("fields"))
                {
                    if obj.len() == 2 {
                        return Value::Struct {
                            schema: schema.clone(),
                            fields: fields.iter().map(Value::from_json).collect(),
                        };
                    }
                }
                let mut map = BTreeMap::new();
                for (k, v) in obj {
                    map.insert(k.clone(), Value::from_json(v));
                }
                Value::Map(map)
            }
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::I64(v) => write!(f, "{v}"),
            Value::F64(v) => write!(f, "{v}"),
            Value::Bool(v) => write!(f, "{v}"),
            Value::Str(s) => write!(f, "\"{s}\""),
            Value::Buf(b) => write!(f, "<buf len={}>", b.len()),
            Value::Vec(v) => {
                write!(f, "[")?;
                for (i, item) in v.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, "]")
            }
            Value::Map(m) => {
                write!(f, "{{")?;
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "\"{k}\": {v}")?;
                }
                write!(f, "}}")
            }
            Value::Chan(c) => write!(f, "chan({c})"),
            Value::Struct { schema, fields } => {
                write!(f, "{schema} {{ ")?;
                for (i, field) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{i}: {field}")?;
                }
                write!(f, " }}")
            }
            Value::Void => write!(f, "()"),
        }
    }
}

/// Errors occurring during runtime platform operations.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum RuntimeError {
    #[error("IO error: {0}")]
    Io(String),

    #[error("Hardware alignment violation: expected 64-byte alignment")]
    UnalignedBuffer,

    #[error("Effect not supported: {0:?}")]
    UnsupportedEffect(Effect),

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("Platform error: {0}")]
    Platform(String),

    #[error("Channel error: {0}")]
    Channel(String),
}

/// Capability-gated host platform interface.
pub trait HostPlatform: Send + Sync {
    /// Executes a platform operation isolated by capability effect.
    fn host_call(&mut self, effect: Effect, op: &str, payload: Value) -> Result<Value, RuntimeError>;
}
