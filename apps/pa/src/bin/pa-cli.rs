//! `pa-cli`: ProjectArchitect on the command line (D-47). Every MCP tool is a command, run on the same `pa.db` as
//! the GUI through the same tool table (`pa_mcp::tools`). A console program: `pa.exe` is a GUI-subsystem exe on
//! Windows, which terminals neither wait for nor print from.

use pa_core::Store;
use pa_mcp::{Server, tools};
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: pa-cli [--data-dir <folder>] <command>

  tools [<tool>]              list the tools, or one tool's parameters
  <tool> [key=value ...]      run a tool; values follow the tool's parameter types,
                              lists as a,b or JSON; a required `project` defaults to
                              the current folder
  <tool> --json '<object>'    run a tool with JSON arguments
  install-mcp [--to <file>]   put the pa-mcp next to pa-cli at the registered path
                              (default %LOCALAPPDATA%\\Programs\\ProjectArchitect),
                              setting a running copy aside

The result is JSON on stdout, a one-line summary on stderr.
Exit codes: 0 done, 1 the tool failed, 2 bad usage or arguments.";

/// `println!` that stops quietly when the reader has gone (`pa-cli tools | head`) instead of panicking.
macro_rules! out {
    ($($t:tt)*) => {{
        use std::io::Write;
        if writeln!(std::io::stdout(), $($t)*).is_err() {
            std::process::exit(0);
        }
    }};
}

fn main() -> ExitCode {
    pa_core::install::remove_old_self();
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err((code, message)) => {
            eprintln!("pa-cli: {message}");
            ExitCode::from(code)
        }
    }
}

type Error = (u8, String);

fn usage(message: impl Into<String>) -> Error {
    (2, message.into())
}

fn run(mut args: Vec<String>) -> Result<(), Error> {
    let mut data_dir = pa_core::default_data_dir();
    if args.first().is_some_and(|a| a == "--data-dir") {
        let dir = args.get(1).ok_or_else(|| usage("--data-dir needs a folder"))?;
        data_dir = PathBuf::from(dir);
        args.drain(..2);
    }
    let Some(command) = args.first().cloned() else { return Err(usage(USAGE)) };
    let rest = &args[1..];
    let list = tools::list();
    match command.as_str() {
        "help" | "--help" | "-h" => {
            out!("{USAGE}");
            Ok(())
        }
        "tools" => match rest.first() {
            None => {
                for t in &list {
                    let first = t["description"].as_str().unwrap_or_default().split(". ").next().unwrap_or_default();
                    out!("{:<26} {first}", t["name"].as_str().unwrap_or_default());
                }
                Ok(())
            }
            Some(name) => {
                out!("{}", describe(find(&list, name)?));
                Ok(())
            }
        },
        "install-mcp" => install_mcp(rest),
        name => {
            let tool = find(&list, name)?;
            let mut arguments = arguments(tool, rest)?;
            let schema = &tool["inputSchema"];
            let needs_project = schema["required"].as_array().is_some_and(|r| r.iter().any(|k| k == "project"));
            if needs_project && !arguments.contains_key("project") {
                let here = std::env::current_dir().map_err(|e| (1, e.to_string()))?;
                arguments.insert("project".into(), Value::String(here.display().to_string()));
            }
            let db = data_dir.join("pa.db");
            let store = Store::open(&db).map_err(|e| (1, format!("cannot open {}: {e}", db.display())))?;
            let server = Server { store, data_dir };
            match tools::call(&server, name, &arguments) {
                Ok(out) => {
                    out!("{}", serde_json::to_string_pretty(&out.data).unwrap_or_default());
                    eprintln!("{}", out.summary);
                    Ok(())
                }
                Err(tools::Failure::Tool(message)) => Err((1, message)),
                Err(tools::Failure::Params(message)) => Err(usage(format!("{message} (pa-cli tools {name})"))),
            }
        }
    }
}

fn find<'a>(list: &'a [Value], name: &str) -> Result<&'a Value, Error> {
    list.iter().find(|t| t["name"] == name).ok_or_else(|| usage(format!("unknown command {name:?}; `pa-cli tools` lists them")))
}

/// A tool's description and parameters, `*` marking the required ones.
fn describe(tool: &Value) -> String {
    let schema = &tool["inputSchema"];
    let required = |k: &str| schema["required"].as_array().is_some_and(|r| r.iter().any(|x| x == k));
    let mut s = format!("{}\n\n{}\n", tool["name"].as_str().unwrap_or_default(), tool["description"].as_str().unwrap_or_default());
    if let Some(props) = schema["properties"].as_object() {
        s.push('\n');
        for (k, p) in props {
            let ty = match &p["type"] {
                Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("|"),
                t => t.as_str().unwrap_or("any").to_string(),
            };
            let values = p["enum"].as_array().map(|e| format!(" ({})", e.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")));
            let mark = if required(k) { "*" } else { " " };
            s.push_str(&format!("  {mark}{k:<18} {ty:<16} {}{}\n", p["description"].as_str().unwrap_or_default(), values.unwrap_or_default()));
        }
    }
    s
}

/// `key=value` pairs (typed by the schema) or `--json <object>` into a tool's arguments.
fn arguments(tool: &Value, rest: &[String]) -> Result<Map<String, Value>, Error> {
    let name = tool["name"].as_str().unwrap_or_default();
    if rest.first().is_some_and(|a| a == "--json") {
        let [_, json] = rest else { return Err(usage("--json takes one JSON object")) };
        return match serde_json::from_str(json) {
            Ok(Value::Object(map)) => Ok(map),
            _ => Err(usage("--json needs a JSON object, e.g. '{\"project\": \"name\"}'")),
        };
    }
    let props = tool["inputSchema"]["properties"].as_object().cloned().unwrap_or_default();
    let mut map = Map::new();
    for pair in rest {
        let (k, raw) = pair.split_once('=').ok_or_else(|| usage(format!("expected key=value, got {pair:?} (pa-cli tools {name})")))?;
        let schema = props.get(k).ok_or_else(|| {
            usage(format!("{name} has no parameter {k:?}; it takes {}", props.keys().cloned().collect::<Vec<_>>().join(", ")))
        })?;
        map.insert(k.to_string(), typed(schema, raw));
    }
    Ok(map)
}

/// A command-line value as the parameter's JSON type: `5` is a number only where the schema takes integers, and
/// anything that is not valid for the schema stays a string (the tool then names the problem).
fn typed(schema: &Value, raw: &str) -> Value {
    let types: Vec<&str> = match &schema["type"] {
        Value::String(t) => vec![t.as_str()],
        Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        let ty = match &v {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(n) if n.is_i64() => "integer",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        if ty != "string" && types.contains(&ty) {
            return v;
        }
    }
    if types.contains(&"array") {
        return Value::Array(raw.split(',').map(|s| Value::String(s.trim().to_string())).filter(|v| v != "").collect());
    }
    Value::String(raw.to_string())
}

fn install_mcp(rest: &[String]) -> Result<(), Error> {
    let name = format!("pa-mcp{}", std::env::consts::EXE_SUFFIX);
    let dest = match rest {
        [] => pa_core::install::default_dir().map(|d| d.join(&name)).ok_or_else(|| usage("no default install folder here; pass --to <file>"))?,
        [flag, to] if flag == "--to" => PathBuf::from(to),
        _ => return Err(usage("install-mcp [--to <file>]")),
    };
    let exe = std::env::current_exe().map_err(|e| (1, e.to_string()))?;
    let src = exe.with_file_name(&name);
    if !src.is_file() {
        return Err((1, format!("{} not found; build it first (cargo build --release -p pa-mcp)", src.display())));
    }
    pa_core::install::replace(&src, &dest).map_err(|e| (1, format!("cannot install {}: {e}", dest.display())))?;
    eprintln!("installed {} (sessions already running keep the old copy until they restart)", dest.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn values_follow_the_parameter_type() {
        let s = |t: Value| json!({"type": t});
        assert_eq!(typed(&s(json!("string")), "12"), json!("12"));
        assert_eq!(typed(&s(json!("integer")), "12"), json!(12));
        assert_eq!(typed(&s(json!(["integer", "string"])), "11"), json!(11));
        assert_eq!(typed(&s(json!(["integer", "string"])), "ProjectArchitect"), json!("ProjectArchitect"));
        assert_eq!(typed(&s(json!(["string", "null"])), "null"), Value::Null);
        assert_eq!(typed(&s(json!(["string", "null"])), "42"), json!("42"));
        assert_eq!(typed(&s(json!("boolean")), "true"), json!(true));
        assert_eq!(typed(&s(json!("array")), "a, b,"), json!(["a", "b"]));
        assert_eq!(typed(&s(json!("array")), r#"["a,b"]"#), json!(["a,b"]));
    }

    #[test]
    fn every_tool_is_a_command_and_unknown_keys_are_refused() {
        let list = tools::list();
        let capture = find(&list, "capture").unwrap();
        let args = arguments(capture, &["kind=Idea".into(), "title=Try the CLI".into()]).unwrap();
        assert_eq!(Value::Object(args), json!({"kind": "Idea", "title": "Try the CLI"}));
        assert_eq!(arguments(capture, &["colour=red".into()]).unwrap_err().0, 2);
        assert_eq!(arguments(capture, &["--json".into(), r#"{"title": "x"}"#.into()]).unwrap()["title"], "x");
        assert!(find(&list, "no_such_tool").is_err());
        assert!(describe(capture).contains("*title"));
    }
}
