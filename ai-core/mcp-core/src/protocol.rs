//! JSON-RPC 2.0 message types (<https://www.jsonrpc.org/specification>).

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;

/// An incoming request or notification.
#[derive(Debug, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    /// `None` when the member is absent (a notification). An explicit
    /// `"id": null` is kept as `Some(Value::Null)`.
    #[serde(default, deserialize_with = "present")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

fn present<'de, D: Deserializer<'de>>(de: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(de).map(Some)
}

impl Request {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(id: Value, error: RpcError) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn absent_id_is_notification() {
        let req: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .unwrap();
        assert!(req.is_notification());
    }

    #[test]
    fn null_and_numeric_ids_are_requests() {
        let req: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#).unwrap();
        assert_eq!(req.id, Some(Value::Null));
        let req: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#).unwrap();
        assert_eq!(req.id, Some(json!(7)));
    }

    #[test]
    fn failure_omits_result() {
        let res = Response::failure(json!(1), RpcError::new(METHOD_NOT_FOUND, "nope"));
        let text = serde_json::to_string(&res).unwrap();
        assert!(!text.contains("result"));
        assert!(text.contains("-32601"));
    }
}
