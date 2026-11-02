pub use knr_runtime::Value;

use crate::error::VmError;
use std::collections::BTreeMap;

/// Conversion helpers to create `Value` variants easily.
pub trait ToValue {
    fn to_value(self) -> Value;
}

impl ToValue for Value {
    fn to_value(self) -> Value {
        self
    }
}

impl ToValue for i64 {
    fn to_value(self) -> Value {
        Value::I64(self)
    }
}

impl ToValue for f64 {
    fn to_value(self) -> Value {
        Value::F64(self)
    }
}

impl ToValue for bool {
    fn to_value(self) -> Value {
        Value::Bool(self)
    }
}

impl ToValue for &str {
    fn to_value(self) -> Value {
        Value::Str(self.to_string())
    }
}

impl ToValue for String {
    fn to_value(self) -> Value {
        Value::Str(self)
    }
}

impl ToValue for Vec<u8> {
    fn to_value(self) -> Value {
        Value::Buf(self)
    }
}

impl ToValue for &[u8] {
    fn to_value(self) -> Value {
        Value::Buf(self.to_vec())
    }
}

impl ToValue for Vec<Value> {
    fn to_value(self) -> Value {
        Value::Vec(self)
    }
}

impl ToValue for BTreeMap<String, Value> {
    fn to_value(self) -> Value {
        Value::Map(self)
    }
}

/// Extension methods for extracting typed values from `Value`.
pub trait ValueExt {
    fn try_into_i64(self, line: usize) -> Result<i64, VmError>;
    fn try_into_f64(self, line: usize) -> Result<f64, VmError>;
    fn try_into_bool(self, line: usize) -> Result<bool, VmError>;
    fn try_into_str(self, line: usize) -> Result<String, VmError>;
    fn try_into_buf(self, line: usize) -> Result<Vec<u8>, VmError>;
    fn try_into_vec(self, line: usize) -> Result<Vec<Value>, VmError>;
    fn try_into_map(self, line: usize) -> Result<BTreeMap<String, Value>, VmError>;
    fn try_into_struct(self, line: usize) -> Result<(String, Vec<Value>), VmError>;
}

impl ValueExt for Value {
    fn try_into_i64(self, line: usize) -> Result<i64, VmError> {
        match self {
            Value::I64(v) => Ok(v),
            other => Err(VmError::TypeMismatch {
                expected: "i64".to_string(),
                actual: other.type_name().to_string(),
                line,
            }),
        }
    }

    fn try_into_f64(self, line: usize) -> Result<f64, VmError> {
        match self {
            Value::F64(v) => Ok(v),
            other => Err(VmError::TypeMismatch {
                expected: "f64".to_string(),
                actual: other.type_name().to_string(),
                line,
            }),
        }
    }

    fn try_into_bool(self, line: usize) -> Result<bool, VmError> {
        match self {
            Value::Bool(v) => Ok(v),
            other => Err(VmError::TypeMismatch {
                expected: "bool".to_string(),
                actual: other.type_name().to_string(),
                line,
            }),
        }
    }

    fn try_into_str(self, line: usize) -> Result<String, VmError> {
        match self {
            Value::Str(v) => Ok(v),
            other => Err(VmError::TypeMismatch {
                expected: "str".to_string(),
                actual: other.type_name().to_string(),
                line,
            }),
        }
    }

    fn try_into_buf(self, line: usize) -> Result<Vec<u8>, VmError> {
        match self {
            Value::Buf(v) => Ok(v),
            other => Err(VmError::TypeMismatch {
                expected: "buf".to_string(),
                actual: other.type_name().to_string(),
                line,
            }),
        }
    }

    fn try_into_vec(self, line: usize) -> Result<Vec<Value>, VmError> {
        match self {
            Value::Vec(v) => Ok(v),
            other => Err(VmError::TypeMismatch {
                expected: "vec".to_string(),
                actual: other.type_name().to_string(),
                line,
            }),
        }
    }

    fn try_into_map(self, line: usize) -> Result<BTreeMap<String, Value>, VmError> {
        match self {
            Value::Map(v) => Ok(v),
            other => Err(VmError::TypeMismatch {
                expected: "map".to_string(),
                actual: other.type_name().to_string(),
                line,
            }),
        }
    }

    fn try_into_struct(self, line: usize) -> Result<(String, Vec<Value>), VmError> {
        match self {
            Value::Struct { schema, fields } => Ok((schema, fields)),
            other => Err(VmError::TypeMismatch {
                expected: "struct".to_string(),
                actual: other.type_name().to_string(),
                line,
            }),
        }
    }
}
