//! End to end: the built binary, a short MCP session over stdin and stdout, a temp data folder.

use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn session_initialize_list_and_call() {
    let data_dir = std::env::temp_dir().join(format!("pa-mcp-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
    let requests = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_projects","arguments":{}}}"#,
    ];
    let mut child = Command::new(env!("CARGO_BIN_EXE_pa-mcp"))
        .arg("--data-dir")
        .arg(&data_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pa-mcp");
    {
        let mut stdin = child.stdin.take().expect("stdin");
        for r in requests {
            writeln!(stdin, "{r}").unwrap();
        }
    }
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));

    // Only replies on stdout: the notification gets none, and nothing else is printed.
    let lines: Vec<Value> = String::from_utf8(output.stdout).unwrap().lines().map(|l| serde_json::from_str(l).expect("json line")).collect();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(lines[0]["result"]["protocolVersion"], "2025-06-18");
    assert!(lines[1]["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "list_projects"));
    assert_eq!(lines[2]["result"]["isError"], false);
    assert_eq!(lines[2]["result"]["structuredContent"]["projects"], serde_json::json!([]));
    let _ = std::fs::remove_dir_all(&data_dir);
}
