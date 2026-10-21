//! `pa-mcp`: the ProjectArchitect MCP server (SPEC § 10, D-11). A stdio JSON-RPC 2.0 server over the same `pa.db` as
//! the GUI. Messages are newline-delimited on stdin and stdout; logs go to stderr only.
//! ponytail: hand-rolled subset (initialize, ping, tools/list, tools/call); no resources, prompts or progress.

use pa_core::Store;
use pa_mcp::{Server, tools};
use serde_json::{Map, Value, json};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const LATEST: &str = "2025-06-18";
const INSTRUCTIONS: &str = "ProjectArchitect keeps the registry of all my coding projects: their checkouts, TODO and roadmap \
files, notes, git state and TODOs across projects. Call current_project with your working directory first to learn which \
project this is. Projects are given as an id, a name or a path inside a checkout. Launchers (VS Code, terminal, Explorer, \
Claude) open desktop apps, so they are not available here. Tools that push, open pull requests, or create GitHub \
repositories or issues are off unless the user enables publishing in the app's settings. Claude config tools preview \
by default; writing (apply) is off unless the user allows it in the app's settings. create_project makes new projects \
from the user's templates.";

fn main() {
    let mut data_dir = pa_core::default_data_dir();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--data-dir", Some(dir)) => data_dir = PathBuf::from(dir),
            _ => {
                eprintln!("pa-mcp: usage: pa-mcp [--data-dir <folder>]");
                std::process::exit(2);
            }
        }
    }
    pa_core::install::remove_old_self();
    let db = data_dir.join("pa.db");
    let store = match Store::open(&db) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("pa-mcp: cannot open {}: {e}", db.display());
            std::process::exit(1);
        }
    };
    let server = Server { store, data_dir };
    if let Err(e) = serve(&server, io::stdin().lock(), io::stdout().lock()) {
        eprintln!("pa-mcp: {e}");
        std::process::exit(1);
    }
}

/// One reply line per request (a message with an `id`); notifications get nothing.
fn serve(server: &Server, input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle(server, &line) {
            writeln!(output, "{reply}")?;
            output.flush()?;
        }
    }
    Ok(())
}

type RpcError = (i64, String);

/// One message in, its reply out. `None` for notifications (no `id`), which never get a reply.
pub fn handle(server: &Server, line: &str) -> Option<String> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(msg) => msg,
        Err(e) => return Some(error_reply(Value::Null, -32700, &format!("parse error: {e}"))),
    };
    let id = msg.get("id")?.clone();
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return Some(error_reply(id, -32600, "missing method"));
    };
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    Some(match dispatch(server, method, &params) {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
        Err((code, message)) => error_reply(id, code, &message),
    })
}

fn error_reply(id: Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

fn invalid(message: impl Into<String>) -> RpcError {
    (-32602, message.into())
}

fn dispatch(server: &Server, method: &str, params: &Value) -> Result<Value, RpcError> {
    match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
            let version = if PROTOCOLS.contains(&asked) { asked } else { LATEST };
            Ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "projectarchitect", "version": VERSION},
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools::list()})),
        "tools/call" => call_tool(server, params),
        _ => Err((-32601, format!("method not found: {method}"))),
    }
}

fn call_tool(server: &Server, params: &Value) -> Result<Value, RpcError> {
    let name = params.get("name").and_then(Value::as_str).ok_or_else(|| invalid("tools/call needs a tool name"))?;
    let empty = Map::new();
    let args = match params.get("arguments") {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(map)) => map,
        Some(_) => return Err(invalid("arguments must be an object")),
    };
    match tools::call(server, name, args) {
        Ok(out) => {
            let text = format!("{}\n{}", out.summary, serde_json::to_string_pretty(&out.data).unwrap_or_default());
            Ok(json!({"content": [{"type": "text", "text": text}], "structuredContent": out.data, "isError": false}))
        }
        Err(tools::Failure::Tool(message)) => Ok(json!({"content": [{"type": "text", "text": message}], "isError": true})),
        Err(tools::Failure::Params(message)) => Err(invalid(message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str) -> Server {
        let data_dir = std::env::temp_dir().join(format!("pa-mcp-proto-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data_dir);
        Server { store: Store::open(&data_dir.join("pa.db")).expect("store"), data_dir }
    }

    fn reply(s: &Server, line: &str) -> Value {
        serde_json::from_str(&handle(s, line).expect("a reply")).expect("json")
    }

    #[test]
    fn initialize_echoes_known_versions_and_defaults_the_rest() {
        let s = server("init");
        let r = reply(&s, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#);
        assert_eq!(r["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(r["result"]["serverInfo"]["name"], "projectarchitect");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        assert!(r["result"]["instructions"].as_str().unwrap().contains("current_project"));
        let r = reply(&s, r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#);
        assert_eq!(r["result"]["protocolVersion"], LATEST);
    }

    #[test]
    fn ping_and_ids_are_echoed() {
        let s = server("ping");
        let r = reply(&s, r#"{"jsonrpc":"2.0","id":"abc","method":"ping"}"#);
        assert_eq!(r, json!({"jsonrpc": "2.0", "id": "abc", "result": {}}));
    }

    #[test]
    fn tools_list_has_an_object_schema_for_every_tool() {
        let s = server("list");
        let r = reply(&s, r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#);
        let tools = r["result"]["tools"].as_array().expect("tools");
        assert!(tools.len() >= 30);
        for t in tools {
            assert!(t["name"].as_str().is_some_and(|n| !n.is_empty()));
            assert!(t["description"].as_str().is_some_and(|d| !d.is_empty()));
            assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
        }
    }

    #[test]
    fn notifications_get_no_reply() {
        let s = server("notify");
        assert!(handle(&s, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        assert!(handle(&s, r#"{"jsonrpc":"2.0","method":"something/unknown"}"#).is_none());
    }

    #[test]
    fn unknown_method_and_bad_input_are_json_rpc_errors() {
        let s = server("errors");
        let r = reply(&s, r#"{"jsonrpc":"2.0","id":4,"method":"resources/list"}"#);
        assert_eq!(r["error"]["code"], -32601);
        assert_eq!(r["id"], 4);
        let r = reply(&s, r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"arguments":{}}}"#);
        assert_eq!(r["error"]["code"], -32602);
        let r = reply(&s, r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"no_such_tool"}}"#);
        assert_eq!(r["error"]["code"], -32602);
        let r = reply(&s, r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"get_project","arguments":{}}}"#);
        assert_eq!(r["error"]["code"], -32602, "missing project argument");
        let r = reply(&s, r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"list_projects","arguments":[]}}"#);
        assert_eq!(r["error"]["code"], -32602);
        let r = reply(&s, "{not json");
        assert_eq!(r["error"]["code"], -32700);
    }

    #[test]
    fn tool_failures_are_results_with_is_error() {
        let s = server("failure");
        let r = reply(&s, r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"get_project","arguments":{"project":"nowhere"}}}"#);
        assert!(r.get("error").is_none());
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("no project matches"));
    }
}
