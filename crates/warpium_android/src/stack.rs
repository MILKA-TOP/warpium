//! Stack-trace frames in log lines (`at com.foo.Bar.baz(Bar.kt:42)`) and finding their source.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

/// A frame of a JVM stack trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackFrame {
    /// The binary class name, such as `com.foo.Bar$Inner$1`.
    pub class_name: String,
    pub method: String,
    /// `Bar.kt`; empty for `Unknown Source` and `Native Method`.
    pub file: String,
    pub line: Option<u32>,
    /// Byte range of the `Class.method(File:line)` part in the parsed line, to underline.
    pub span: Range<usize>,
}

static FRAME: LazyLock<Regex> = LazyLock::new(|| {
    // `at [module@version/][loader/]class.method(file:line)`; module and class-loader prefixes
    // come from Java 9+ (`java.base/…`, `app//…`).
    Regex::new(
        r"^\s*at\s+(?:[\w.$@-]*/+)?(?P<frame>(?P<class>[\w$]+(?:\.[\w$]+)*)\.(?P<method>[\w$<>-]+)\((?P<location>[^()]*)\))",
    )
    .expect("the stack frame pattern is valid")
});

/// Parses a stack frame line, or `None` if `line` isn't one.
pub fn parse_frame(line: &str) -> Option<StackFrame> {
    if !line.trim_start().starts_with("at ") {
        return None;
    }
    let captures = FRAME.captures(line)?;
    let location = captures.name("location")?.as_str();
    let (file, line_number) = match location.rsplit_once(':') {
        Some((file, number)) => (file, number.parse().ok()),
        None => (location, None),
    };
    let file = if file.contains('.') { file } else { "" };
    Some(StackFrame {
        class_name: captures.name("class")?.as_str().to_owned(),
        method: captures.name("method")?.as_str().to_owned(),
        file: file.to_owned(),
        line: line_number,
        span: captures.name("frame")?.range(),
    })
}

impl StackFrame {
    /// The package: everything before the last `.` of the class name.
    pub fn package(&self) -> &str {
        self.class_name
            .rsplit_once('.')
            .map_or("", |(package, _)| package)
    }

    /// The top-level class's simple name (`Bar` for `com.foo.Bar$Inner$1`).
    pub fn outer_simple_name(&self) -> &str {
        let simple = self
            .class_name
            .rsplit_once('.')
            .map_or(self.class_name.as_str(), |(_, simple)| simple);
        simple.split('$').next().unwrap_or(simple)
    }

    /// Qualified names to look up in a code index, most specific first: the named nested classes
    /// (`com.foo.Bar.Inner` for `com.foo.Bar$Inner$1`), then the top-level class. Kotlin file
    /// facades (`BarKt`) have no declaration, so they yield nothing; [`Self::file`] finds those.
    pub fn lookup_names(&self) -> Vec<String> {
        let package = self.package();
        let simple = self
            .class_name
            .rsplit_once('.')
            .map_or(self.class_name.as_str(), |(_, simple)| simple);
        let named: Vec<&str> = simple
            .split('$')
            .take_while(|part| !part.is_empty() && !part.chars().all(|c| c.is_ascii_digit()))
            .collect();
        let qualify = |parts: &[&str]| {
            let nested = parts.join(".");
            if package.is_empty() {
                nested
            } else {
                format!("{package}.{nested}")
            }
        };
        let mut names: Vec<String> = (1..=named.len())
            .rev()
            .map(|count| qualify(&named[..count]))
            .collect();
        if named.len() == 1 && named[0].ends_with("Kt") && self.file.ends_with(".kt") {
            names.clear();
        }
        names
    }
}

/// Picks the source file for a frame from the files of the classes a code index matched: one
/// with the frame's file name, else that name next to one of them (Kotlin files declare several
/// classes, and inline functions report their own file).
pub fn choose_source_file(frame: &StackFrame, class_files: &[PathBuf]) -> Option<PathBuf> {
    if frame.file.is_empty() {
        return class_files.first().cloned();
    }
    if let Some(exact) = class_files.iter().find(|path| {
        path.file_name()
            .is_some_and(|name| name == frame.file.as_str())
    }) {
        return Some(exact.clone());
    }
    class_files
        .iter()
        .filter_map(|path| path.parent().map(|dir| dir.join(&frame.file)))
        .find(|sibling| sibling.is_file())
}

/// Searches `root` (honoring `.gitignore`, skipping hidden and `build` directories) for the
/// frame's file, preferring one under the package's directories (`com/foo/Bar.kt`). Walks the
/// tree, so run it off the UI thread.
pub fn find_source_file(root: &Path, frame: &StackFrame) -> Option<PathBuf> {
    if frame.file.is_empty() {
        return None;
    }
    let package_dirs: PathBuf = frame.package().split('.').collect();
    let mut fallback = None;
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != "build")
        .build();
    for entry in walker.flatten() {
        if entry.file_name() != frame.file.as_str() {
            continue;
        }
        let path = entry.into_path();
        if path
            .parent()
            .is_some_and(|dir| dir.ends_with(&package_dirs))
        {
            return Some(path);
        }
        fallback.get_or_insert(path);
    }
    fallback
}

#[cfg(test)]
#[path = "stack_tests.rs"]
mod tests;
