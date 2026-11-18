//! The MCP server: newline-delimited JSON-RPC 2.0 over stdio, like `warpium-code-index mcp`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Local;
use code_index::{CodeIndex, default_snapshot_path, default_threads};
use instant::Instant;
use serde_json::{Value, json};
use warpium_android::AdbClient;
use warpium_android::device::{DeviceEntry, DeviceKind, read_props};
use warpium_android::logcat::process::process_belongs_to;
use warpium_android::logcat::{LogEntry, LogFilter, LogLevel, LogTime, TagFilter, TextFilter};
use warpium_android::stack::{StackFrame, choose_source_file, find_source_file, parse_frame};

use crate::capture::{Capture, CaptureConfig, PidRanges, State};
use crate::crash::{Crash, extract_crashes};
use crate::since::{MARK_PREFIX, Since, parse_since};

/// Protocol version answered when the client doesn't ask for one we know.
const LATEST_PROTOCOL_VERSION: &str = "2025-06-18";
const KNOWN_PROTOCOL_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 2000;
/// Text returned by one call at most, so a chatty app can't flood the agent's context.
const MAX_OUTPUT_BYTES: usize = 200 * 1024;
/// Bytes of one message shown at most.
const MAX_MESSAGE_BYTES: usize = 8 * 1024;
/// The longest line of a crash report shown.
const MAX_CRASH_LINE_BYTES: usize = 1024;
const DEFAULT_CRASH_LIMIT: usize = 5;
const MAX_CRASH_LIMIT: usize = 20;
const DEFAULT_WAIT_SECS: f64 = 30.0;
const MAX_WAIT_SECS: f64 = 120.0;
const DEFAULT_CONTEXT: usize = 10;
const MAX_CONTEXT: usize = 50;
/// Lines after a `wait_for_log` hit returned at most, and how long to wait for them.
const AFTER_CONTEXT: usize = 5;
const AFTER_CONTEXT_WAIT: Duration = Duration::from_millis(300);
/// The code index is rescanned before resolving frames when the last scan is older than this.
const INDEX_REFRESH_AFTER: Duration = Duration::from_secs(2);
/// Project walks for frames the code index can't place, per call.
const MAX_SOURCE_WALKS: usize = 10;
/// Frames of these packages are never in the project.
const FRAMEWORK_PREFIXES: &[&str] = &[
    "java.",
    "javax.",
    "jdk.",
    "sun.",
    "android.",
    "androidx.",
    "com.android.",
    "com.google.android.",
    "dalvik.",
    "libcore.",
    "kotlin.",
    "kotlinx.",
];

/// How the server runs.
#[derive(Clone, Debug)]
pub struct ServerOptions {
    /// The project, for resolving stack frames to files.
    pub root: Option<PathBuf>,
    /// The device used when a call names none and several are online (`ANDROID_SERIAL`).
    pub default_serial: Option<String>,
    /// Entries kept per device.
    pub capacity: usize,
    /// Recent entries read when a device's capture starts.
    pub backlog: usize,
    /// A capture's backlog counts as loaded once logcat has been quiet this long.
    pub settle: Duration,
    /// The longest a capture waits for its backlog.
    pub settle_max: Duration,
    /// How often `wait_for_log` asks the device for the package's processes.
    pub process_poll: Duration,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            root: None,
            default_serial: None,
            capacity: 100_000,
            backlog: 10_000,
            settle: Duration::from_millis(200),
            settle_max: Duration::from_secs(3),
            process_poll: Duration::from_secs(2),
        }
    }
}

pub struct Server {
    client: AdbClient,
    options: ServerOptions,
    captures: HashMap<String, Capture>,
    index: Option<LoadedIndex>,
}

struct LoadedIndex {
    root: PathBuf,
    index: CodeIndex,
    refreshed: Instant,
}

/// Whose logs a call reads.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Scope {
    Package(String),
    AllPackages,
}

impl Scope {
    fn from_arguments(arguments: &Value) -> Result<Self, String> {
        match text_argument(arguments, "package") {
            Some(package) => Ok(Scope::Package(package.to_owned())),
            None if arguments.get("all_packages").and_then(Value::as_bool) == Some(true) => {
                Ok(Scope::AllPackages)
            }
            None => Err(
                "Pass package (the app's application id, such as com.example.app). \
                 Logs of other apps and the system may hold personal data, so reading every \
                 package needs all_packages: true explicitly."
                    .to_owned(),
            ),
        }
    }

    /// The pids whose entries are in scope; `None` for every package.
    fn pids(&self, state: &State) -> Option<PidRanges> {
        match self {
            Scope::Package(package) => Some(state.pids_of(package)),
            Scope::AllPackages => None,
        }
    }

    fn label(&self) -> String {
        match self {
            Scope::Package(package) => package.clone(),
            Scope::AllPackages => "all packages".to_owned(),
        }
    }
}

/// The level, tag and text filters of a call.
struct Filters {
    filter: LogFilter,
    regex: TextFilter,
}

impl Filters {
    fn from_arguments(arguments: &Value) -> Result<Self, String> {
        let min_level = match text_argument(arguments, "min_level") {
            Some(level) => parse_level(level)?,
            None => LogLevel::Verbose,
        };
        let tags = TagFilter::parse(text_argument(arguments, "tag").unwrap_or_default());
        let text = TextFilter::parse(text_argument(arguments, "text").unwrap_or_default(), false)
            .map_err(|err| err.to_string())?;
        let regex = TextFilter::parse(text_argument(arguments, "regex").unwrap_or_default(), true)
            .map_err(|err| format!("Invalid regex: {err}"))?;
        Ok(Self {
            filter: LogFilter {
                min_level,
                tags,
                text,
            },
            regex,
        })
    }

    fn has_text(&self) -> bool {
        !self.filter.text.is_empty() || !self.regex.is_empty()
    }

    fn matches(&self, entry: &LogEntry) -> bool {
        self.filter.matches(entry, None) && self.regex.matches(entry)
    }
}

fn in_scope(pids: &Option<PidRanges>, seq: u64, entry: &LogEntry) -> bool {
    pids.as_ref()
        .is_none_or(|pids| pids.contains(seq, entry.pid))
}

fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

fn in_since(since: Option<Since>, seq: u64, entry: &LogEntry) -> bool {
    match since {
        None => true,
        Some(Since::Seq(start)) => seq >= start,
        Some(Since::Time(start)) => entry.time >= start,
    }
}

impl Server {
    pub fn new(client: AdbClient, options: ServerOptions) -> Self {
        Self {
            client,
            options,
            captures: HashMap::new(),
            index: None,
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
        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
        let result = match name {
            "list_devices" => self.list_devices(),
            "get_logs" => self.get_logs(&arguments),
            "get_crashes" => self.get_crashes(&arguments),
            "wait_for_log" => self.wait_for_log(&arguments),
            "clear_logs" => self.clear_logs(&arguments),
            _ => Err(format!("Unknown tool: {name}")),
        };
        match result {
            Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
            Err(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": true }),
        }
    }

    fn devices(&self) -> Result<Vec<DeviceEntry>, String> {
        self.client
            .devices()
            .map_err(|err| format!("Can't reach the adb server: {err}"))
    }

    fn list_devices(&mut self) -> Result<String, String> {
        let devices = self.devices()?;
        if devices.is_empty() {
            return Ok(
                "No devices. Start an emulator or connect a device with USB debugging on."
                    .to_owned(),
            );
        }
        let default = default_serial(&devices, self.options.default_serial.as_deref()).ok();
        let lines: Vec<String> = devices
            .iter()
            .map(|device| {
                let kind = match device.kind {
                    DeviceKind::Emulator => "emulator",
                    DeviceKind::Usb => "usb",
                    DeviceKind::Wifi => "wifi",
                };
                let mut line = format!("{}  {kind}  {}", device.serial, device.state.label());
                let props = device
                    .state
                    .is_online()
                    .then(|| read_props(&self.client, &device.serial).ok())
                    .flatten();
                let model = props
                    .as_ref()
                    .and_then(|props| props.avd_name.clone().or_else(|| props.model.clone()))
                    .or_else(|| device.model.clone());
                if let Some(model) = model {
                    line.push_str(&format!("  {}", model.replace('_', " ")));
                }
                if let Some(props) = &props {
                    match (props.api_level, &props.release) {
                        (Some(api), Some(release)) => {
                            line.push_str(&format!("  API {api} (Android {release})"))
                        }
                        (Some(api), None) => line.push_str(&format!("  API {api}")),
                        (None, _) => {}
                    }
                }
                if default.as_deref() == Some(device.serial.as_str()) {
                    line.push_str("  [default]");
                }
                line
            })
            .collect();
        Ok(lines.join("\n"))
    }

    /// The device a call works on: its `device` argument, `ANDROID_SERIAL`, or the only online
    /// device.
    fn resolve_device(&self, arguments: &Value) -> Result<String, String> {
        let devices = self.devices()?;
        match text_argument(arguments, "device") {
            Some(serial) => match devices.iter().find(|device| device.serial == serial) {
                Some(device) if device.state.is_online() => Ok(serial.to_owned()),
                Some(device) => Err(format!(
                    "Device {serial} is {}. {}",
                    device.state.label(),
                    describe_devices(&devices)
                )),
                None => Err(format!(
                    "No device {serial}. {}",
                    describe_devices(&devices)
                )),
            },
            None => default_serial(&devices, self.options.default_serial.as_deref()),
        }
    }

    /// The capture of `serial`, started (or restarted) as needed.
    fn capture(&mut self, serial: &str) -> &mut Capture {
        let capture = self.captures.entry(serial.to_owned()).or_insert_with(|| {
            Capture::start(
                self.client.clone(),
                serial.to_owned(),
                CaptureConfig {
                    capacity: self.options.capacity,
                    backlog: self.options.backlog,
                    settle: self.options.settle,
                    settle_max: self.options.settle_max,
                },
            )
        });
        capture.ensure_running();
        capture
    }

    fn get_logs(&mut self, arguments: &Value) -> Result<String, String> {
        let scope = Scope::from_arguments(arguments)?;
        let filters = Filters::from_arguments(arguments)?;
        let limit = usize_argument(arguments, "limit", DEFAULT_LIMIT, MAX_LIMIT);
        let serial = self.resolve_device(arguments)?;
        let capture = self.capture(&serial);
        if let Scope::Package(package) = &scope {
            capture.refresh_processes(package);
        }
        let since = since_argument(arguments, capture)?;
        let state = capture.lock();
        let pids = scope.pids(&state);
        if pids.as_ref().is_some_and(PidRanges::is_empty) {
            return Ok(package_not_seen(&scope, &serial));
        }

        let mut picked = Vec::new();
        let mut bytes = 0;
        let mut truncated = false;
        for (seq, entry) in state.buffer.entries().rev() {
            if let Some(Since::Seq(start)) = since
                && seq < start
            {
                break;
            }
            if !in_since(since, seq, entry)
                || !in_scope(&pids, seq, entry)
                || !filters.matches(entry)
            {
                continue;
            }
            let text = format_entry(entry);
            if picked.len() == limit || bytes + text.len() > MAX_OUTPUT_BYTES {
                truncated = true;
                break;
            }
            bytes += text.len() + 1;
            picked.push(text);
        }
        picked.reverse();

        let mut lines = vec![format!(
            "{} entr{} from {serial} for {}, newest last. truncated: {truncated}",
            picked.len(),
            if picked.len() == 1 { "y" } else { "ies" },
            scope.label(),
        )];
        if truncated {
            lines.push(
                "Older matching entries were left out; narrow with since, min_level, tag or \
                 text, or raise limit (at most 2000)."
                    .to_owned(),
            );
        }
        if let Some(reason) = &state.ended {
            lines.push(format!(
                "Note: logcat stopped ({reason}); showing what was captured."
            ));
        }
        lines.extend(picked);
        Ok(lines.join("\n"))
    }

    fn clear_logs(&mut self, arguments: &Value) -> Result<String, String> {
        let serial = self.resolve_device(arguments)?;
        let capture = self.capture(&serial);
        let mark = capture.lock().buffer.next_seq();
        capture.mark = Some(mark);
        Ok(format!(
            "Mark set: {MARK_PREFIX}{mark}. get_logs and get_crashes on {serial} now leave out \
             everything logged before this call unless you pass since. The device's log buffers \
             were not cleared."
        ))
    }

    fn wait_for_log(&mut self, arguments: &Value) -> Result<String, String> {
        let scope = Scope::from_arguments(arguments)?;
        let filters = Filters::from_arguments(arguments)?;
        if !filters.has_text() {
            return Err("Pass text or regex: the line to wait for.".to_owned());
        }
        let timeout_secs = arguments
            .get("timeout_s")
            .and_then(Value::as_f64)
            .filter(|secs| secs.is_finite() && *secs >= 0.0)
            .unwrap_or(DEFAULT_WAIT_SECS)
            .min(MAX_WAIT_SECS);
        let context = usize_argument(arguments, "context", DEFAULT_CONTEXT, MAX_CONTEXT);
        let serial = self.resolve_device(arguments)?;
        let process_poll = self.options.process_poll;
        let started = Instant::now();
        let deadline = started + Duration::from_secs_f64(timeout_secs);

        let capture = self.capture(&serial);
        let explicit_since = match text_argument(arguments, "since") {
            Some(text) => Some(parse_since(text, Local::now(), capture.clock_offset_secs)?),
            None => None,
        };
        let start = match explicit_since {
            Some(Since::Seq(seq)) => seq,
            Some(Since::Time(_)) => 0,
            None => capture.lock().buffer.next_seq(),
        };
        // Without since, only lines logged after the call count: on a slow device the replayed
        // backlog may still be arriving behind `start`. Two seconds absorb the clock offset's
        // rounding.
        let time_floor = explicit_since.is_none().then(|| LogTime {
            sec: unix_now_secs() + capture.clock_offset_secs - 2,
            nsec: 0,
        });
        let mut cursor = start;
        let mut known_pids: Option<Option<PidRanges>> = None;
        let mut last_poll: Option<Instant> = None;
        loop {
            if let Scope::Package(package) = &scope
                && last_poll.is_none_or(|polled| polled.elapsed() >= process_poll)
            {
                capture.refresh_processes(package);
                last_poll = Some(Instant::now());
            }
            capture.ensure_running();
            let state = capture.lock();
            let pids = scope.pids(&state);
            // A process logs a few lines before ActivityManager announces it.
            if known_pids.as_ref() != Some(&pids) {
                cursor = start;
                known_pids = Some(pids.clone());
            }
            let hit = state
                .buffer
                .entries_from(cursor)
                .find(|(seq, entry)| {
                    in_since(explicit_since, *seq, entry)
                        && time_floor.is_none_or(|floor| entry.time >= floor)
                        && in_scope(&pids, *seq, entry)
                        && filters.matches(entry)
                })
                .map(|(seq, _)| seq);
            if let Some(hit) = hit {
                drop(state);
                return Ok(wait_result(capture, &scope, hit, context, started));
            }
            cursor = state.buffer.next_seq();
            let now = Instant::now();
            if now >= deadline {
                let note = match &state.ended {
                    Some(reason) => format!(" logcat stopped meanwhile ({reason})."),
                    None => String::new(),
                };
                return Ok(format!(
                    "timed out: true. No matching line from {} appeared on {serial} within \
                     {timeout_secs} s.{note} get_logs shows what was logged.",
                    scope.label()
                ));
            }
            drop(capture.wait(state, (deadline - now).min(process_poll)));
        }
    }

    fn get_crashes(&mut self, arguments: &Value) -> Result<String, String> {
        let scope = Scope::from_arguments(arguments)?;
        let limit = usize_argument(arguments, "limit", DEFAULT_CRASH_LIMIT, MAX_CRASH_LIMIT);
        let root = text_argument(arguments, "root")
            .map(PathBuf::from)
            .or_else(|| self.options.root.clone());
        let serial = self.resolve_device(arguments)?;
        let capture = self.capture(&serial);
        if let Scope::Package(package) = &scope {
            capture.refresh_processes(package);
        }
        let since = since_argument(arguments, capture)?;
        let crashes: Vec<Crash> = {
            let state = capture.lock();
            let entries = state
                .buffer
                .entries()
                .filter(|(seq, entry)| in_since(since, *seq, entry))
                .map(|(_, entry)| entry);
            extract_crashes(entries, &state.processes)
        };
        let mut crashes: Vec<Crash> = crashes
            .into_iter()
            .filter(|crash| match &scope {
                Scope::Package(package) => crash
                    .process
                    .as_deref()
                    .is_some_and(|process| process_belongs_to(process, package)),
                Scope::AllPackages => true,
            })
            .collect();
        if crashes.is_empty() {
            return Ok(format!(
                "No crashes of {} on {serial} in the captured logs{}.",
                scope.label(),
                if since.is_some() { " since then" } else { "" }
            ));
        }
        let total = crashes.len();
        let shown = crashes.split_off(total.saturating_sub(limit));

        let root = root.filter(|root| root.is_dir());
        let index = root.as_deref().map(|root| self.index_for(root));
        let mut resolver = root.as_deref().map(|root| FrameResolver {
            root,
            index,
            cache: HashMap::new(),
            walks_left: MAX_SOURCE_WALKS,
        });

        let mut lines = vec![format!(
            "{total} crash{} of {} on {serial}{}, oldest first{}.",
            if total == 1 { "" } else { "es" },
            scope.label(),
            if since.is_some() { " since then" } else { "" },
            if shown.len() < total {
                format!("; showing the newest {}", shown.len())
            } else {
                String::new()
            },
        )];
        // Built newest first, so capping the output leaves out the oldest crashes; one crash is
        // always shown.
        let mut blocks = Vec::new();
        let mut bytes = 0;
        for (number, crash) in shown.iter().enumerate().rev() {
            let mut block = vec![format!(
                "\n[{}] {} at {} in {}{}",
                number + 1,
                crash.kind.label(),
                crash.time.format_local(),
                crash.process.as_deref().unwrap_or("an unknown process"),
                crash
                    .pid
                    .map(|pid| format!(" (pid {pid})"))
                    .unwrap_or_default(),
            )];
            for line in &crash.lines {
                let location = resolver.as_mut().and_then(|resolver| {
                    parse_frame(line).and_then(|frame| resolver.resolve(&frame))
                });
                let line = truncate(line, MAX_CRASH_LINE_BYTES);
                block.push(match location {
                    Some(location) => format!("{line}  -> {location}"),
                    None => line.into_owned(),
                });
            }
            if crash.omitted_lines > 0 {
                block.push(format!("... {} more lines", crash.omitted_lines));
            }
            let text = block.join("\n");
            if !blocks.is_empty() && bytes + text.len() > MAX_OUTPUT_BYTES {
                lines.push(format!(
                    "(Left out {number_left} older crash{plural} to cap the output.)",
                    number_left = number + 1,
                    plural = if number == 0 { "" } else { "es" },
                ));
                break;
            }
            bytes += text.len();
            blocks.push(text);
        }
        blocks.reverse();
        lines.extend(blocks);
        if root.is_none() {
            lines.push(
                "\nPass root (the project directory) to resolve stack frames to files.".to_owned(),
            );
        }
        Ok(lines.join("\n"))
    }

    fn index_for(&mut self, root: &Path) -> &CodeIndex {
        match &mut self.index {
            Some(loaded) if loaded.root == root => {
                if loaded.refreshed.elapsed() >= INDEX_REFRESH_AFTER {
                    loaded.index.refresh(default_threads());
                    loaded.refreshed = Instant::now();
                }
            }
            _ => {
                let snapshot = default_snapshot_path(root);
                let (index, _) = code_index::open(root, snapshot.as_deref(), default_threads());
                self.index = Some(LoadedIndex {
                    root: root.to_path_buf(),
                    index,
                    refreshed: Instant::now(),
                });
            }
        }
        &self
            .index
            .as_ref()
            .expect("the index was loaded above")
            .index
    }
}

/// `wait_for_log`'s answer: the hit with lines of the same scope around it.
fn wait_result(
    capture: &Capture,
    scope: &Scope,
    hit: u64,
    context: usize,
    started: Instant,
) -> String {
    let waited = started.elapsed().as_secs_f64();
    // Lines that follow the hit (the rest of a stack trace) usually arrive right after it.
    {
        let mut state = capture.lock();
        let deadline = Instant::now() + AFTER_CONTEXT_WAIT;
        loop {
            let now = Instant::now();
            if now >= deadline || state.buffer.next_seq() > hit + AFTER_CONTEXT as u64 * 4 {
                break;
            }
            state = capture.wait(state, deadline - now);
        }
    }
    let state = capture.lock();
    let pids = scope.pids(&state);
    let mut before: Vec<String> = state
        .buffer
        .entries()
        .rev()
        .skip_while(|(seq, _)| *seq >= hit)
        .filter(|(seq, entry)| in_scope(&pids, *seq, entry))
        .take(context)
        .map(|(_, entry)| format_entry(entry))
        .collect();
    before.reverse();
    let mut hit_time = None;
    let mut lines = vec![format!(
        "Found after {waited:.1} s on {} ({}):",
        capture.serial(),
        scope.label()
    )];
    lines.extend(before);
    let mut after = 0;
    for (seq, entry) in state.buffer.entries_from(hit) {
        if seq == hit {
            hit_time = Some(entry.time);
            lines.push(format!(">> {}", format_entry(entry)));
        } else if in_scope(&pids, seq, entry) {
            lines.push(format_entry(entry));
            after += 1;
            if after == AFTER_CONTEXT {
                break;
            }
        }
    }
    if let Some(time) = hit_time {
        lines.push(format!(
            "The match is marked >>; pass since: \"{}\" to later calls to start there.",
            time.format_local()
        ));
    }
    lines.join("\n")
}

/// Places stack frames in the project: through the code index (fully qualified class names),
/// else by walking the project for the frame's file name.
struct FrameResolver<'a> {
    root: &'a Path,
    index: Option<&'a CodeIndex>,
    cache: HashMap<(String, String), Option<String>>,
    walks_left: usize,
}

impl FrameResolver<'_> {
    /// `path:line` relative to the root, for frames in the project.
    fn resolve(&mut self, frame: &StackFrame) -> Option<String> {
        if frame.file.is_empty()
            || FRAMEWORK_PREFIXES
                .iter()
                .any(|prefix| frame.class_name.starts_with(prefix))
        {
            return None;
        }
        let key = (frame.class_name.clone(), frame.file.clone());
        let path = match self.cache.get(&key) {
            Some(path) => path.clone(),
            None => {
                let class_files: Vec<PathBuf> = self
                    .index
                    .and_then(|index| {
                        frame.lookup_names().iter().find_map(|name| {
                            let files: Vec<PathBuf> = index
                                .types_named(name)
                                .into_iter()
                                .map(|symbol| index.abs_path(&symbol.rel_path))
                                .collect();
                            (!files.is_empty()).then_some(files)
                        })
                    })
                    .unwrap_or_default();
                let path = choose_source_file(frame, &class_files).or_else(|| {
                    (self.walks_left > 0).then(|| {
                        self.walks_left -= 1;
                        find_source_file(self.root, frame)
                    })?
                });
                let relative = path.map(|path| {
                    path.strip_prefix(self.root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/")
                });
                self.cache.insert(key, relative.clone());
                relative
            }
        }?;
        Some(match frame.line {
            Some(line) => format!("{path}:{line}"),
            None => path,
        })
    }
}

fn default_serial(devices: &[DeviceEntry], preferred: Option<&str>) -> Result<String, String> {
    if let Some(preferred) = preferred
        && devices
            .iter()
            .any(|device| device.serial == preferred && device.state.is_online())
    {
        return Ok(preferred.to_owned());
    }
    let online: Vec<&DeviceEntry> = devices
        .iter()
        .filter(|device| device.state.is_online())
        .collect();
    match online.as_slice() {
        [device] => Ok(device.serial.clone()),
        [] => Err(format!(
            "No online device. {}",
            if devices.is_empty() {
                "Start an emulator or connect a device with USB debugging on.".to_owned()
            } else {
                describe_devices(devices)
            }
        )),
        _ => Err(format!(
            "Several devices are online; pass device. {}",
            describe_devices(devices)
        )),
    }
}

fn describe_devices(devices: &[DeviceEntry]) -> String {
    if devices.is_empty() {
        return "No devices are connected.".to_owned();
    }
    format!(
        "Devices: {}.",
        devices
            .iter()
            .map(|device| format!("{} ({})", device.serial, device.state.label()))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn package_not_seen(scope: &Scope, serial: &str) -> String {
    format!(
        "0 entries from {serial} for {}. truncated: false\nNo process of {} has run on {serial} \
         since its logs were captured, and none runs now. Start the app, then call again or use \
         wait_for_log.",
        scope.label(),
        scope.label()
    )
}

/// One entry: `MM-DD HH:MM:SS.mmm  pid L tag: message`, further message lines indented.
fn format_entry(entry: &LogEntry) -> String {
    let message = truncate(&entry.message, MAX_MESSAGE_BYTES);
    let mut message_lines = message.split('\n').map(|line| line.trim_end_matches('\r'));
    let mut text = format!(
        "{} {:>5} {} {}: {}",
        entry.time.format_local(),
        entry.pid,
        entry.level.letter(),
        entry.tag,
        message_lines.next().unwrap_or_default()
    );
    for line in message_lines {
        text.push_str("\n    ");
        text.push_str(line);
    }
    text
}

fn truncate(text: &str, max: usize) -> Cow<'_, str> {
    if text.len() <= max {
        return Cow::Borrowed(text);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(format!(
        "{}… [{} bytes cut]",
        &text[..end],
        text.len() - end
    ))
}

fn parse_level(text: &str) -> Result<LogLevel, String> {
    let lowered = text.trim().to_ascii_lowercase();
    let alias = match lowered.as_str() {
        "warning" => Some(LogLevel::Warn),
        "assert" | "a" => Some(LogLevel::Fatal),
        _ => None,
    };
    LogLevel::ALL
        .into_iter()
        .find(|level| {
            lowered == level.name().to_ascii_lowercase()
                || lowered == level.letter().to_ascii_lowercase().to_string()
        })
        .or(alias)
        .ok_or_else(|| format!("Unknown min_level \"{text}\": use V, D, I, W, E or F."))
}

/// The call's `since`, else the device's mark.
fn since_argument(arguments: &Value, capture: &Capture) -> Result<Option<Since>, String> {
    match text_argument(arguments, "since") {
        Some(text) => parse_since(text, Local::now(), capture.clock_offset_secs).map(Some),
        None => Ok(capture.mark.map(Since::Seq)),
    }
}

fn text_argument<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn usize_argument(arguments: &Value, key: &str, default: usize, max: usize) -> usize {
    arguments
        .get(key)
        .and_then(Value::as_u64)
        .map_or(default, |value| (value as usize).clamp(1, max))
}

fn initialize_result(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|version| KNOWN_PROTOCOL_VERSIONS.contains(version))
        .unwrap_or(LATEST_PROTOCOL_VERSION);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "warpium-logcat", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Android device logs (logcat) of the app you are working on. Use \
            get_crashes after the app crashed or froze (stack frames point at project files), \
            get_logs for recent lines (filter by min_level, tag and text), clear_logs before \
            reproducing a bug so later calls only show new lines, and wait_for_log after \
            restarting the app to wait for a line. Calls are scoped to one app package; the \
            server follows the app's process across restarts. Logs may contain personal data: \
            don't copy them anywhere they aren't needed.",
    })
}

fn tool_definitions() -> Value {
    let device = json!({
        "type": "string",
        "description": "Device serial from list_devices. Optional: defaults to ANDROID_SERIAL or the only online device.",
    });
    let package = json!({
        "type": "string",
        "description": "Application id, such as com.example.app. Includes its :subprocesses and follows the app across restarts. Required unless all_packages is true.",
    });
    let all_packages = json!({
        "type": "boolean",
        "description": "Read every app's and the system's logs instead of one package. They may hold personal data; only use it when the package's own logs aren't enough.",
    });
    let since = json!({
        "type": "string",
        "description": "Start of the window: a duration back from now (\"30s\", \"2m\", \"1h\"), a time as printed (\"11-15 10:23:45.123\", \"10:23:45\") or ISO 8601, or a mark from clear_logs. Default: the last clear_logs mark, else everything captured.",
    });
    let min_level = json!({
        "type": "string",
        "description": "Lowest level shown: V, D, I, W, E or F.",
    });
    let tag = json!({
        "type": "string",
        "description": "Comma-separated tag substrings to include; -tag excludes (\"OkHttp,-chatty\").",
    });
    let text = json!({
        "type": "string",
        "description": "Case-insensitive text to find in the tag or message.",
    });
    let regex = json!({
        "type": "string",
        "description": "Regular expression to find in the tag or message (Rust regex syntax).",
    });
    json!([
        {
            "name": "list_devices",
            "description": "List connected Android devices and emulators: serial, connection, state, model and API level.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "get_logs",
            "description": format!("Recent logcat lines of an app, newest last: `time pid level tag: message`, with stack traces kept together. At most {MAX_LIMIT} entries; the header says whether older matching ones were left out (truncated)."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device": device,
                    "package": package,
                    "all_packages": all_packages,
                    "since": since,
                    "min_level": min_level,
                    "tag": tag,
                    "text": text,
                    "regex": regex,
                    "limit": { "type": "integer", "description": format!("Most recent matching entries returned (default {DEFAULT_LIMIT}, at most {MAX_LIMIT}).") },
                },
            },
        },
        {
            "name": "get_crashes",
            "description": "Crashes of an app in the captured logs: Java/Kotlin FATAL EXCEPTIONs, ANRs and native crashes, each with its stack trace. Frames in the project get `-> path:line` (relative to root).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device": device,
                    "package": package,
                    "all_packages": all_packages,
                    "since": since,
                    "root": { "type": "string", "description": "Project directory for resolving stack frames to files. Defaults to the server's --root." },
                    "limit": { "type": "integer", "description": format!("Newest crashes returned (default {DEFAULT_CRASH_LIMIT}, at most {MAX_CRASH_LIMIT}).") },
                },
            },
        },
        {
            "name": "wait_for_log",
            "description": format!("Wait until a matching line appears, such as after restarting the app, and return it with the lines around it. Only lines logged after the call count unless since is passed. Gives up after timeout_s (at most {MAX_WAIT_SECS} s)."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device": device,
                    "package": package,
                    "all_packages": all_packages,
                    "text": text,
                    "regex": regex,
                    "min_level": min_level,
                    "tag": tag,
                    "since": { "type": "string", "description": "Also accept lines already logged since then (same forms as get_logs' since)." },
                    "timeout_s": { "type": "number", "description": format!("Seconds to wait (default {DEFAULT_WAIT_SECS}, at most {MAX_WAIT_SECS}).") },
                    "context": { "type": "integer", "description": format!("Lines of the same app before the match (default {DEFAULT_CONTEXT}, at most {MAX_CONTEXT}).") },
                },
            },
        },
        {
            "name": "clear_logs",
            "description": "Start fresh before reproducing a bug: later get_logs and get_crashes calls leave out everything logged so far unless they pass since. Sets a mark; the device's log buffers are not cleared.",
            "inputSchema": { "type": "object", "properties": { "device": device } },
        },
    ])
}

fn error_response(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
