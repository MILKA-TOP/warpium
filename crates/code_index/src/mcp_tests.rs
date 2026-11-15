use std::path::Path;

use serde_json::Value;

use super::*;

fn server() -> Server {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("kmp");
    Server::new(root, None)
}

fn call(server: &mut Server, request: Value) -> Value {
    server
        .handle_message(&request.to_string())
        .expect("requests get a response")
}

fn tool_text(server: &mut Server, name: &str, arguments: Value) -> (String, bool) {
    let response = call(
        server,
        json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments },
        }),
    );
    let result = &response["result"];
    (
        result["content"][0]["text"]
            .as_str()
            .expect("text content")
            .to_owned(),
        result["isError"].as_bool().unwrap_or(false),
    )
}

#[test]
fn initialize_echoes_a_known_protocol_version() {
    let response = call(
        &mut server(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-03-26", "capabilities": {} },
        }),
    );

    assert_eq!(response["id"], 1);
    assert_eq!(response["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(response["result"]["serverInfo"]["name"], "warpium-code-index");
}

#[test]
fn notifications_get_no_response() {
    let response = server().handle_message(
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
    );

    assert_eq!(response, None);
}

#[test]
fn lists_the_tools() {
    let response = call(
        &mut server(),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    );

    let names: Vec<&str> = response["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect();
    assert_eq!(names, vec!["find_class", "find_symbol", "file_outline"]);
}

#[test]
fn unknown_methods_are_errors() {
    let response = call(
        &mut server(),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/list" }),
    );

    assert_eq!(response["error"]["code"], -32601);
}

#[test]
fn find_class_lists_matches_with_expect_actual_counterparts() {
    let (text, is_error) = tool_text(
        &mut server(),
        "find_class",
        json!({ "query": "Platform", "limit": 1 }),
    );

    assert!(!is_error);
    assert_eq!(
        text,
        "class Platform  com.example.shared  \
         shared/src/androidMain/kotlin/com/example/shared/Platform.android.kt:3  \
         [androidMain, actual]\n    \
         expect in shared/src/commonMain/kotlin/com/example/shared/Platform.kt:6\n    \
         actual in shared/src/iosMain/kotlin/com/example/shared/Platform.ios.kt:3"
    );
}

#[test]
fn find_symbol_filters_by_kind() {
    let (text, _) = tool_text(
        &mut server(),
        "find_symbol",
        json!({ "query": "shout", "kind": "function" }),
    );

    assert_eq!(
        text,
        "fun String.shout  com.example.shared  \
         shared/src/commonMain/kotlin/com/example/shared/Platform.kt:29  [commonMain]"
    );
}

#[test]
fn file_outline_lists_header_and_nested_declarations() {
    let (text, _) = tool_text(
        &mut server(),
        "file_outline",
        json!({ "path": "shared/src/androidMain/kotlin/com/example/shared/Platform.android.kt" }),
    );

    assert_eq!(
        text,
        "package com.example.shared\nclass Platform :3\n  property name :4\nfun platformName :7"
    );
}

#[test]
fn missing_arguments_are_tool_errors() {
    let (text, is_error) = tool_text(&mut server(), "find_class", json!({}));

    assert!(is_error);
    assert_eq!(text, "Missing required argument: query");
}
