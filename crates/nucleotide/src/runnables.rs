// ABOUTME: Runnable discovery and command normalization for GUI run actions
// ABOUTME: Provides local Rust/Cargo discovery plus rust-analyzer runnable conversion

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use helix_lsp::lsp;
use nucleotide_events::v2::run::{
    CommandSpec, ResolvedTask, RunKind, SourceLocation, TaskTemplate,
};
use regex::Regex;
use serde::{Deserialize, Serialize};

const TAG_FILE_TESTS: &str = "file-tests";
const TAG_LOCAL_DISCOVERY: &str = "local-rust";
const TAG_RUST_ANALYZER: &str = "rust-analyzer";

static FN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\b")
        .expect("valid Rust function regex")
});

static MOD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\b")
        .expect("valid Rust module regex")
});

#[derive(Debug, Clone)]
pub struct RunnableDocument {
    pub path: PathBuf,
    pub text: String,
    pub cursor_line: usize,
    pub project_root: Option<PathBuf>,
}

pub fn discover_local_rust_runnables(document: &RunnableDocument) -> Vec<ResolvedTask> {
    if document.path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
        return Vec::new();
    }

    let Some(cargo_root) = document
        .project_root
        .as_deref()
        .filter(|root| root.join("Cargo.toml").is_file())
        .map(Path::to_path_buf)
        .or_else(|| find_cargo_root(&document.path))
    else {
        return Vec::new();
    };

    let mut tasks = Vec::new();
    let mut test_count = 0usize;
    let mut pending_test_attr: Option<usize> = None;

    for (line_idx, line) in document.text.lines().enumerate() {
        let trimmed = line.trim_start();

        if is_test_attribute(trimmed) {
            pending_test_attr = Some(line_idx);
            continue;
        }

        if let Some(attr_line) = pending_test_attr {
            if trimmed.is_empty() || trimmed.starts_with("#[") {
                continue;
            }

            if let Some(name) = function_name(line) {
                tasks.push(cargo_task(
                    format!("Run Test {name}"),
                    RunKind::Test,
                    [
                        "test".to_string(),
                        name,
                        "--".to_string(),
                        "--nocapture".to_string(),
                    ],
                    &cargo_root,
                    Some(SourceLocation {
                        path: document.path.clone(),
                        line: attr_line,
                        column: 0,
                    }),
                    [TAG_LOCAL_DISCOVERY.to_string()],
                ));
                test_count += 1;
            }

            pending_test_attr = None;
        }

        if function_name(line).as_deref() == Some("main")
            && let Some(args) = cargo_run_args_for_path(&document.path, &cargo_root)
        {
            tasks.push(cargo_task(
                run_label_for_path(&document.path, &cargo_root),
                RunKind::Run,
                args,
                &cargo_root,
                Some(SourceLocation {
                    path: document.path.clone(),
                    line: line_idx,
                    column: 0,
                }),
                [TAG_LOCAL_DISCOVERY.to_string()],
            ));
        }

        if let Some(module_name) = module_name(line)
            && module_name == "tests"
        {
            tasks.push(cargo_task(
                "Run Test Module tests".to_string(),
                RunKind::TestModule,
                [
                    "test".to_string(),
                    module_name,
                    "--".to_string(),
                    "--nocapture".to_string(),
                ],
                &cargo_root,
                Some(SourceLocation {
                    path: document.path.clone(),
                    line: line_idx,
                    column: 0,
                }),
                [TAG_LOCAL_DISCOVERY.to_string()],
            ));
        }
    }

    if test_count > 0 || is_integration_test_file(&document.path, &cargo_root) {
        tasks.insert(
            0,
            cargo_task(
                file_tests_label(&document.path, &cargo_root),
                RunKind::TestModule,
                cargo_file_test_args(&document.path, &cargo_root),
                &cargo_root,
                Some(SourceLocation {
                    path: document.path.clone(),
                    line: 0,
                    column: 0,
                }),
                [TAG_LOCAL_DISCOVERY.to_string(), TAG_FILE_TESTS.to_string()],
            ),
        );
    }

    dedupe_tasks(tasks)
}

pub fn nearest_runnable(
    tasks: &[ResolvedTask],
    cursor_line: usize,
    include_file_wide: bool,
) -> Option<ResolvedTask> {
    let mut before_or_on_cursor: Vec<&ResolvedTask> = tasks
        .iter()
        .filter(|task| include_file_wide || !has_tag(task, TAG_FILE_TESTS))
        .filter(|task| {
            task.source()
                .is_some_and(|source| source.line <= cursor_line)
        })
        .collect();
    before_or_on_cursor.sort_by_key(|task| std::cmp::Reverse(task.source().unwrap().line));

    before_or_on_cursor
        .into_iter()
        .next()
        .or_else(|| {
            tasks
                .iter()
                .filter(|task| include_file_wide || !has_tag(task, TAG_FILE_TESTS))
                .filter(|task| task.source().is_some())
                .min_by_key(|task| task.source().unwrap().line)
        })
        .cloned()
}

pub fn file_tests_runnable(tasks: &[ResolvedTask]) -> Option<ResolvedTask> {
    tasks
        .iter()
        .find(|task| has_tag(task, TAG_FILE_TESTS))
        .cloned()
}

pub fn is_file_tests_runnable(task: &ResolvedTask) -> bool {
    has_tag(task, TAG_FILE_TESTS)
}

pub fn merge_runnable_tasks(
    mut preferred: Vec<ResolvedTask>,
    fallback: Vec<ResolvedTask>,
) -> Vec<ResolvedTask> {
    preferred.extend(fallback);
    dedupe_tasks(preferred)
}

pub fn shell_command_line(command: &CommandSpec) -> String {
    std::iter::once(command.program.as_str())
        .chain(command.args.iter().map(String::as_str))
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn task_preview_text(task: &ResolvedTask) -> String {
    let cwd = task
        .command
        .cwd
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "<current directory>".to_string());
    let env = if task.command.env.is_empty() {
        String::new()
    } else {
        let mut entries = task.command.env.clone();
        entries.sort_by(|(left, _), (right, _)| left.cmp(right));
        let body = entries
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("\n\nEnvironment:\n{body}")
    };

    format!(
        "{}\n\nKind: {:?}\nWorking directory: {}\nCommand: {}{}",
        task.label(),
        task.kind(),
        cwd,
        shell_command_line(&task.command),
        env
    )
}

pub fn runnable_to_task_template(runnable: RaRunnable) -> ResolvedTask {
    let source = runnable
        .location
        .as_ref()
        .and_then(source_from_location_link);
    let kind = run_kind_from_label(&runnable.label);
    let command = match runnable.args {
        RaRunnableArgs::Cargo(cargo) => {
            let mut program = cargo.override_cargo.unwrap_or_else(|| "cargo".to_string());
            let mut args = Vec::new();

            if program.contains(char::is_whitespace) {
                let parts = program
                    .split_whitespace()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                if let Some((first, rest)) = parts.split_first() {
                    program = first.clone();
                    args.extend(rest.iter().cloned());
                }
            }

            args.extend(cargo.cargo_args);
            if !cargo.executable_args.is_empty() {
                args.push("--".to_string());
                args.extend(cargo.executable_args);
            }

            CommandSpec {
                program,
                args,
                cwd: Some(cargo.workspace_root.unwrap_or(cargo.cwd)),
                env: sorted_env(cargo.environment),
            }
        }
        RaRunnableArgs::Shell(shell) => CommandSpec {
            program: shell.program,
            args: shell.args,
            cwd: Some(shell.cwd),
            env: sorted_env(shell.environment),
        },
    };

    let template = TaskTemplate {
        label: runnable.label,
        kind,
        command: command.clone(),
        source,
        tags: vec![TAG_RUST_ANALYZER.to_string()],
    };

    ResolvedTask { template, command }
}

pub enum RaRunnablesRequest {}

impl lsp::request::Request for RaRunnablesRequest {
    type Params = RunnablesParams;
    type Result = Vec<RaRunnable>;

    const METHOD: &'static str = "experimental/runnables";
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunnablesParams {
    pub text_document: lsp::TextDocumentIdentifier,
    #[serde(default)]
    pub position: Option<lsp::Position>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RaRunnable {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<lsp::LocationLink>,
    #[serde(flatten)]
    pub args: RaRunnableArgs,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "kind", content = "args")]
#[serde(rename_all = "lowercase")]
pub enum RaRunnableArgs {
    Cargo(RaCargoRunnableArgs),
    Shell(RaShellRunnableArgs),
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RaCargoRunnableArgs {
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub environment: HashMap<String, String>,
    pub cwd: PathBuf,
    #[serde(default)]
    pub override_cargo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<PathBuf>,
    #[serde(default)]
    pub cargo_args: Vec<String>,
    #[serde(default)]
    pub executable_args: Vec<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RaShellRunnableArgs {
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub environment: HashMap<String, String>,
    pub cwd: PathBuf,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
}

fn cargo_task(
    label: String,
    kind: RunKind,
    args: impl IntoIterator<Item = String>,
    cwd: &Path,
    source: Option<SourceLocation>,
    tags: impl IntoIterator<Item = String>,
) -> ResolvedTask {
    let command = CommandSpec::new("cargo")
        .with_args(args)
        .with_cwd(cwd.to_path_buf());
    let template = TaskTemplate {
        label,
        kind,
        command: command.clone(),
        source,
        tags: tags.into_iter().collect(),
    };

    ResolvedTask { template, command }
}

fn function_name(line: &str) -> Option<String> {
    FN_RE
        .captures(line)
        .and_then(|captures| captures.get(1))
        .map(|name| name.as_str().to_string())
}

fn module_name(line: &str) -> Option<String> {
    MOD_RE
        .captures(line)
        .and_then(|captures| captures.get(1))
        .map(|name| name.as_str().to_string())
}

fn is_test_attribute(trimmed: &str) -> bool {
    trimmed.starts_with("#[test")
        || (trimmed.starts_with("#[") && trimmed.contains("::test"))
        || trimmed.starts_with("#[rstest")
}

fn find_cargo_root(path: &Path) -> Option<PathBuf> {
    let mut current = if path.is_dir() { path } else { path.parent()? };

    loop {
        if current.join("Cargo.toml").is_file() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

fn cargo_run_args_for_path(path: &Path, cargo_root: &Path) -> Option<Vec<String>> {
    let relative = path.strip_prefix(cargo_root).ok()?;
    let components = relative.components().collect::<Vec<_>>();

    match components.as_slice() {
        [Component::Normal(src), Component::Normal(file)]
            if *src == "src" && *file == "main.rs" =>
        {
            Some(vec!["run".to_string()])
        }
        [
            Component::Normal(src),
            Component::Normal(bin),
            Component::Normal(file),
        ] if *src == "src" && *bin == "bin" => {
            path_stem(file).map(|name| vec!["run".to_string(), "--bin".to_string(), name])
        }
        [Component::Normal(examples), Component::Normal(file)] if *examples == "examples" => {
            path_stem(file).map(|name| vec!["run".to_string(), "--example".to_string(), name])
        }
        _ => Some(vec!["run".to_string()]),
    }
}

fn run_label_for_path(path: &Path, cargo_root: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(cargo_root) {
        if relative == Path::new("src/main.rs") {
            return "Run Binary".to_string();
        }

        if let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) {
            return format!("Run {name}");
        }
    }

    "Run".to_string()
}

fn cargo_file_test_args(path: &Path, cargo_root: &Path) -> Vec<String> {
    if is_integration_test_file(path, cargo_root)
        && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
    {
        return vec![
            "test".to_string(),
            "--test".to_string(),
            stem.to_string(),
            "--".to_string(),
            "--nocapture".to_string(),
        ];
    }

    vec![
        "test".to_string(),
        "--".to_string(),
        "--nocapture".to_string(),
    ]
}

fn file_tests_label(path: &Path, cargo_root: &Path) -> String {
    let name = path
        .strip_prefix(cargo_root)
        .unwrap_or(path)
        .display()
        .to_string();
    format!("Run File Tests {name}")
}

fn is_integration_test_file(path: &Path, cargo_root: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(cargo_root) else {
        return false;
    };

    let components = relative.components().collect::<Vec<_>>();
    matches!(
        components.as_slice(),
        [Component::Normal(tests), Component::Normal(file)]
            if *tests == "tests" && Path::new(file).extension().and_then(|ext| ext.to_str()) == Some("rs")
    )
}

fn path_stem(name: &std::ffi::OsStr) -> Option<String> {
    Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(ToString::to_string)
}

fn dedupe_tasks(tasks: Vec<ResolvedTask>) -> Vec<ResolvedTask> {
    let mut result = Vec::new();
    for task in tasks {
        if !result.iter().any(|existing: &ResolvedTask| {
            existing.label() == task.label()
                && existing.command.program == task.command.program
                && existing.command.args == task.command.args
        }) {
            result.push(task);
        }
    }
    result
}

fn has_tag(task: &ResolvedTask, tag: &str) -> bool {
    task.template.tags.iter().any(|candidate| candidate == tag)
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    if value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'/' | b':' | b'=' | b'+' | b',' | b'@' | b'%'
            )
    }) {
        return value.to_string();
    }

    format!("'{}'", value.replace('\'', r#"'\''"#))
}

fn sorted_env(environment: HashMap<String, String>) -> Vec<(String, String)> {
    let mut env = environment.into_iter().collect::<Vec<_>>();
    env.sort_by(|(left, _), (right, _)| left.cmp(right));
    env
}

fn source_from_location_link(location: &lsp::LocationLink) -> Option<SourceLocation> {
    let path = location.target_uri.to_file_path().ok()?;
    Some(SourceLocation {
        path,
        line: location.target_range.start.line as usize,
        column: location.target_range.start.character as usize,
    })
}

fn run_kind_from_label(label: &str) -> RunKind {
    if label.starts_with("test-mod ") {
        RunKind::TestModule
    } else if label.starts_with("test ") {
        RunKind::Test
    } else {
        RunKind::Run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(path: &str, text: &str, cursor_line: usize, root: &str) -> RunnableDocument {
        RunnableDocument {
            path: PathBuf::from(path),
            text: text.to_string(),
            cursor_line,
            project_root: Some(PathBuf::from(root)),
        }
    }

    #[test]
    fn local_discovery_finds_main_and_tests() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        let file = temp.path().join("src/main.rs");
        std::fs::write(&file, "").unwrap();
        let text = r#"
fn main() {}

#[test]
fn parses_input() {}
"#;

        let tasks = discover_local_rust_runnables(&doc(
            file.to_str().unwrap(),
            text,
            4,
            temp.path().to_str().unwrap(),
        ));

        assert!(tasks.iter().any(|task| task.label() == "Run Binary"));
        assert!(
            tasks
                .iter()
                .any(|task| task.label() == "Run Test parses_input")
        );
        assert!(tasks.iter().any(|task| has_tag(task, TAG_FILE_TESTS)));
        assert!(
            tasks
                .iter()
                .any(|task| task.label().starts_with("Run File Tests")
                    && is_file_tests_runnable(task))
        );
    }

    #[test]
    fn nearest_runnable_prefers_previous_source_target() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        let file = temp.path().join("src/lib.rs");
        std::fs::write(&file, "").unwrap();
        let text = r#"
#[test]
fn first() {}

#[test]
fn second() {}
"#;

        let tasks = discover_local_rust_runnables(&doc(
            file.to_str().unwrap(),
            text,
            5,
            temp.path().to_str().unwrap(),
        ));

        let nearest = nearest_runnable(&tasks, 5, false).unwrap();
        assert_eq!(nearest.label(), "Run Test second");
    }

    #[test]
    fn integration_test_file_uses_cargo_test_target() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join("tests")).unwrap();
        let file = temp.path().join("tests/api.rs");
        std::fs::write(&file, "").unwrap();

        let tasks = discover_local_rust_runnables(&doc(
            file.to_str().unwrap(),
            "",
            0,
            temp.path().to_str().unwrap(),
        ));
        let file_task = file_tests_runnable(&tasks).unwrap();

        assert_eq!(
            file_task.command.args,
            vec!["test", "--test", "api", "--", "--nocapture"]
        );
    }

    #[test]
    fn shell_command_line_quotes_arguments() {
        let command = CommandSpec::new("cargo")
            .with_args(["test", "name with spaces", "it's_ok"])
            .with_cwd("/workspace");

        assert_eq!(
            shell_command_line(&command),
            "cargo test 'name with spaces' 'it'\\''s_ok'"
        );
    }

    #[test]
    fn rust_analyzer_cargo_runnable_converts_to_command_spec() {
        let raw = r#"
        {
          "label": "test tests::parses",
          "kind": "cargo",
          "args": {
            "environment": {"RUST_BACKTRACE": "1"},
            "cwd": "/workspace/crate",
            "workspaceRoot": "/workspace",
            "cargoArgs": ["test", "--package", "demo", "parses"],
            "executableArgs": ["--nocapture"]
          }
        }
        "#;

        let runnable: RaRunnable = serde_json::from_str(raw).unwrap();
        let task = runnable_to_task_template(runnable);

        assert_eq!(task.kind(), RunKind::Test);
        assert_eq!(task.command.program, "cargo");
        assert_eq!(task.command.cwd, Some(PathBuf::from("/workspace")));
        assert_eq!(
            task.command.args,
            vec!["test", "--package", "demo", "parses", "--", "--nocapture"]
        );
        assert_eq!(
            task.command.env,
            vec![("RUST_BACKTRACE".into(), "1".into())]
        );
    }

    #[test]
    fn rust_analyzer_shell_runnable_converts_to_command_spec() {
        let raw = r#"
        {
          "label": "run shell task",
          "kind": "shell",
          "args": {
            "environment": {},
            "cwd": "/workspace",
            "program": "just",
            "args": ["test"]
          }
        }
        "#;

        let runnable: RaRunnable = serde_json::from_str(raw).unwrap();
        let task = runnable_to_task_template(runnable);

        assert_eq!(task.kind(), RunKind::Run);
        assert_eq!(task.command.program, "just");
        assert_eq!(task.command.args, vec!["test"]);
        assert_eq!(task.command.cwd, Some(PathBuf::from("/workspace")));
    }
}
