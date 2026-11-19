//! Recognizes a terminal command as an interactive session inside a container (Docker, Podman,
//! nerdctl, Compose) or on an SSH host, and extracts what's needed to reach it from the host.

/// The container engine whose CLI started the session. All three share Docker's `exec`, `cp`,
/// `ps` and `inspect` syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Docker,
    Podman,
    Nerdctl,
}

impl Engine {
    pub fn default_program(self) -> &'static str {
        match self {
            Engine::Docker => "docker",
            Engine::Podman => "podman",
            Engine::Nerdctl => "nerdctl",
        }
    }

    fn global_options(self) -> OptionSpec {
        match self {
            Engine::Docker => OptionSpec {
                short_values: "cHl",
                long_values: &[
                    "config",
                    "context",
                    "host",
                    "log-level",
                    "tlscacert",
                    "tlscert",
                    "tlskey",
                ],
                long_bools: &[],
                unknown_long_takes_value: false,
            },
            Engine::Podman => OptionSpec {
                short_values: "c",
                long_values: &[
                    "cgroup-manager",
                    "config",
                    "conmon",
                    "connection",
                    "db-backend",
                    "events-backend",
                    "hooks-dir",
                    "identity",
                    "imagestore",
                    "log-level",
                    "module",
                    "network-cmd-path",
                    "network-config-dir",
                    "root",
                    "runroot",
                    "runtime",
                    "runtime-flag",
                    "ssh",
                    "storage-driver",
                    "storage-opt",
                    "tmpdir",
                    "url",
                    "volumepath",
                ],
                long_bools: &[],
                unknown_long_takes_value: false,
            },
            Engine::Nerdctl => OptionSpec {
                short_values: "naH",
                long_values: &[
                    "address",
                    "bridge-ip",
                    "cgroup-manager",
                    "cni-netconfpath",
                    "cni-path",
                    "data-root",
                    "host",
                    "host-gateway-ip",
                    "hosts-dir",
                    "namespace",
                    "snapshotter",
                    "storage-driver",
                ],
                long_bools: &[],
                unknown_long_takes_value: false,
            },
        }
    }
}

/// How the session's container is identified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContainerRef {
    /// A container name or id given on the command line (`exec`, `attach`, `start`, `--name`).
    Named(String),
    /// A container started by `run` from this image without a name: the newest running one.
    Image(String),
    ComposeService(ComposeService),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposeService {
    pub service: String,
    /// `-p`/`--project-name`.
    pub project: Option<String>,
    /// `--project-directory`, as written.
    pub project_directory: Option<String>,
    /// `-f`/`--file`, as written.
    pub files: Vec<String>,
    /// `compose run` starts a one-off container, while `compose exec` and `attach` enter the
    /// service's own.
    pub one_off: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerInvocation {
    pub engine: Engine,
    /// The engine's CLI as written (`docker`, `/usr/local/bin/podman`), or the engine's default
    /// program for Compose front-ends.
    pub program: String,
    /// Global options that select the daemon (`--context`, `-H`, `--connection`, ...), to be
    /// passed to every command run against the same container.
    pub global_args: Vec<String>,
    pub container: ContainerRef,
    /// The command started inside the container, if given.
    pub inner_command: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshInvocation {
    /// `ssh` as written.
    pub program: String,
    /// Options that pick the host, identity and connection (`-p`, `-i`, `-J`, `-F`, `-o`, ...).
    pub options: Vec<String>,
    pub destination: String,
    pub inner_command: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Container(ContainerInvocation),
    Ssh(SshInvocation),
}

impl Target {
    /// A container chosen by the user rather than detected: `name`, or `podman:name` for Podman
    /// (`nerdctl:` likewise).
    pub fn from_override(value: &str) -> Option<Target> {
        let value = value.trim();
        let (engine, name) = match value.split_once(':') {
            Some(("docker", name)) => (Engine::Docker, name),
            Some(("podman", name)) => (Engine::Podman, name),
            Some(("nerdctl", name)) => (Engine::Nerdctl, name),
            _ => (Engine::Docker, value),
        };
        let name = name.trim();
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            return None;
        }
        Some(Target::Container(ContainerInvocation {
            engine,
            program: engine.default_program().to_owned(),
            global_args: Vec::new(),
            container: ContainerRef::Named(name.to_owned()),
            inner_command: None,
        }))
    }

    /// The executable name of the command started inside the container or on the host.
    pub fn inner_program(&self) -> Option<&str> {
        let command = match self {
            Target::Container(invocation) => invocation.inner_command.as_deref(),
            Target::Ssh(invocation) => invocation.inner_command.as_deref(),
        }?;
        let first = command.split_whitespace().next()?;
        Some(first.rsplit('/').next().unwrap_or(first))
    }

    /// How a path to a pasted file is typed for the agent: Gemini CLI attaches files written as
    /// `@path`, the other agents (Claude Code, Codex, Kimi Code) take a bare pasted path.
    pub fn paste_text_for_path(&self, path: &str) -> String {
        match self.inner_program() {
            Some("gemini") => format!("@{path}"),
            _ => path.to_owned(),
        }
    }
}

/// Parses `command` (a shell command line) and returns the container or SSH session it starts.
///
/// `resolve_alias` maps a command's first word to its alias value, if it is an alias. In a command
/// list (`cd app && docker compose run web`), the last command that starts a session counts.
pub fn parse_command(
    command: &str,
    resolve_alias: impl Fn(&str) -> Option<String>,
) -> Option<Target> {
    let words = shlex::split(command.trim())?;
    split_command_list(&words)
        .into_iter()
        .rev()
        .find_map(|segment| {
            let segment = expand_alias(segment, &resolve_alias);
            parse_simple_command(&segment)
        })
}

fn split_command_list(words: &[String]) -> Vec<Vec<String>> {
    let mut segments = vec![Vec::new()];
    for word in words {
        match word.as_str() {
            "&&" | "||" | ";" | "|" | "&" | "|&" => segments.push(Vec::new()),
            _ => match word.strip_suffix(';') {
                Some(stripped) if !stripped.ends_with('\\') => {
                    if let Some(segment) = segments.last_mut()
                        && !stripped.is_empty()
                    {
                        segment.push(stripped.to_owned());
                    }
                    segments.push(Vec::new());
                }
                _ => {
                    if let Some(segment) = segments.last_mut() {
                        segment.push(word.clone());
                    }
                }
            },
        }
    }
    segments.retain(|segment| !segment.is_empty());
    segments
}

fn expand_alias(
    segment: Vec<String>,
    resolve_alias: &impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    let Some(first) = segment.first() else {
        return segment;
    };
    match resolve_alias(first).and_then(|value| shlex::split(&value)) {
        Some(mut expanded) if !expanded.is_empty() => {
            expanded.extend(segment.into_iter().skip(1));
            expanded
        }
        _ => segment,
    }
}

fn is_env_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Skips wrappers that run the rest of the line as a command: assignments, `env`, `sudo`,
/// `command`, `exec`, `time`, `nice`, `nohup`, `doas`.
fn strip_wrappers(words: &[String]) -> &[String] {
    let mut rest = words;
    loop {
        let Some(first) = rest.first() else {
            return rest;
        };
        let skip = if is_env_assignment(first) {
            1
        } else {
            match first.rsplit('/').next().unwrap_or(first) {
                "env" => {
                    1 + rest[1..]
                        .iter()
                        .position(|w| !(w.starts_with('-') || is_env_assignment(w)))
                        .unwrap_or(rest.len() - 1)
                }
                "sudo" | "doas" => 1 + options_len(&rest[1..], "ugCDhpPrtTU"),
                "nice" => 1 + options_len(&rest[1..], "n"),
                "command" | "exec" | "time" | "nohup" => 1 + options_len(&rest[1..], ""),
                _ => return rest,
            }
        };
        rest = &rest[skip.min(rest.len())..];
    }
}

/// The number of leading option words, where short options in `short_values` take a value.
fn options_len(words: &[String], short_values: &str) -> usize {
    let mut i = 0;
    while let Some(word) = words.get(i) {
        if word == "--" {
            return i + 1;
        }
        if !word.starts_with('-') || word == "-" {
            break;
        }
        i += 1;
        if !word.starts_with("--")
            && word.len() == 2
            && word[1..].chars().all(|c| short_values.contains(c))
        {
            i += 1;
        }
    }
    i.min(words.len())
}

fn parse_simple_command(words: &[String]) -> Option<Target> {
    let words = strip_wrappers(words);
    let program = words.first()?;
    let rest = &words[1..];
    match program.rsplit('/').next().unwrap_or(program) {
        "docker" => parse_engine(Engine::Docker, program, rest),
        "podman" => parse_engine(Engine::Podman, program, rest),
        "nerdctl" => parse_engine(Engine::Nerdctl, program, rest),
        "docker-compose" => parse_compose(Engine::Docker, "docker", Vec::new(), rest),
        "podman-compose" => parse_compose(Engine::Podman, "podman", Vec::new(), rest),
        "ssh" => parse_ssh(program, rest),
        _ => None,
    }
}

/// Which options take a value. Options not listed are flags, unless `unknown_long_takes_value`.
struct OptionSpec {
    short_values: &'static str,
    long_values: &'static [&'static str],
    long_bools: &'static [&'static str],
    unknown_long_takes_value: bool,
}

#[derive(Debug)]
struct ParsedOption {
    /// The option's name without dashes; a single character for short options.
    name: String,
    value: Option<String>,
    /// The words the option was written as.
    words: Vec<String>,
}

/// Splits leading options from the first positional argument (whose index is returned).
fn scan_options(words: &[String], spec: &OptionSpec) -> (Vec<ParsedOption>, usize) {
    let mut options = Vec::new();
    let mut i = 0;
    while let Some(word) = words.get(i) {
        if word == "--" {
            return (options, i + 1);
        }
        if !word.starts_with('-') || word == "-" {
            break;
        }
        i += 1;
        if let Some(long) = word.strip_prefix("--") {
            if let Some((name, value)) = long.split_once('=') {
                options.push(ParsedOption {
                    name: name.to_owned(),
                    value: Some(value.to_owned()),
                    words: vec![word.clone()],
                });
                continue;
            }
            let takes_value = spec.long_values.contains(&long)
                || (spec.unknown_long_takes_value && !spec.long_bools.contains(&long));
            let mut option_words = vec![word.clone()];
            let value = if takes_value {
                let value = words.get(i).cloned();
                if let Some(value) = &value {
                    option_words.push(value.clone());
                    i += 1;
                }
                value
            } else {
                None
            };
            options.push(ParsedOption {
                name: long.to_owned(),
                value,
                words: option_words,
            });
            continue;
        }
        // A cluster of short flags (`-it`), where a value-taking letter consumes the rest of the
        // word (`-p8080:80`) or the next word.
        let letters = &word[1..];
        for (index, letter) in letters.char_indices() {
            if spec.short_values.contains(letter) {
                let attached = &letters[index + letter.len_utf8()..];
                let mut option_words = if index == 0 {
                    vec![word.clone()]
                } else {
                    Vec::new()
                };
                let value = if !attached.is_empty() {
                    Some(attached.trim_start_matches('=').to_owned())
                } else {
                    let value = words.get(i).cloned();
                    if let Some(value) = &value {
                        option_words.push(value.clone());
                        i += 1;
                    }
                    value
                };
                options.push(ParsedOption {
                    name: letter.to_string(),
                    value,
                    words: option_words,
                });
                break;
            }
            options.push(ParsedOption {
                name: letter.to_string(),
                value: None,
                words: if index == 0 {
                    vec![word.clone()]
                } else {
                    Vec::new()
                },
            });
        }
    }
    (options, i.min(words.len()))
}

fn option_value<'a>(options: &'a [ParsedOption], names: &[&str]) -> Option<&'a str> {
    options
        .iter()
        .rev()
        .find(|option| names.contains(&option.name.as_str()))
        .and_then(|option| option.value.as_deref())
}

fn has_flag(options: &[ParsedOption], names: &[&str]) -> bool {
    options.iter().any(|option| {
        names.contains(&option.name.as_str())
            && option
                .value
                .as_deref()
                .is_none_or(|value| !matches!(value, "false" | "0"))
    })
}

fn inner_command(words: &[String]) -> Option<String> {
    (!words.is_empty()).then(|| {
        shlex::try_join(words.iter().map(String::as_str)).unwrap_or_else(|_| words.join(" "))
    })
}

fn parse_engine(engine: Engine, program: &str, words: &[String]) -> Option<Target> {
    let (globals, subcommand_index) = scan_options(words, &engine.global_options());
    let global_args = globals
        .into_iter()
        .filter(|option| !matches!(option.name.as_str(), "D" | "debug" | "l" | "log-level"))
        .flat_map(|option| option.words)
        .collect::<Vec<_>>();
    let mut rest = &words[subcommand_index..];
    let mut subcommand = rest.first()?.as_str();
    if subcommand == "container" {
        rest = &rest[1..];
        subcommand = rest.first()?.as_str();
    }
    let args = &rest[1..];
    let container = match subcommand {
        "run" => parse_run(args)?,
        "exec" => parse_exec(args)?,
        "attach" => {
            let (_, index) = scan_options(args, &ATTACH_OPTIONS);
            (ContainerRef::Named(args.get(index)?.clone()), None)
        }
        "start" => {
            let (options, index) = scan_options(args, &START_OPTIONS);
            if !has_flag(&options, &["a", "attach", "i", "interactive"]) {
                return None;
            }
            (ContainerRef::Named(args.get(index)?.clone()), None)
        }
        "compose" => return parse_compose(engine, program, global_args, args),
        _ => return None,
    };
    let (container, inner_command) = container;
    Some(Target::Container(ContainerInvocation {
        engine,
        program: program.to_owned(),
        global_args,
        container,
        inner_command,
    }))
}

const RUN_BOOL_OPTIONS: &[&str] = &[
    "detach",
    "disable-content-trust",
    "env-host",
    "help",
    "http-proxy",
    "init",
    "interactive",
    "no-healthcheck",
    "no-hosts",
    "oom-kill-disable",
    "passwd",
    "privileged",
    "publish-all",
    "quiet",
    "read-only",
    "read-only-tmpfs",
    "replace",
    "rm",
    "rmi",
    "sig-proxy",
    "tls-verify",
    "tty",
    "use-api-socket",
];

const RUN_OPTIONS: OptionSpec = OptionSpec {
    short_values: "acehlmpuvw",
    long_values: &[],
    long_bools: RUN_BOOL_OPTIONS,
    unknown_long_takes_value: true,
};

const EXEC_OPTIONS: OptionSpec = OptionSpec {
    short_values: "euw",
    long_values: &[],
    long_bools: &["detach", "interactive", "privileged", "tty", "latest"],
    unknown_long_takes_value: true,
};

const ATTACH_OPTIONS: OptionSpec = OptionSpec {
    short_values: "",
    long_values: &["detach-keys"],
    long_bools: &[],
    unknown_long_takes_value: false,
};

const START_OPTIONS: OptionSpec = OptionSpec {
    short_values: "",
    long_values: &["checkpoint", "checkpoint-dir", "detach-keys"],
    long_bools: &[],
    unknown_long_takes_value: false,
};

fn parse_run(args: &[String]) -> Option<(ContainerRef, Option<String>)> {
    let (options, index) = scan_options(args, &RUN_OPTIONS);
    if has_flag(&options, &["d", "detach"]) {
        return None;
    }
    let image = args.get(index)?;
    let container = match option_value(&options, &["name"]) {
        Some(name) => ContainerRef::Named(name.to_owned()),
        None => ContainerRef::Image(image.clone()),
    };
    Some((container, inner_command(&args[index + 1..])))
}

fn parse_exec(args: &[String]) -> Option<(ContainerRef, Option<String>)> {
    let (options, index) = scan_options(args, &EXEC_OPTIONS);
    if has_flag(&options, &["d", "detach"]) {
        return None;
    }
    let container = args.get(index)?;
    Some((
        ContainerRef::Named(container.clone()),
        inner_command(&args[index + 1..]),
    ))
}

const COMPOSE_GLOBAL_OPTIONS: OptionSpec = OptionSpec {
    short_values: "fp",
    long_values: &[],
    long_bools: &[
        "all-resources",
        "compatibility",
        "dry-run",
        "no-ansi",
        "no-cleanup",
        "no-normalize",
        "no-pod",
        "verbose",
    ],
    unknown_long_takes_value: true,
};

const COMPOSE_RUN_OPTIONS: OptionSpec = OptionSpec {
    short_values: "elpvwu",
    long_values: &[],
    long_bools: &[
        "build",
        "detach",
        "interactive",
        "no-TTY",
        "no-deps",
        "publish-all",
        "quiet",
        "quiet-build",
        "quiet-pull",
        "remove-orphans",
        "rm",
        "service-ports",
        "tty",
        "use-aliases",
        "dry-run",
    ],
    unknown_long_takes_value: true,
};

const COMPOSE_EXEC_OPTIONS: OptionSpec = OptionSpec {
    short_values: "euw",
    long_values: &[],
    long_bools: &[
        "detach",
        "dry-run",
        "interactive",
        "no-TTY",
        "privileged",
        "tty",
    ],
    unknown_long_takes_value: true,
};

fn parse_compose(
    engine: Engine,
    program: &str,
    global_args: Vec<String>,
    words: &[String],
) -> Option<Target> {
    let (globals, subcommand_index) = scan_options(words, &COMPOSE_GLOBAL_OPTIONS);
    let files = globals
        .iter()
        .filter(|option| matches!(option.name.as_str(), "f" | "file"))
        .filter_map(|option| option.value.clone())
        .collect();
    let project = option_value(&globals, &["p", "project-name"]).map(str::to_owned);
    let project_directory = option_value(&globals, &["project-directory"]).map(str::to_owned);
    let subcommand = words.get(subcommand_index)?.as_str();
    let args = &words[subcommand_index + 1..];
    let (options, index, one_off) = match subcommand {
        "run" => {
            let (options, index) = scan_options(args, &COMPOSE_RUN_OPTIONS);
            (options, index, true)
        }
        "exec" => {
            let (options, index) = scan_options(args, &COMPOSE_EXEC_OPTIONS);
            (options, index, false)
        }
        "attach" => {
            let (options, index) = scan_options(args, &ATTACH_OPTIONS);
            (options, index, false)
        }
        _ => return None,
    };
    if has_flag(&options, &["d", "detach"]) {
        return None;
    }
    let service = args.get(index)?.clone();
    let container = match option_value(&options, &["name"]).filter(|_| one_off) {
        Some(name) => ContainerRef::Named(name.to_owned()),
        None => ContainerRef::ComposeService(ComposeService {
            service,
            project,
            project_directory,
            files,
            one_off,
        }),
    };
    Some(Target::Container(ContainerInvocation {
        engine,
        program: program.to_owned(),
        global_args,
        container,
        inner_command: inner_command(&args[index + 1..]),
    }))
}

const SSH_OPTIONS: OptionSpec = OptionSpec {
    short_values: "BbcDEeFIiJLlmOopQRSWw",
    long_values: &[],
    long_bools: &[],
    unknown_long_takes_value: false,
};

/// Options that only matter to the interactive session (forwarding, logging, control commands)
/// and could make a second connection fail or misbehave.
const SSH_SESSION_ONLY_OPTIONS: &[&str] = &[
    "D", "E", "L", "N", "O", "Q", "R", "T", "W", "f", "n", "q", "s", "t", "v", "w", "e", "G", "V",
    "M", "x", "X", "Y", "A", "a", "g", "k", "K",
];

fn parse_ssh(program: &str, words: &[String]) -> Option<Target> {
    let (options, index) = scan_options(words, &SSH_OPTIONS);
    if has_flag(&options, &["G", "V", "O", "Q", "N", "W"]) {
        return None;
    }
    let destination = words.get(index)?.clone();
    let options = options
        .into_iter()
        .filter(|option| !SSH_SESSION_ONLY_OPTIONS.contains(&option.name.as_str()))
        .flat_map(|option| match &option.value {
            Some(value) => vec![format!("-{}", option.name), value.clone()],
            None => vec![format!("-{}", option.name)],
        })
        .collect();
    Some(Target::Ssh(SshInvocation {
        program: program.to_owned(),
        options,
        destination,
        inner_command: inner_command(&words[index + 1..]),
    }))
}

#[cfg(test)]
#[path = "invocation_tests.rs"]
mod tests;
