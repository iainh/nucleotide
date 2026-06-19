// ABOUTME: Run/task domain events and command model for GUI runnables
// ABOUTME: Normalizes discovered run targets into terminal-executable tasks

use std::path::PathBuf;

/// Strongly-typed identifier for GUI run requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RunId(pub u64);

/// The user-visible category of a runnable task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunKind {
    Run,
    Test,
    TestModule,
    Debug,
}

/// Lifecycle state tracked independently from terminal output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunStatus {
    Pending,
    Running,
    Finished,
    Failed,
    Cancelled,
}

/// Source range associated with a runnable.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceLocation {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
}

/// Concrete command specification that can be executed in a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
        }
    }

    pub fn with_args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn with_env(mut self, env: impl IntoIterator<Item = (String, String)>) -> Self {
        self.env = env.into_iter().collect();
        self
    }
}

/// Declarative, source-linked runnable shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskTemplate {
    pub label: String,
    pub kind: RunKind,
    pub command: CommandSpec,
    pub source: Option<SourceLocation>,
    pub tags: Vec<String>,
}

/// Fully resolved task ready for execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTask {
    pub template: TaskTemplate,
    pub command: CommandSpec,
}

impl ResolvedTask {
    pub fn label(&self) -> &str {
        &self.template.label
    }

    pub fn kind(&self) -> RunKind {
        self.template.kind
    }

    pub fn source(&self) -> Option<&SourceLocation> {
        self.template.source.as_ref()
    }
}

/// Run domain events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Requested {
        task: ResolvedTask,
    },
    Started {
        id: RunId,
        task: ResolvedTask,
        terminal_id: Option<crate::terminal::TerminalId>,
    },
    StatusChanged {
        id: RunId,
        status: RunStatus,
    },
    Finished {
        id: RunId,
        code: Option<i32>,
    },
    CancelRequested {
        id: RunId,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_spec_builders_preserve_execution_parts() {
        let spec = CommandSpec::new("cargo")
            .with_args(["test", "sample"])
            .with_cwd("/workspace")
            .with_env([("RUST_LOG".to_string(), "info".to_string())]);

        assert_eq!(spec.program, "cargo");
        assert_eq!(spec.args, vec!["test", "sample"]);
        assert_eq!(spec.cwd, Some(PathBuf::from("/workspace")));
        assert_eq!(spec.env, vec![("RUST_LOG".to_string(), "info".to_string())]);
    }
}
