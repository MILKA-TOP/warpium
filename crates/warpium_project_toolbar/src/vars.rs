//! Variables in `command` buttons, such as `${file}`, expanded when the button runs.
//!
//! Each variable becomes one shell word, quoted for the shell the command runs in, so commands
//! use them unquoted: `ktlint --format ${file}`. `$${` stands for a literal `${`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::action::ShellFamily;

/// The variables a command can use, for help text: name and meaning.
pub const COMMAND_VARIABLES: &[(&str, &str)] = &[
    ("${projectRoot}", "the project's root folder"),
    ("${file}", "the file open in the focused editor"),
    ("${fileDir}", "the folder of that file"),
    ("${selection}", "the text selected in the focused editor or terminal"),
    ("${cwd}", "the active session's working directory (else the project root)"),
    ("${prompt:Label}", "a value asked for in a dialog when the button runs"),
];

const PROMPT_PREFIX: &str = "prompt:";

/// What the variables expand to when a command runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VariableValues {
    pub project_root: PathBuf,
    /// The file open in the focused editor.
    pub file: Option<PathBuf>,
    /// The selected text in the focused editor or terminal.
    pub selection: Option<String>,
    /// The active session's working directory.
    pub cwd: Option<PathBuf>,
    /// The answers to `${prompt:Label}`, by label.
    pub prompts: BTreeMap<String, String>,
}

/// Why a command's variables couldn't be expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariableError {
    Unknown(String),
    /// A `${` without its closing `}`.
    Unterminated,
    /// `${file}` or `${fileDir}` with no file open in an editor.
    NoFile,
    /// `${selection}` with nothing selected.
    NoSelection,
    /// No answer was given for this `${prompt:Label}`.
    MissingPrompt(String),
}

impl fmt::Display for VariableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(name) => write!(
                f,
                "unknown variable `${{{name}}}`; use ${{projectRoot}}, ${{file}}, ${{fileDir}}, \
                 ${{selection}}, ${{cwd}} or ${{prompt:Label}}"
            ),
            Self::Unterminated => write!(f, "a `${{` has no closing `}}`"),
            Self::NoFile => write!(f, "the command uses ${{file}}, but no file is open in an editor"),
            Self::NoSelection => write!(f, "the command uses ${{selection}}, but nothing is selected"),
            Self::MissingPrompt(label) => write!(f, "no value was given for \"{label}\""),
        }
    }
}

impl std::error::Error for VariableError {}

enum Piece<'a> {
    Text(&'a str),
    Variable(&'a str),
}

/// Splits `command` into literal text and variable names, turning `$${` into a literal `${`.
fn pieces(command: &str) -> Result<Vec<Piece<'_>>, VariableError> {
    let mut pieces = Vec::new();
    let mut rest = command;
    while let Some(start) = rest.find("${") {
        if rest[..start].ends_with('$') {
            pieces.push(Piece::Text(&rest[..start - 1]));
            pieces.push(Piece::Text("${"));
            rest = &rest[start + 2..];
            continue;
        }
        pieces.push(Piece::Text(&rest[..start]));
        let after = &rest[start + 2..];
        let end = after.find('}').ok_or(VariableError::Unterminated)?;
        pieces.push(Piece::Variable(&after[..end]));
        rest = &after[end + 1..];
    }
    pieces.push(Piece::Text(rest));
    Ok(pieces)
}

/// Whether `command` has any variables (or `$${` escapes) to expand.
pub fn has_variables(command: &str) -> bool {
    command.contains("${")
}

/// The labels of the `${prompt:Label}` variables in `command`, in order and without repeats.
pub fn prompt_labels(command: &str) -> Vec<String> {
    let mut labels: Vec<String> = Vec::new();
    for piece in pieces(command).unwrap_or_default() {
        if let Piece::Variable(name) = piece
            && let Some(label) = name.strip_prefix(PROMPT_PREFIX)
            && !labels.iter().any(|known| known == label)
        {
            labels.push(label.to_owned());
        }
    }
    labels
}

/// Checks that every variable in `command` is a known one.
pub fn validate_variables(command: &str) -> Result<(), VariableError> {
    for piece in pieces(command)? {
        if let Piece::Variable(name) = piece
            && !is_known(name)
        {
            return Err(VariableError::Unknown(name.to_owned()));
        }
    }
    Ok(())
}

fn is_known(name: &str) -> bool {
    matches!(name, "projectRoot" | "file" | "fileDir" | "selection" | "cwd")
        || name.starts_with(PROMPT_PREFIX)
}

/// Replaces the variables in `command` with their values, each quoted as one word for `shell`.
pub fn expand_variables(
    command: &str,
    values: &VariableValues,
    shell: ShellFamily,
) -> Result<String, VariableError> {
    let path = |path: &Path| shell.quote(&path.to_string_lossy());
    let mut expanded = String::with_capacity(command.len());
    for piece in pieces(command)? {
        match piece {
            Piece::Text(text) => expanded.push_str(text),
            Piece::Variable(name) => {
                let value = match name {
                    "projectRoot" => path(&values.project_root),
                    "file" => path(values.file.as_deref().ok_or(VariableError::NoFile)?),
                    "fileDir" => path(
                        values
                            .file
                            .as_deref()
                            .and_then(Path::parent)
                            .ok_or(VariableError::NoFile)?,
                    ),
                    "selection" => shell.quote(
                        values
                            .selection
                            .as_deref()
                            .filter(|selection| !selection.is_empty())
                            .ok_or(VariableError::NoSelection)?,
                    ),
                    "cwd" => path(values.cwd.as_deref().unwrap_or(&values.project_root)),
                    other => match other.strip_prefix(PROMPT_PREFIX) {
                        Some(label) => shell.quote(
                            values
                                .prompts
                                .get(label)
                                .ok_or_else(|| VariableError::MissingPrompt(label.to_owned()))?,
                        ),
                        None => return Err(VariableError::Unknown(other.to_owned())),
                    },
                };
                expanded.push_str(&value);
            }
        }
    }
    Ok(expanded)
}

#[cfg(test)]
#[path = "vars_tests.rs"]
mod tests;
