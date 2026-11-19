/// What a sub-tab's chip says about its process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubTabStatus {
    /// Nothing has run yet (or the pane isn't a terminal).
    Idle,
    /// A command is running.
    Running,
    /// The last command exited with 0.
    Succeeded,
    /// The last command exited with this non-zero code.
    Failed(i32),
}

impl SubTabStatus {
    /// The status of a terminal: `running` wins, else the last finished command's exit code.
    pub fn of_terminal(running: bool, last_exit_code: Option<i32>) -> Self {
        match (running, last_exit_code) {
            (true, _) => Self::Running,
            (false, Some(0)) => Self::Succeeded,
            (false, Some(code)) => Self::Failed(code),
            (false, None) => Self::Idle,
        }
    }

    /// The chip's short status text: a check mark, or a cross with the exit code. Running and
    /// idle sub-tabs show none (running ones get an indicator dot instead).
    pub fn label(self) -> Option<String> {
        match self {
            Self::Idle | Self::Running => None,
            Self::Succeeded => Some("✓".to_owned()),
            Self::Failed(code) => Some(format!("✗ {code}")),
        }
    }

    /// The tooltip line describing the status.
    pub fn description(self) -> Option<String> {
        match self {
            Self::Idle => None,
            Self::Running => Some("Running".to_owned()),
            Self::Succeeded => Some("Last command succeeded".to_owned()),
            Self::Failed(code) => Some(format!("Last command failed with exit code {code}")),
        }
    }
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
