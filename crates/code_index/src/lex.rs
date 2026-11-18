//! A tokenizer that finds the identifiers in Kotlin and Java source code, skipping comments and
//! string literals (but not the code inside Kotlin string templates). It is far cheaper than a
//! parse, so it builds the per-file identifier sets and finds usage candidates.

use crate::model::Language;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Code; `braces` counts the open braces of a `${…}` template (0 outside templates).
    Code { braces: u32 },
    /// A string literal; raw for `"""…"""` (Kotlin raw strings and Java text blocks).
    Str { raw: bool },
}

/// Calls `found(identifier, byte_offset)` for every identifier in code, in source order.
/// Kotlin backtick identifiers are reported without the backticks.
pub(crate) fn for_each_identifier<'s>(
    source: &'s str,
    language: Language,
    mut found: impl FnMut(&'s str, usize),
) {
    let bytes = source.as_bytes();
    let templates = language == Language::Kotlin;
    let mut modes = vec![Mode::Code { braces: 0 }];
    let mut i = 0;
    while i < bytes.len() {
        let mode = *modes.last().expect("the mode stack is never empty");
        let byte = bytes[i];
        match mode {
            Mode::Str { raw } => match byte {
                b'\\' if !raw || language == Language::Java => i += 2,
                b'"' if raw => {
                    if bytes[i..].starts_with(b"\"\"\"") {
                        // Extra quotes before the closing delimiter belong to the string.
                        let mut end = i + 3;
                        while bytes.get(end) == Some(&b'"') {
                            end += 1;
                        }
                        modes.pop();
                        i = end;
                    } else {
                        i += 1;
                    }
                }
                b'"' => {
                    modes.pop();
                    i += 1;
                }
                b'\n' if !raw => {
                    // An unterminated string ends at the line, so a typo can't hide the file.
                    modes.pop();
                    i += 1;
                }
                b'$' if templates => {
                    if bytes.get(i + 1) == Some(&b'{') {
                        modes.push(Mode::Code { braces: 1 });
                        i += 2;
                    } else {
                        let start = i + 1;
                        let end = identifier_end(bytes, start, language);
                        if end > start {
                            found(&source[start..end], start);
                        }
                        i = end.max(start);
                    }
                }
                _ => i += 1,
            },
            Mode::Code { braces } => match byte {
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    i = memchr_newline(bytes, i);
                }
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    i = skip_block_comment(bytes, i, language == Language::Kotlin);
                }
                b'"' => {
                    let raw = bytes[i..].starts_with(b"\"\"\"");
                    modes.push(Mode::Str { raw });
                    i += if raw { 3 } else { 1 };
                }
                b'\'' => i = skip_char_literal(bytes, i),
                b'`' if language == Language::Kotlin => {
                    let start = i + 1;
                    let end = bytes[start..]
                        .iter()
                        .position(|byte| matches!(byte, b'`' | b'\n'))
                        .map_or(bytes.len(), |offset| start + offset);
                    if end > start && bytes.get(end) == Some(&b'`') {
                        found(&source[start..end], start);
                    }
                    i = (end + 1).min(bytes.len());
                }
                b'{' if braces > 0 => {
                    *modes.last_mut().expect("non-empty") = Mode::Code { braces: braces + 1 };
                    i += 1;
                }
                b'}' if braces > 0 => {
                    if braces == 1 {
                        modes.pop();
                    } else {
                        *modes.last_mut().expect("non-empty") = Mode::Code { braces: braces - 1 };
                    }
                    i += 1;
                }
                _ if is_identifier_start(byte, language) => {
                    let end = identifier_end(bytes, i, language);
                    found(&source[i..end], i);
                    i = end;
                }
                b'0'..=b'9' => {
                    // Number literals, including suffixes and hex digits (`0xFFL`, `1e10f`).
                    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
                    {
                        i += 1;
                    }
                }
                _ => i += 1,
            },
        }
    }
}

/// Hard keywords, which never name a declaration and are left out of identifier sets.
pub(crate) fn is_keyword(word: &str, language: Language) -> bool {
    const KOTLIN: &[&str] = &[
        "as",
        "break",
        "class",
        "continue",
        "do",
        "else",
        "false",
        "for",
        "fun",
        "if",
        "in",
        "interface",
        "is",
        "null",
        "object",
        "package",
        "return",
        "super",
        "this",
        "throw",
        "true",
        "try",
        "typealias",
        "typeof",
        "val",
        "var",
        "when",
        "while",
    ];
    const JAVA: &[&str] = &[
        "abstract",
        "assert",
        "boolean",
        "break",
        "byte",
        "case",
        "catch",
        "char",
        "class",
        "const",
        "continue",
        "default",
        "do",
        "double",
        "else",
        "enum",
        "extends",
        "false",
        "final",
        "finally",
        "float",
        "for",
        "goto",
        "if",
        "implements",
        "import",
        "instanceof",
        "int",
        "interface",
        "long",
        "native",
        "new",
        "null",
        "package",
        "private",
        "protected",
        "public",
        "return",
        "short",
        "static",
        "strictfp",
        "super",
        "switch",
        "synchronized",
        "this",
        "throw",
        "throws",
        "transient",
        "true",
        "try",
        "void",
        "volatile",
        "while",
    ];
    match language {
        Language::Kotlin => KOTLIN.contains(&word),
        Language::Java => JAVA.contains(&word),
    }
}

fn is_identifier_start(byte: u8, language: Language) -> bool {
    byte.is_ascii_alphabetic()
        || byte == b'_'
        || byte >= 0x80
        || (byte == b'$' && language == Language::Java)
}

fn identifier_end(bytes: &[u8], start: usize, language: Language) -> usize {
    match bytes.get(start) {
        Some(byte) if is_identifier_start(*byte, language) => {}
        _ => return start,
    }
    let mut end = start + 1;
    while end < bytes.len()
        && (bytes[end].is_ascii_alphanumeric()
            || bytes[end] == b'_'
            || bytes[end] >= 0x80
            || (bytes[end] == b'$' && language == Language::Java))
    {
        end += 1;
    }
    end
}

fn memchr_newline(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |offset| from + offset)
}

/// Skips a `/* … */` comment starting at `start`. Kotlin block comments nest.
fn skip_block_comment(bytes: &[u8], start: usize, nested: bool) -> usize {
    let mut depth = 0u32;
    let mut i = start;
    while i + 1 < bytes.len() {
        if bytes[i] == b'/' && bytes[i + 1] == b'*' {
            if depth == 0 || nested {
                depth += 1;
            }
            i += 2;
        } else if bytes[i] == b'*' && bytes[i + 1] == b'/' {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            i += 1;
        }
    }
    bytes.len()
}

/// Skips a character literal (`'a'`, `'\n'`, `'A'`). A lone quote is skipped by itself.
fn skip_char_literal(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < bytes.len() && i - start <= 8 {
        match bytes[i] {
            b'\\' => i += 2,
            b'\'' => return i + 1,
            b'\n' => break,
            _ => i += 1,
        }
    }
    start + 1
}

/// Byte offsets where each line starts, for converting offsets to positions.
pub(crate) fn line_starts(source: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(source.match_indices('\n').map(|(offset, _)| offset + 1))
        .collect()
}

/// The zero-based line and character column of `offset`, given [`line_starts`].
pub(crate) fn position_of(source: &str, starts: &[usize], offset: usize) -> crate::Position {
    let line = starts
        .partition_point(|start| *start <= offset)
        .saturating_sub(1);
    let column = source[starts[line]..offset].chars().count();
    crate::Position {
        line: line as u32,
        column: column as u32,
    }
}

/// The byte offset of a zero-based line and character column, clamped to the line's end.
pub(crate) fn offset_of(
    source: &str,
    starts: &[usize],
    position: crate::Position,
) -> Option<usize> {
    let start = *starts.get(position.line as usize)?;
    let line_end = starts
        .get(position.line as usize + 1)
        .map_or(source.len(), |next| next - 1);
    let line = &source[start..line_end];
    let byte = line
        .char_indices()
        .nth(position.column as usize)
        .map_or(line.len(), |(byte, _)| byte);
    Some(start + byte)
}

#[cfg(test)]
#[path = "lex_tests.rs"]
mod tests;
