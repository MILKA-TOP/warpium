//! The Warpium Logcat MCP server for coding agents.
//!
//! ```text
//! warpium-logcat mcp [--root DIR] [--adb PATH]
//! ```
//!
//! `--root` is the Android project, for resolving stack frames to files and finding the SDK
//! through its `local.properties`. Without it, the current directory is used when it is a Gradle
//! project (agents start their MCP servers in the project they work on). The adb server port honors `ANDROID_ADB_SERVER_PORT`, and
//! `ANDROID_SERIAL` picks the default device.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use warpium_android::AdbClient;
use warpium_android::adb::server_port_from_env;
use warpium_android::sdk::{SdkEnv, find_sdk};
use warpium_logcat_mcp::{Server, ServerOptions};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("mcp") {
        return usage();
    }
    let mut root = None;
    let mut adb = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next().map(PathBuf::from),
            "--adb" => adb = args.next().map(PathBuf::from),
            _ => return usage(),
        }
    }
    let root = root
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .filter(|dir| is_gradle_project(dir))
        })
        .map(|root| std::fs::canonicalize(&root).unwrap_or(root));
    let adb = adb.or_else(|| find_sdk(root.as_deref(), &SdkEnv::from_process()).map(|sdk| sdk.adb));
    let client = AdbClient::new(server_port_from_env(), adb);
    let options = ServerOptions {
        root,
        default_serial: std::env::var("ANDROID_SERIAL")
            .ok()
            .filter(|serial| !serial.is_empty()),
        ..ServerOptions::default()
    };

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    match Server::new(client, options).serve(stdin.lock(), stdout.lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("MCP transport error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Whether `dir` is the root of a Gradle build.
fn is_gradle_project(dir: &Path) -> bool {
    [
        "settings.gradle.kts",
        "settings.gradle",
        "build.gradle.kts",
        "build.gradle",
    ]
    .iter()
    .any(|name| dir.join(name).is_file())
}

fn usage() -> ExitCode {
    eprintln!("usage: warpium-logcat mcp [--root DIR] [--adb PATH]");
    ExitCode::FAILURE
}
