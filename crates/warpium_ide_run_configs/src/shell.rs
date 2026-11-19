//! Building command lines from configuration values.

/// `arg` as one shell word: unchanged when it has only safe characters, else single-quoted
/// (which fish and PowerShell read the same way unless the word itself has a quote).
pub(crate) fn quote_arg(arg: &str) -> String {
    let safe = !arg.is_empty()
        && arg.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | '-' | '.' | '/' | ':' | '=' | '@' | '%' | '+' | ',')
        });
    if safe {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// Splits a parameter string the way the IDE does: on whitespace, with double quotes grouping.
pub(crate) fn split_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                in_word = true;
            }
            '\\' if quoted && chars.peek() == Some(&'"') => {
                current.push('"');
                chars.next();
            }
            c if c.is_whitespace() && !quoted => {
                if in_word {
                    args.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            c => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        args.push(current);
    }
    args
}

/// Joins the non-empty parts of a command line with spaces.
pub(crate) fn join_command<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `command` with every `${` written as `$${`, so running it as a project toolbar command leaves
/// shell parameter expansions such as `${HOME}` to the shell instead of reading them as toolbar
/// variables.
pub fn escape_toolbar_variables(command: &str) -> String {
    command.replace("${", "$${")
}

/// Whether `name` can be exported as an environment variable from a shell.
pub(crate) fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
#[path = "shell_tests.rs"]
mod tests;
