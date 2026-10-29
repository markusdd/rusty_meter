//! Newline-delimited JSON between the open RustyMeter window and `rusty-meter-mcp`.
//!
//! One request per connection. The GUI owns the instrument; this protocol never
//! carries a raw SCPI string.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_ENDPOINT: &str = "127.0.0.1:9741";
pub const DEFAULT_PORT: u16 = 9741;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LineRequest {
    pub id: u64,
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volts: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amps: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ovp: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocp: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LineResponse {
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl LineResponse {
    pub fn success(id: u64, result: Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(id: u64, error: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(error.into()),
        }
    }

    /// One JSON object and a newline. Suitable for the TCP socket.
    pub fn to_line(&self) -> String {
        match serde_json::to_string(self) {
            Ok(mut line) => {
                line.push('\n');
                line
            }
            Err(_) => format!(
                "{{\"id\":{},\"ok\":false,\"error\":\"failed to encode response\"}}\n",
                self.id
            ),
        }
    }
}

pub fn parse_request(line: &str) -> Result<LineRequest, String> {
    serde_json::from_str(line.trim()).map_err(|err| format!("invalid request: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip_keeps_optional_fields() {
        let req = LineRequest {
            id: 7,
            op: "set_psu_limits".to_owned(),
            limit: None,
            mode: None,
            range: None,
            rate: None,
            volts: None,
            amps: None,
            ovp: Some(12.5),
            ocp: None,
            on: None,
            name: None,
        };
        let line = serde_json::to_string(&req).unwrap();
        let parsed = parse_request(&line).unwrap();
        assert_eq!(parsed, req);
        assert!(!line.contains("volts"));
    }

    #[test]
    fn failure_line_is_one_json_object() {
        let line = LineResponse::failure(3, "not connected").to_line();
        assert!(line.ends_with('\n'));
        let parsed: LineResponse = serde_json::from_str(line.trim()).unwrap();
        assert!(!parsed.ok);
        assert_eq!(parsed.error.as_deref(), Some("not connected"));
    }
}
