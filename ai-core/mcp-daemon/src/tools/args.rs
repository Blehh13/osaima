//! Validated access to a tool's `arguments` object.

use serde_json::{Map, Value};

use super::ToolError;

#[derive(Debug, Default)]
pub struct Args(Map<String, Value>);

impl Args {
    /// Accept a missing/null/object `arguments` value and reject keys not in `allowed`.
    pub fn parse(arguments: Option<Value>, allowed: &[&str]) -> Result<Self, ToolError> {
        let map = match arguments {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(map)) => map,
            Some(_) => return Err(ToolError::invalid("arguments must be an object")),
        };
        if let Some(key) = map.keys().find(|k| !allowed.contains(&k.as_str())) {
            return Err(ToolError::invalid(format!("unexpected argument: {key}")));
        }
        Ok(Self(map))
    }

    fn present(&self, key: &str) -> Option<&Value> {
        self.0.get(key).filter(|v| !v.is_null())
    }

    pub fn u64(&self, key: &str) -> Result<Option<u64>, ToolError> {
        self.present(key)
            .map(|v| {
                v.as_u64().ok_or_else(|| {
                    ToolError::invalid(format!("{key} must be a non-negative integer"))
                })
            })
            .transpose()
    }

    pub fn i64(&self, key: &str) -> Result<Option<i64>, ToolError> {
        self.present(key)
            .map(|v| {
                v.as_i64()
                    .ok_or_else(|| ToolError::invalid(format!("{key} must be an integer")))
            })
            .transpose()
    }

    pub fn bool(&self, key: &str) -> Result<Option<bool>, ToolError> {
        self.present(key)
            .map(|v| {
                v.as_bool()
                    .ok_or_else(|| ToolError::invalid(format!("{key} must be true or false")))
            })
            .transpose()
    }

    pub fn str(&self, key: &str) -> Result<Option<&str>, ToolError> {
        self.present(key)
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| ToolError::invalid(format!("{key} must be a string")))
            })
            .transpose()
    }

    /// A non-empty, trimmed string.
    pub fn required_str(&self, key: &str) -> Result<&str, ToolError> {
        match self.str(key)?.map(str::trim) {
            Some(s) if !s.is_empty() => Ok(s),
            _ => Err(ToolError::invalid(format!("{key} is required"))),
        }
    }

    /// An integer in `min..=max`, or `default` when absent.
    pub fn u64_in(&self, key: &str, min: u64, max: u64, default: u64) -> Result<u64, ToolError> {
        let value = self.u64(key)?.unwrap_or(default);
        if (min..=max).contains(&value) {
            Ok(value)
        } else {
            Err(ToolError::invalid(format!(
                "{key} must be between {min} and {max}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_missing_or_null_arguments() {
        assert!(Args::parse(None, &[]).is_ok());
        assert!(Args::parse(Some(Value::Null), &[]).is_ok());
    }

    #[test]
    fn rejects_unknown_keys_and_non_objects() {
        assert!(Args::parse(Some(json!({ "x": 1 })), &["y"]).is_err());
        assert!(Args::parse(Some(json!([1])), &[]).is_err());
    }

    #[test]
    fn typed_accessors_validate() {
        let args = Args::parse(
            Some(json!({ "n": 5, "s": " hi ", "b": true, "neg": -3, "empty": "  " })),
            &["n", "s", "b", "neg", "empty"],
        )
        .unwrap();
        assert_eq!(args.u64("n").unwrap(), Some(5));
        assert_eq!(args.required_str("s").unwrap(), "hi");
        assert_eq!(args.bool("b").unwrap(), Some(true));
        assert_eq!(args.i64("neg").unwrap(), Some(-3));
        assert!(args.u64("neg").is_err());
        assert!(args.required_str("empty").is_err());
        assert!(args.required_str("missing").is_err());
        assert_eq!(args.u64_in("n", 1, 10, 3).unwrap(), 5);
        assert_eq!(args.u64_in("absent", 1, 10, 3).unwrap(), 3);
        assert!(args.u64_in("n", 6, 10, 7).is_err());
    }
}
