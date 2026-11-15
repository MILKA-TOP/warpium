//! A minimal Model Context Protocol server over stdio, so AI agents can query the index (as a
//! candidate replacement for `ast-index`). Messages are newline-delimited JSON-RPC 2.0.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Duration;

use instant::Instant;
use serde_json::{Value, json};

use crate::index::{CodeIndex, SearchScope, SymbolInfo, default_threads};
use crate::model::{Language, Position, SymbolKind, flags};
use crate::resolve::SourceFile;

/// Protocol version answered when the client doesn't ask for one we know.
const LATEST_PROTOCOL_VERSION: &str = "2025-06-18";
const KNOWN_PROTOCOL_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

/// Files are rescanned before a tool call when the last scan is older than this.
const REFRESH_AFTER: Duration = Duration::from_secs(2);
const DEFAULT_LIMIT: usize = 30;
const MAX_LIMIT: usize = 200;
const DEFAULT_USAGES_LIMIT: usize = 100;
const MAX_USAGES_LIMIT: usize = 1000;

pub struct Server {
    root: PathBuf,
    snapshot: Option<PathBuf>,
    index: Option<CodeIndex>,
    last_refresh: Option<Instant>,
}

impl Server {
    pub fn new(root: PathBuf, snapshot: Option<PathBuf>) -> Self {
        Self {
            root,
            snapshot,
            index: None,
            last_refresh: None,
        }
    }

    /// Serves requests from `input` until it closes.
    pub fn serve(&mut self, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Some(response) = self.handle_message(&line) {
                serde_json::to_writer(&mut output, &response)?;
                output.write_all(b"\n")?;
                output.flush()?;
            }
        }
        Ok(())
    }

    /// Handles one JSON-RPC message and returns the response, or `None` for notifications.
    pub fn handle_message(&mut self, line: &str) -> Option<Value> {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(_) => return Some(error_response(Value::Null, -32700, "Parse error")),
        };
        let id = message.get("id").cloned()?;
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => Ok(initialize_result(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_definitions() })),
            "tools/call" => Ok(self.call_tool(&params)),
            _ => Err((-32601, "Method not found")),
        };
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => error_response(id, code, message),
        })
    }

    fn call_tool(&mut self, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
        let text_argument = |key: &str| arguments.get(key).and_then(Value::as_str);
        let requested_limit = arguments.get("limit").and_then(Value::as_u64);
        let limit =
            requested_limit.map_or(DEFAULT_LIMIT, |limit| (limit as usize).clamp(1, MAX_LIMIT));

        let result = match name {
            "find_class" | "find_symbol" => match text_argument("query") {
                Some(query) => {
                    let scope = if name == "find_class" {
                        SearchScope::Types
                    } else {
                        SearchScope::All
                    };
                    let kind = text_argument("kind");
                    Ok(self.find(query, scope, kind, limit))
                }
                None => Err("Missing required argument: query".to_owned()),
            },
            "file_outline" => match text_argument("path") {
                Some(path) => self.outline(path),
                None => Err("Missing required argument: path".to_owned()),
            },
            "find_definition" => self.definition(&arguments),
            "find_usages" => {
                let limit = requested_limit.map_or(DEFAULT_USAGES_LIMIT, |limit| {
                    (limit as usize).clamp(1, MAX_USAGES_LIMIT)
                });
                self.usages(&arguments, limit)
            }
            _ => Err(format!("Unknown tool: {name}")),
        };
        match result {
            Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
            Err(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": true }),
        }
    }

    fn index(&mut self) -> &CodeIndex {
        let stale = self
            .last_refresh
            .is_none_or(|refreshed| refreshed.elapsed() >= REFRESH_AFTER);
        if stale {
            let threads = default_threads();
            let changed = match &mut self.index {
                Some(index) => index.refresh(threads) != Default::default(),
                None => {
                    let (index, report) =
                        crate::open(&self.root, self.snapshot.as_deref(), threads);
                    self.index = Some(index);
                    report.update != Default::default()
                }
            };
            if changed
                && let (Some(index), Some(snapshot)) = (&self.index, &self.snapshot)
                && let Err(err) = index.save(snapshot)
            {
                log::warn!("[Code index] Couldn't save the index snapshot: {err}");
            }
            self.last_refresh = Some(Instant::now());
        }
        self.index.as_ref().expect("index was opened above")
    }

    fn find(
        &mut self,
        query: &str,
        scope: SearchScope,
        kind: Option<&str>,
        limit: usize,
    ) -> String {
        let index = self.index();
        // Kind filtering happens after ranking, so ask for more and trim.
        let fetch = if kind.is_some() { MAX_LIMIT * 5 } else { limit };
        let results: Vec<SymbolInfo> = index
            .search(query, scope, fetch)
            .into_iter()
            .map(|result| result.symbol)
            .filter(|symbol| kind.is_none_or(|kind| kind_matches(symbol.kind, kind)))
            .take(limit)
            .collect();
        if results.is_empty() {
            return format!("No declarations match \"{query}\".");
        }
        results
            .iter()
            .map(|symbol| describe(symbol, index))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn rel_path(&self, path: &str) -> String {
        std::path::Path::new(path)
            .strip_prefix(&self.root)
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.trim_start_matches("./").to_owned())
    }

    /// Reads the file a position argument names and finds the position: `line` (1-based) and
    /// either `column` (1-based characters) or `name`, an identifier on that line.
    fn location(&self, arguments: &Value) -> Result<(String, String, Language, Position), String> {
        let path = arguments
            .get("path")
            .and_then(Value::as_str)
            .ok_or("Missing required argument: path")?;
        let line = arguments
            .get("line")
            .and_then(Value::as_u64)
            .filter(|line| *line >= 1)
            .ok_or("Missing required argument: line (1-based)")?;
        let rel_path = self.rel_path(path);
        let language = Language::of_editable_path(std::path::Path::new(&rel_path))
            .ok_or_else(|| format!("{rel_path} isn't a Kotlin or Java file."))?;
        let text = std::fs::read_to_string(crate::scan::abs_path(&self.root, &rel_path))
            .map_err(|err| format!("Couldn't read {rel_path}: {err}"))?;
        let line_index = (line - 1) as u32;
        let line_text = text
            .lines()
            .nth(line_index as usize)
            .ok_or_else(|| format!("{rel_path} has no line {line}."))?;
        let column = match (
            arguments.get("column").and_then(Value::as_u64),
            arguments.get("name").and_then(Value::as_str),
        ) {
            (Some(column), _) => column.saturating_sub(1) as u32,
            (None, Some(name)) => identifier_column(line_text, name)
                .ok_or_else(|| format!("\"{name}\" isn't on line {line} of {rel_path}."))?,
            (None, None) => return Err("Pass column or name to locate the reference.".to_owned()),
        };
        Ok((
            rel_path,
            text,
            language,
            Position {
                line: line_index,
                column,
            },
        ))
    }

    fn definition(&mut self, arguments: &Value) -> Result<String, String> {
        let (rel_path, text, language, position) = self.location(arguments)?;
        let index = self.index();
        let file = SourceFile {
            rel_path: &rel_path,
            text: &text,
            language,
        };
        let Some(result) = index.find_definition(&file, position) else {
            return Err(format!(
                "No identifier at {rel_path}:{}:{}.",
                position.line + 1,
                position.column + 1
            ));
        };
        if result.definitions.is_empty() {
            return Ok(format!(
                "No declaration of \"{}\" found in the project (it may come from a library).",
                result.name
            ));
        }
        let mut lines = Vec::new();
        if result.at_declaration {
            lines.push(format!(
                "\"{}\" is declared here; use find_usages for its usages.",
                result.name
            ));
        }
        for definition in &result.definitions {
            let mut line = describe_location(&definition.symbol);
            let mut tags = vec![definition.resolution.label().to_owned()];
            if definition.local {
                tags.push("local".to_owned());
            }
            if definition.counterpart {
                tags.push("counterpart".to_owned());
            }
            line.push_str(&format!("  ({})", tags.join(", ")));
            lines.push(line);
        }
        Ok(lines.join("\n"))
    }

    fn usages(&mut self, arguments: &Value, limit: usize) -> Result<String, String> {
        let (rel_path, text, language, position) = self.location(arguments)?;
        let index = self.index();
        let file = SourceFile {
            rel_path: &rel_path,
            text: &text,
            language,
        };
        let Some(result) = index.find_usages(&file, position, limit, default_threads()) else {
            return Err(format!(
                "No identifier at {rel_path}:{}:{}.",
                position.line + 1,
                position.column + 1
            ));
        };
        if result.targets.is_empty() {
            return Ok(format!(
                "No declaration of \"{}\" found in the project, so its usages are unknown.",
                result.name
            ));
        }
        let mut lines = vec![format!(
            "{} usage{} of {}{}:",
            result.usages.len(),
            if result.usages.len() == 1 { "" } else { "s" },
            result
                .targets
                .iter()
                .map(|target| {
                    format!("{} ({}:{})", target.qualified_name(), target.rel_path, target.name_pos.line + 1)
                })
                .collect::<Vec<_>>()
                .join(", "),
            if result.complete { "" } else { " (more not shown)" },
        )];
        for usage in &result.usages {
            lines.push(format!(
                "{}:{}:{}{}  {}",
                usage.rel_path,
                usage.position.line + 1,
                usage.position.column + 1,
                if usage.exact { "" } else { " ≈" },
                usage.line_text
            ));
        }
        if result.usages.iter().any(|usage| !usage.exact) {
            lines.push("≈ the name matches and the declaration is visible there, but the reference wasn't resolved to it.".to_owned());
        }
        Ok(lines.join("\n"))
    }

    fn outline(&mut self, path: &str) -> Result<String, String> {
        let rel_path = self.rel_path(path);
        let index = self.index();
        let Some((package, imports)) = index.file_header(&rel_path) else {
            return Err(format!("{rel_path} isn't an indexed Kotlin or Java file."));
        };
        let mut lines = vec![format!("package {package}")];
        lines.extend(imports.iter().map(|import| format!("import {import}")));
        for symbol in index.file_symbols(&rel_path) {
            let depth =
                symbol.container.matches('.').count() + usize::from(!symbol.container.is_empty());
            lines.push(format!(
                "{}{} {}{} :{}",
                "  ".repeat(depth),
                symbol.kind.label(),
                receiver_prefix(&symbol),
                symbol.name,
                symbol.name_pos.line + 1
            ));
        }
        Ok(lines.join("\n"))
    }
}

fn kind_matches(kind: SymbolKind, wanted: &str) -> bool {
    let wanted = wanted.to_ascii_lowercase();
    match wanted.as_str() {
        "function" | "fun" | "method" => kind == SymbolKind::Function,
        "property" | "field" | "val" | "var" => kind == SymbolKind::Property,
        "type" => kind.is_type(),
        _ => kind.label().replace(' ', "_") == wanted.replace(' ', "_"),
    }
}

fn receiver_prefix(symbol: &SymbolInfo) -> String {
    symbol
        .receiver
        .as_ref()
        .map(|receiver| format!("{receiver}."))
        .unwrap_or_default()
}

/// One result line: `class UserViewModel  com.app.feature  feature/src/main/…/X.kt:12  [main]`.
fn describe(symbol: &SymbolInfo, index: &CodeIndex) -> String {
    let mut tags = Vec::new();
    if !symbol.source_set.is_empty() {
        tags.push(symbol.source_set.clone());
    }
    if symbol.has_flag(flags::EXPECT) {
        tags.push("expect".to_owned());
    }
    if symbol.has_flag(flags::ACTUAL) {
        tags.push("actual".to_owned());
    }
    if symbol.origin == crate::Origin::Generated {
        tags.push("generated".to_owned());
    }
    let mut line = format!(
        "{} {}{}  {}  {}:{}",
        symbol.kind.label(),
        receiver_prefix(symbol),
        symbol.name,
        symbol.location(),
        symbol.rel_path,
        symbol.name_pos.line + 1
    );
    if !tags.is_empty() {
        line.push_str(&format!("  [{}]", tags.join(", ")));
    }
    for counterpart in index.expect_actual_counterparts(symbol) {
        line.push_str(&format!(
            "\n    {} in {}:{}",
            if counterpart.has_flag(flags::EXPECT) {
                "expect"
            } else {
                "actual"
            },
            counterpart.rel_path,
            counterpart.name_pos.line + 1
        ));
    }
    line
}

/// `kind name  location  path:line:column  [source set]`.
fn describe_location(symbol: &SymbolInfo) -> String {
    let mut line = format!(
        "{} {}{}  {}  {}:{}:{}",
        symbol.kind.label(),
        receiver_prefix(symbol),
        symbol.name,
        symbol.location(),
        symbol.rel_path,
        symbol.name_pos.line + 1,
        symbol.name_pos.column + 1
    );
    if !symbol.source_set.is_empty() {
        line.push_str(&format!("  [{}]", symbol.source_set));
    }
    line
}

/// The character column where `name` occurs on `line` as a whole identifier.
fn identifier_column(line: &str, name: &str) -> Option<u32> {
    let is_identifier_char = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    line.match_indices(name).find_map(|(start, _)| {
        let before = line[..start].chars().next_back();
        let after = line[start + name.len()..].chars().next();
        (!before.is_some_and(is_identifier_char) && !after.is_some_and(is_identifier_char))
            .then(|| line[..start].chars().count() as u32)
    })
}

fn initialize_result(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|version| KNOWN_PROTOCOL_VERSIONS.contains(version))
        .unwrap_or(LATEST_PROTOCOL_VERSION);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "warpium-code-index", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Kotlin and Java declaration index of the project. Use find_class and \
            find_symbol (fuzzy, camel humps: \"UVM\" finds UserViewModel; \"pkg.Name\" narrows by \
            package or enclosing class) instead of grep to locate declarations, file_outline \
            to list a file's declarations, find_definition to jump from a reference to its \
            declaration, and find_usages to list where a declaration is used.",
    })
}

fn tool_definitions() -> Value {
    let limit = json!({
        "type": "integer",
        "description": format!("Maximum results (default {DEFAULT_LIMIT}, at most {MAX_LIMIT})."),
    });
    let position = |what: &str| {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path, absolute or relative to the project root (.kt, .kts or .java)." },
                "line": { "type": "integer", "description": "1-based line of the reference." },
                "column": { "type": "integer", "description": format!("1-based character column of {what}. Optional when name is given.") },
                "name": { "type": "string", "description": "The identifier on that line, instead of a column (its first whole-word occurrence)." },
                "limit": { "type": "integer", "description": format!("Maximum usages (default {DEFAULT_USAGES_LIMIT}, at most {MAX_USAGES_LIMIT}); find_usages only.") },
            },
            "required": ["path", "line"],
        })
    };
    json!([
        {
            "name": "find_class",
            "description": "Find Kotlin/Java classes, interfaces, objects, enums, annotations, \
                records and type aliases by name. Fuzzy with camel humps (\"UVM\" → \
                UserViewModel); \"pkg.Name\" narrows by package. Returns kind, name, package, \
                file:line and source set; expect/actual counterparts are listed under each hit.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Name or camel-hump abbreviation." },
                    "limit": limit,
                },
                "required": ["query"],
            },
        },
        {
            "name": "find_symbol",
            "description": "Find any Kotlin/Java declaration (types, functions including \
                extension functions, properties, constructors, enum entries) by name. Same \
                matching as find_class; \"Receiver.name\" finds extension functions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Name or camel-hump abbreviation." },
                    "kind": {
                        "type": "string",
                        "description": "Optional filter: class, interface, object, enum, \
                            typealias, function, property, constructor, enum_entry or type.",
                    },
                    "limit": limit,
                },
                "required": ["query"],
            },
        },
        {
            "name": "file_outline",
            "description": "List the package, imports and declarations (with line numbers) of \
                a Kotlin or Java file in the project.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path, absolute or relative to the project root." },
                },
                "required": ["path"],
            },
        },
        {
            "name": "find_definition",
            "description": "Go to definition for a Kotlin/Java reference: the declaration(s) a \
                name at path:line refers to, resolved through local scopes, imports, the package \
                and declared receiver types (no compiler, so ambiguous members list every \
                candidate, marked \"name only\"). expect/actual counterparts are listed after \
                the definition.",
            "inputSchema": position("the reference"),
        },
        {
            "name": "find_usages",
            "description": "Find usages of the Kotlin/Java declaration a name at path:line \
                declares or refers to, across the project (imports, type references, calls; not \
                comments or strings). Usages that only match by name are marked ≈.",
            "inputSchema": position("the name"),
        },
    ])
}

fn error_response(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
