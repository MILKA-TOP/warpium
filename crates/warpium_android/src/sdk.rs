//! Finding the Android SDK and its `adb` and `emulator` binaries.

use std::path::{Path, PathBuf};

/// Where an SDK location came from, for display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SdkSource {
    LocalProperties,
    AndroidHome,
    AndroidSdkRoot,
    DefaultLocation,
    /// No SDK directory was found, but `adb` is on `PATH`.
    Path,
}

impl SdkSource {
    pub fn label(self) -> &'static str {
        match self {
            SdkSource::LocalProperties => "local.properties sdk.dir",
            SdkSource::AndroidHome => "ANDROID_HOME",
            SdkSource::AndroidSdkRoot => "ANDROID_SDK_ROOT",
            SdkSource::DefaultLocation => "default location",
            SdkSource::Path => "PATH",
        }
    }
}

/// A located Android SDK.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AndroidSdk {
    /// The SDK directory; `None` when only an `adb` on `PATH` was found.
    pub root: Option<PathBuf>,
    pub source: SdkSource,
    pub adb: PathBuf,
    /// The emulator binary, when the SDK has one installed.
    pub emulator: Option<PathBuf>,
}

/// The environment discovery reads, so tests don't depend on the process environment.
#[derive(Clone, Debug, Default)]
pub struct SdkEnv {
    pub android_home: Option<PathBuf>,
    pub android_sdk_root: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub path: Option<std::ffi::OsString>,
    /// `%LOCALAPPDATA%`, for the default Windows location.
    pub local_app_data: Option<PathBuf>,
}

impl SdkEnv {
    pub fn from_process() -> Self {
        let non_empty = |name: &str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Self {
            android_home: non_empty("ANDROID_HOME"),
            android_sdk_root: non_empty("ANDROID_SDK_ROOT"),
            home: std::env::home_dir(),
            path: std::env::var_os("PATH"),
            local_app_data: non_empty("LOCALAPPDATA"),
        }
    }
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// The `adb` inside an SDK directory.
pub fn adb_in(root: &Path) -> PathBuf {
    root.join("platform-tools").join(exe("adb"))
}

/// The `emulator` inside an SDK directory.
pub fn emulator_in(root: &Path) -> PathBuf {
    root.join("emulator").join(exe("emulator"))
}

/// The platform's default SDK directory, where Android Studio installs it.
pub fn default_sdk_dir(env: &SdkEnv) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        env.home
            .as_ref()
            .map(|home| home.join("Library/Android/sdk"))
    } else if cfg!(windows) {
        env.local_app_data
            .as_ref()
            .map(|dir| dir.join("Android").join("Sdk"))
    } else {
        env.home.as_ref().map(|home| home.join("Android/Sdk"))
    }
}

/// Finds the SDK for a project (whose `local.properties` may name one), in Gradle's order:
/// `local.properties` `sdk.dir`, `ANDROID_HOME`, `ANDROID_SDK_ROOT`, then the default location.
/// A directory only counts when it has `platform-tools/adb`. Without one, an `adb` on `PATH`
/// is used.
pub fn find_sdk(project_root: Option<&Path>, env: &SdkEnv) -> Option<AndroidSdk> {
    let candidates = [
        (
            project_root.and_then(local_properties_sdk_dir),
            SdkSource::LocalProperties,
        ),
        (env.android_home.clone(), SdkSource::AndroidHome),
        (env.android_sdk_root.clone(), SdkSource::AndroidSdkRoot),
        (default_sdk_dir(env), SdkSource::DefaultLocation),
    ];
    for (root, source) in candidates {
        let Some(root) = root else {
            continue;
        };
        let adb = adb_in(&root);
        if adb.is_file() {
            let emulator = Some(emulator_in(&root)).filter(|path| path.is_file());
            return Some(AndroidSdk {
                root: Some(root),
                source,
                adb,
                emulator,
            });
        }
    }
    let adb = find_on_path(&exe("adb"), env.path.as_deref()?)?;
    Some(AndroidSdk {
        root: None,
        source: SdkSource::Path,
        adb,
        emulator: None,
    })
}

fn find_on_path(name: &str, path: &std::ffi::OsStr) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// `sdk.dir` from `<project>/local.properties`, if set.
pub fn local_properties_sdk_dir(project_root: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(project_root.join("local.properties")).ok()?;
    property(&text, "sdk.dir")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Looks up `key` in Java `.properties` text: `key=value` or `key: value` lines, `#`/`!`
/// comments, backslash escapes (`C\:\\Users` is `C:\Users`) and backslash line continuations.
pub fn property(text: &str, key: &str) -> Option<String> {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let mut logical = line.trim_start().to_owned();
        if logical.starts_with('#') || logical.starts_with('!') {
            continue;
        }
        while ends_with_continuation(&logical) {
            logical.pop();
            match lines.next() {
                Some(next) => logical.push_str(next.trim_start()),
                None => break,
            }
        }
        let (name, value) = split_property(&logical);
        if name == key {
            return Some(value);
        }
    }
    None
}

/// Whether a line ends with an odd number of backslashes.
fn ends_with_continuation(line: &str) -> bool {
    line.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1
}

fn split_property(line: &str) -> (String, String) {
    let mut name = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(escaped) = chars.next() {
                    name.push(unescape(escaped));
                }
            }
            '=' | ':' => break,
            c if c.is_whitespace() => {
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
                if matches!(chars.peek(), Some('=' | ':')) {
                    chars.next();
                }
                break;
            }
            c => name.push(c),
        }
    }
    while chars.peek().is_some_and(|c| c.is_whitespace()) {
        chars.next();
    }
    let mut value = String::new();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(escaped) = chars.next() {
                value.push(unescape(escaped));
            }
        } else {
            value.push(c);
        }
    }
    (name, value.trim_end().to_owned())
}

fn unescape(c: char) -> char {
    match c {
        't' => '\t',
        'n' => '\n',
        'r' => '\r',
        'f' => '\u{c}',
        other => other,
    }
}

#[cfg(test)]
#[path = "sdk_tests.rs"]
mod tests;
