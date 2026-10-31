//! Wire-level contract test.
//!
//! Everything here exercises the real server binary over stdio JSON-RPC, which
//! is the only layer that catches the class of bug that shipped in 1.0.3: the
//! unit test for `get_info()` proves the capability is set in code, but only a
//! real handshake proves the model would ever see the tools.
//!
//! Runs on every platform in CI; no indexer service or Everything install is
//! required, because engine acquisition happens on the first tool call.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn request(id: u32, method: &str, params: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#)
}

#[test]
fn initialize_advertises_tools_and_tools_list_returns_the_five() {
    let exe = env!("CARGO_BIN_EXE_instant-file-search-mcp-server");
    let mut child = Command::new(exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("server binary should start");

    let stdout = child.stdout.take().expect("stdout is piped");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        while reader.read_line(&mut line).map(|n| n > 0).unwrap_or(false) {
            let _ = tx.send(std::mem::take(&mut line));
        }
    });

    let mut stdin = child.stdin.take().expect("stdin is piped");
    writeln!(
        stdin,
        "{}",
        request(
            1,
            "initialize",
            r#"{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"mcp-handshake-test","version":"1"}}"#
        )
    )
    .unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#).unwrap();
    writeln!(stdin, "{}", request(2, "tools/list", "{}")).unwrap();
    stdin.flush().unwrap();

    let mut init = None;
    let mut listed = None;
    for _ in 0..20 {
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(line) => {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
                    continue;
                };
                match value.get("id").and_then(|id| id.as_u64()) {
                    Some(1) => init = Some(value),
                    Some(2) => listed = Some(value),
                    _ => {}
                }
            }
            Err(_) => break,
        }
        if init.is_some() && listed.is_some() {
            break;
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    let init = init.expect("initialize must answer over stdio");
    let capabilities = &init["result"]["capabilities"];
    assert!(
        capabilities.get("tools").map(|t| t.is_object()).unwrap_or(false),
        "initialize must advertise capabilities.tools; strict clients skip tools/list without it: {capabilities}"
    );
    assert_eq!(init["result"]["serverInfo"]["name"], "instant-file-search");

    let listed = listed.expect("tools/list must answer over stdio");
    let mut names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .expect("tools/list returns an array")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "aggregate_files",
            "count_files",
            "find_files",
            "recent_changes",
            "search_status",
        ]
    );
}

#[test]
fn self_test_reports_the_contract_without_a_transport() {
    let exe = env!("CARGO_BIN_EXE_instant-file-search-mcp-server");
    let output = Command::new(exe)
        .arg("--self-test")
        .output()
        .expect("server binary should start");
    assert!(output.status.success(), "--self-test should exit 0");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("initialize.capabilities.tools: true"),
        "--self-test output: {text}"
    );
    assert!(text.contains("find_files"), "--self-test output: {text}");
}
