//! JSON-RPC 2.0 and the slice of MCP this server implements.
//!
//! Newline-delimited JSON on stdin and stdout — the MCP stdio transport. Written out
//! by hand rather than pulled from a crate because the surface actually needed is
//! `initialize`, `tools/list` and `tools/call`, and hand-rolling those is smaller than
//! the dependency would be.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Protocol revision this server speaks. If a client asks for a different one we echo
/// theirs back when we can still serve it — MCP revisions have so far been additive.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    #[allow(dead_code)]
    pub jsonrpc: Option<String>,
    /// Absent for notifications, which get no response.
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

pub mod codes {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
}

impl Response {
    pub fn ok(id: Value, result: Value) -> Self {
        Response { jsonrpc: "2.0", id, result: Some(result), error: None }
    }

    pub fn err(id: Value, code: i32, message: impl Into<String>) -> Self {
        Response {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcError { code, message: message.into(), data: None }),
        }
    }
}

/// A tool as advertised to the client.
#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// The result of a tool call.
///
/// MCP reports tool *failures* in the result with `isError`, not as JSON-RPC errors:
/// a tool that refused is information the model should read and act on, whereas a
/// transport error is not something it can do anything about.
pub struct ToolResult {
    pub content: Vec<Value>,
    pub is_error: bool,
}

impl ToolResult {
    pub fn text(body: impl Into<String>) -> Self {
        ToolResult {
            content: vec![json!({ "type": "text", "text": body.into() })],
            is_error: false,
        }
    }

    pub fn error(body: impl Into<String>) -> Self {
        ToolResult {
            content: vec![json!({ "type": "text", "text": body.into() })],
            is_error: true,
        }
    }

    /// Text plus a rendered image — how a snapshot comes back.
    pub fn image(caption: impl Into<String>, png_base64: String) -> Self {
        ToolResult {
            content: vec![
                json!({ "type": "text", "text": caption.into() }),
                json!({ "type": "image", "data": png_base64, "mimeType": "image/png" }),
            ],
            is_error: false,
        }
    }

    pub fn to_value(&self) -> Value {
        json!({ "content": self.content, "isError": self.is_error })
    }
}

/// Shorthand for the common `{ "type": "object", "properties": {...} }` schema.
pub fn schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}
