//! Request dispatch: the MCP lifecycle (`initialize`, `ping`), tools, and the
//! legacy `system.*` methods the shell used before MCP.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::protocol::{
    Request, Response, RpcError, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR,
};
use crate::settings::Settings;
use crate::system::{ProcessSort, SystemMonitor};
use crate::tools::{self, Caller, HostPaths, ToolContext};

/// Newest MCP revision implemented; listed first.
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const SERVER_INSTRUCTIONS: &str = "OSAIMA AI Core: live context and actions for this \
    computer. Prefer read-only tools to gather facts. Tools annotated destructiveHint \
    (kill_process, close_window, power_action) must be confirmed with the user first.";

/// Per-connection state.
#[derive(Debug)]
pub struct Session {
    pub caller: Caller,
    /// Version agreed during `initialize`, if the client performed it.
    pub protocol_version: Option<String>,
}

impl Session {
    pub fn new(caller: Caller) -> Self {
        Self {
            caller,
            protocol_version: None,
        }
    }
}

#[derive(Clone)]
pub struct Server {
    ctx: ToolContext,
}

impl Server {
    /// A server for the running system.
    pub fn new(monitor: Arc<SystemMonitor>, settings: Settings) -> Self {
        Self::with_context(ToolContext {
            monitor,
            paths: Arc::new(HostPaths::from_env()),
            settings: Arc::new(settings),
        })
    }

    pub fn settings(&self) -> &Settings {
        &self.ctx.settings
    }

    pub fn with_context(ctx: ToolContext) -> Self {
        Self { ctx }
    }

    /// Handle one raw message. Returns the serialized response, or `None`
    /// for notifications.
    pub async fn handle_message(&self, raw: &str, session: &mut Session) -> Option<String> {
        let response = match serde_json::from_str::<Value>(raw) {
            Err(err) => Some(Response::failure(
                Value::Null,
                RpcError::new(PARSE_ERROR, format!("parse error: {err}")),
            )),
            Ok(value) => self.handle_value(value, session).await,
        };
        response.map(|r| serde_json::to_string(&r).expect("responses always serialize"))
    }

    async fn handle_value(&self, value: Value, session: &mut Session) -> Option<Response> {
        let fallback_id = value.get("id").cloned().unwrap_or(Value::Null);
        let request: Request = match serde_json::from_value(value) {
            Ok(req) => req,
            Err(err) => {
                return Some(Response::failure(
                    fallback_id,
                    RpcError::new(INVALID_REQUEST, format!("invalid request: {err}")),
                ))
            }
        };
        if request.jsonrpc != "2.0" {
            return Some(Response::failure(
                request.id.unwrap_or(Value::Null),
                RpcError::new(INVALID_REQUEST, "jsonrpc must be \"2.0\""),
            ));
        }

        let result = self
            .dispatch(&request.method, request.params, session)
            .await;
        let id = request.id?; // notifications never get a response
        Some(match result {
            Ok(value) => Response::success(id, value),
            Err(err) => Response::failure(id, err),
        })
    }

    async fn dispatch(
        &self,
        method: &str,
        params: Option<Value>,
        session: &mut Session,
    ) -> Result<Value, RpcError> {
        match method {
            "initialize" => Ok(self.initialize(params, session)),
            "ping" => Ok(json!({})),
            "notifications/initialized" | "notifications/cancelled" => Ok(Value::Null),
            "tools/list" => Ok(json!({ "tools": tools::definitions(&self.ctx.settings) })),
            "tools/call" => {
                let params = params.unwrap_or(Value::Null);
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| RpcError::invalid_params("params.name must be a string"))?;
                tools::call(
                    &self.ctx,
                    session.caller,
                    name,
                    params.get("arguments").cloned(),
                )
                .await
            }
            // Pre-MCP methods kept for existing clients.
            "system.get_stats" => {
                Ok(serde_json::to_value(self.ctx.monitor.stats()).expect("stats always serialize"))
            }
            "system.get_processes" => Ok(json!(self.ctx.monitor.processes(ProcessSort::Cpu, 40))),
            _ => Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("method not found: {method}"),
            )),
        }
    }

    fn initialize(&self, params: Option<Value>, session: &mut Session) -> Value {
        let requested = params
            .as_ref()
            .and_then(|p| p.get("protocolVersion"))
            .and_then(Value::as_str);
        // Echo the client's version when we support it, otherwise offer our newest.
        let version = requested
            .filter(|v| SUPPORTED_PROTOCOL_VERSIONS.contains(v))
            .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0])
            .to_string();
        session.protocol_version = Some(version.clone());
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": "osaima-mcp-daemon",
                "title": "OSAIMA AI Core",
                "version": env!("CARGO_PKG_VERSION")
            },
            "instructions": SERVER_INSTRUCTIONS
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::INVALID_PARAMS;

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn setup() -> (Server, Session) {
        let server = Server::new(Arc::new(SystemMonitor::new()), Settings::default());
        (server, Session::new(Caller { uid: 0 }))
    }

    fn roundtrip(server: &Server, session: &mut Session, raw: &str) -> Response {
        let text = block_on(server.handle_message(raw, session)).expect("expected a response");
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn initialize_negotiates_version() {
        let (server, mut session) = setup();
        let res = roundtrip(
            &server,
            &mut session,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
        );
        let result = res.result.unwrap();
        assert_eq!(result["protocolVersion"], "2025-03-26");
        assert_eq!(result["serverInfo"]["name"], "osaima-mcp-daemon");
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(session.protocol_version.as_deref(), Some("2025-03-26"));

        let res = roundtrip(
            &server,
            &mut session,
            r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        );
        assert_eq!(
            res.result.unwrap()["protocolVersion"],
            SUPPORTED_PROTOCOL_VERSIONS[0]
        );
    }

    #[test]
    fn notifications_get_no_response() {
        let (server, mut session) = setup();
        let raw = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert!(block_on(server.handle_message(raw, &mut session)).is_none());
    }

    #[test]
    fn ids_are_echoed_verbatim() {
        let (server, mut session) = setup();
        let res = roundtrip(
            &server,
            &mut session,
            r#"{"jsonrpc":"2.0","id":42,"method":"ping"}"#,
        );
        assert_eq!(res.id, json!(42));
        assert_eq!(res.result, Some(json!({})));
        let res = roundtrip(
            &server,
            &mut session,
            r#"{"jsonrpc":"2.0","id":"a","method":"ping"}"#,
        );
        assert_eq!(res.id, json!("a"));
    }

    #[test]
    fn tools_list_and_call() {
        let (server, mut session) = setup();
        let res = roundtrip(
            &server,
            &mut session,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        let names: Vec<String> = res.result.unwrap()["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"list_processes".to_string()));

        let res = roundtrip(
            &server,
            &mut session,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_processes","arguments":{"limit":3}}}"#,
        );
        let result = res.result.unwrap();
        assert_eq!(result["isError"], false);
        assert!(
            result["structuredContent"]["processes"]
                .as_array()
                .unwrap()
                .len()
                <= 3
        );
    }

    #[test]
    fn errors_use_standard_codes() {
        let (server, mut session) = setup();
        let res = roundtrip(&server, &mut session, "{not json");
        assert_eq!(res.error.unwrap().code, PARSE_ERROR);
        assert_eq!(res.id, Value::Null);

        let res = roundtrip(
            &server,
            &mut session,
            r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#,
        );
        assert_eq!(res.error.unwrap().code, INVALID_REQUEST);

        let res = roundtrip(
            &server,
            &mut session,
            r#"{"jsonrpc":"2.0","id":3,"params":{}}"#,
        );
        assert_eq!(res.error.unwrap().code, INVALID_REQUEST);
        assert_eq!(res.id, json!(3));

        let res = roundtrip(
            &server,
            &mut session,
            r#"{"jsonrpc":"2.0","id":4,"method":"rm_rf"}"#,
        );
        assert_eq!(res.error.unwrap().code, METHOD_NOT_FOUND);

        let res = roundtrip(
            &server,
            &mut session,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{}}"#,
        );
        assert_eq!(res.error.unwrap().code, INVALID_PARAMS);
    }

    #[test]
    fn legacy_stats_method_still_works() {
        let (server, mut session) = setup();
        let res = roundtrip(
            &server,
            &mut session,
            r#"{"jsonrpc":"2.0","id":"1","method":"system.get_stats"}"#,
        );
        let stats = res.result.unwrap();
        assert!(stats["memory"]["total_bytes"].as_u64().unwrap() > 0);
        assert!(stats["cpu"]["usage_percent"].is_number());
    }
}
