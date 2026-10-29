//! stdio MCP server. Tool calls are forwarded to the RustyMeter window.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde_json::{Value, json};

use crate::agent_proto::{DEFAULT_ENDPOINT, LineRequest, LineResponse};

pub fn run() -> std::io::Result<()> {
    let endpoint = endpoint_from_args();
    let stdin = std::io::stdin();
    let mut input = BufReader::new(stdin.lock());
    let mut output = std::io::stdout().lock();
    let mut framed = false;
    while let Some(message) = read_message(&mut input, &mut framed)? {
        if let Some(response) = dispatch_message(&message, &endpoint) {
            write_message(&mut output, &response, framed)?;
        }
    }
    Ok(())
}

fn endpoint_from_args() -> String {
    let mut endpoint =
        std::env::var("RUSTY_METER_AGENT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_owned());
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--endpoint"
            && let Some(value) = args.next()
        {
            endpoint = value;
        }
    }
    endpoint
}

/// `Some` when the client used `Content-Length` framing for this message.
fn read_message(input: &mut impl BufRead, framed: &mut bool) -> std::io::Result<Option<String>> {
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        if !line.trim().is_empty() {
            break;
        }
    }
    if line.trim_start().starts_with('{') {
        *framed = false;
        return Ok(Some(line.trim().to_owned()));
    }
    *framed = true;
    let mut content_length = None;
    loop {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().ok();
        }
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
    }
    let Some(len) = content_length else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "MCP header is missing Content-Length",
        ));
    };
    let mut buf = vec![0_u8; len];
    input.read_exact(&mut buf)?;
    String::from_utf8(buf)
        .map(Some)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
}

fn write_message(output: &mut impl Write, body: &str, framed: bool) -> std::io::Result<()> {
    if framed {
        write!(output, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    } else {
        writeln!(output, "{body}")?;
    }
    output.flush()
}

fn dispatch_message(message: &str, endpoint: &str) -> Option<String> {
    let value: Value = match serde_json::from_str(message) {
        Ok(value) => value,
        Err(err) => {
            return Some(error_body(
                Value::Null,
                -32700,
                &format!("parse error: {err}"),
            ));
        }
    };
    let id = value.get("id").cloned().unwrap_or(Value::Null);
    let method = value.get("method").and_then(Value::as_str).unwrap_or("");
    if value.get("id").is_none_or(|id| id.is_null()) {
        return None;
    }
    let body = match method {
        "initialize" => result_body(id, initialize_result(&value)),
        "ping" => result_body(id, json!({})),
        "tools/list" => result_body(id, json!({ "tools": tool_defs() })),
        "tools/call" => result_body(id, call_tool(&value, endpoint)),
        _ => error_body(id, -32601, "Method not found"),
    };
    Some(body)
}

fn initialize_result(request: &Value) -> Value {
    let client_version = request
        .pointer("/params/protocolVersion")
        .and_then(Value::as_str)
        .filter(|version| !version.is_empty())
        .unwrap_or("2024-11-05");
    json!({
        "protocolVersion": client_version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "rusty-meter",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": "RustyMeter must already be open. In Menu → Agent, enable Listen for an agent. Reads work then. Writes also need Allow agent control. Commands use the same buttons and sliders as the window, so a person watching sees them. set_output is separate from voltage and current. get_trace matches the graph and clears on a mode change. get_history keeps samples across mode changes.",
    })
}

fn call_tool(request: &Value, endpoint: &str) -> Value {
    let name = request
        .pointer("/params/name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let args = request
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(json!({}));
    match forward_tool(name, &args, endpoint) {
        Ok(value) => tool_result(&pretty(&value), false),
        Err(err) => tool_result(&err, true),
    }
}

fn forward_tool(name: &str, args: &Value, endpoint: &str) -> Result<Value, String> {
    let request = LineRequest {
        id: 1,
        op: name.to_owned(),
        limit: args
            .get("limit")
            .and_then(Value::as_u64)
            .map(|n| n as usize),
        mode: args.get("mode").and_then(Value::as_str).map(str::to_owned),
        range: args.get("range").and_then(Value::as_str).map(str::to_owned),
        rate: args.get("rate").and_then(Value::as_str).map(str::to_owned),
        volts: finite_f32(args.get("volts")),
        amps: finite_f32(args.get("amps")),
        ovp: finite_f32(args.get("ovp")),
        ocp: finite_f32(args.get("ocp")),
        on: args.get("on").and_then(Value::as_bool),
        name: args.get("name").and_then(Value::as_str).map(str::to_owned),
    };
    if request.op.is_empty() {
        return Err("tool name is empty".into());
    }
    query_app(endpoint, &request)
}

fn finite_f32(value: Option<&Value>) -> Option<f32> {
    let number = value?.as_f64()?;
    number.is_finite().then_some(number as f32)
}

pub fn query_app(endpoint: &str, request: &LineRequest) -> Result<Value, String> {
    let addr = endpoint
        .to_socket_addrs()
        .map_err(|err| format!("bad agent endpoint {endpoint}: {err}"))?
        .next()
        .ok_or_else(|| format!("bad agent endpoint {endpoint}"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|_| {
        format!(
            "Could not connect to RustyMeter at {endpoint}. Open the app and enable Menu → Agent → Listen for an agent. The port must match."
        )
    })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|err| err.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|err| err.to_string())?;
    let line = serde_json::to_string(request).map_err(|err| err.to_string())? + "\n";
    stream
        .write_all(line.as_bytes())
        .map_err(|err| err.to_string())?;
    let mut response_line = String::new();
    BufReader::new(stream)
        .read_line(&mut response_line)
        .map_err(|err| err.to_string())?;
    if response_line.trim().is_empty() {
        return Err("RustyMeter closed the agent connection".into());
    }
    let response: LineResponse =
        serde_json::from_str(response_line.trim()).map_err(|err| err.to_string())?;
    if response.ok {
        Ok(response.result.unwrap_or(Value::Null))
    } else {
        Err(response
            .error
            .unwrap_or_else(|| "agent request failed".into()))
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn tool_result(text: &str, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
    })
}

fn result_body(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error_body(id: Value, code: i32, message: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
    .to_string()
}

fn tool_defs() -> Vec<Value> {
    vec![
        tool(
            "get_session",
            "Connection, instrument identity, mode, range, rate, recording state, and PSU setpoints. Call this first.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "get_reading",
            "Latest measurement. channels[] uses the recorder shape (name, unit, value). overload is true for an open lead or a SCPI sentinel, not for a normal 1 GΩ or 1 GHz reading. value is null when the window has no sample yet.",
            json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "get_trace",
            "Samples currently drawn on the graph, oldest first. Cleared when the mode changes or the user resets the graph. Each sample has t_ms.",
            limit_schema(),
        ),
        tool(
            "get_history",
            "Recent samples that survive a mode change. Cleared when the instrument disconnects. Prefer this when a mode switch would wipe the graph.",
            limit_schema(),
        ),
        tool(
            "get_recording",
            "Tail of the recorder buffer. Empty unless the user has started a recording in the window.",
            limit_schema(),
        ),
        tool(
            "set_mode",
            "Switch a SCPI multimeter mode. Same action as a mode button. Clears the graph trace when the mode actually changes. Requires Allow agent control. Victor meters are read-only.",
            json!({
                "type": "object",
                "properties": {
                    "mode": { "type": "string", "description": "Vdc, Vac, Adc, Aac, Res, Fres, Cap, Freq, Per, Diod, Cont, Temp" }
                },
                "required": ["mode"]
            }),
        ),
        tool(
            "set_range",
            "Set the manual range or auto. Examples: auto, 5V, 50E-3. Requires Allow agent control.",
            json!({
                "type": "object",
                "properties": { "range": { "type": "string" } },
                "required": ["range"]
            }),
        ),
        tool(
            "set_rate",
            "Set the sampling rate by label or SCPI code. Examples: Slow, Medium, Fast, S, M, F. Requires Allow agent control.",
            json!({
                "type": "object",
                "properties": { "rate": { "type": "string" } },
                "required": ["rate"]
            }),
        ),
        tool(
            "set_psu_voltage",
            "Set the PSU voltage setpoint in volts. Clamped to the model rating. Does not change the output. Rejected while the user is dragging that slider.",
            json!({
                "type": "object",
                "properties": { "volts": { "type": "number" } },
                "required": ["volts"]
            }),
        ),
        tool(
            "set_psu_current",
            "Set the PSU current setpoint in amps. Clamped to the model rating. Does not enable the output.",
            json!({
                "type": "object",
                "properties": { "amps": { "type": "number" } },
                "required": ["amps"]
            }),
        ),
        tool(
            "set_psu_limits",
            "Set OVP (volts), OCP (amps), or both. Clamped to the model's protection headroom.",
            json!({
                "type": "object",
                "properties": {
                    "ovp": { "type": "number" },
                    "ocp": { "type": "number" }
                }
            }),
        ),
        tool(
            "set_output",
            "Turn the PSU output on or off. Separate from the setpoint tools.",
            json!({
                "type": "object",
                "properties": { "on": { "type": "boolean" } },
                "required": ["on"]
            }),
        ),
        tool(
            "run_macro",
            "Run a saved SCPI macro by name. The macro must apply to the connected instrument.",
            json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }),
        ),
    ]
}

fn limit_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "limit": { "type": "integer", "minimum": 1, "description": "How many samples to return, from the newest end." }
        }
    })
}

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": schema,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn initialize_echoes_protocol_and_lists_tools() {
        let init = dispatch_message(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
            "127.0.0.1:9",
        )
        .unwrap();
        let init: Value = serde_json::from_str(&init).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], "rusty-meter");

        let listed = dispatch_message(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            "127.0.0.1:9",
        )
        .unwrap();
        let listed: Value = serde_json::from_str(&listed).unwrap();
        let names: Vec<&str> = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert!(names.contains(&"get_reading"));
        assert!(names.contains(&"get_history"));
        assert!(names.contains(&"set_output"));
        assert!(names.contains(&"set_mode"));
    }

    #[test]
    fn notification_has_no_response() {
        assert!(
            dispatch_message(
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                "127.0.0.1:9"
            )
            .is_none()
        );
    }

    #[test]
    fn tool_call_reads_the_open_app() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(sock.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let req: LineRequest = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(req.op, "get_session");
            let body = LineResponse::success(req.id, json!({"connected": false})).to_line();
            sock.write_all(body.as_bytes()).unwrap();
        });
        let response = dispatch_message(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_session","arguments":{}}}"#,
            &format!("127.0.0.1:{port}"),
        )
        .unwrap();
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["result"]["isError"], false);
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("\"connected\": false")
        );
        server.join().unwrap();
    }

    #[test]
    fn content_length_framing_roundtrip() {
        let mut framed = false;
        let raw = b"Content-Length: 7\r\n\r\n{\"a\":1}";
        let mut cursor = std::io::Cursor::new(&raw[..]);
        let message = read_message(&mut cursor, &mut framed).unwrap().unwrap();
        assert!(framed);
        assert_eq!(message, "{\"a\":1}");
    }
}
