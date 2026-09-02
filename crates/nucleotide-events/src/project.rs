// ABOUTME: Canonical project classification shared across application domains.
// ABOUTME: Keeps project identity independent from filesystem and UI concerns.

/// Type of project detected from workspace markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectType {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
    Java,
    CSharp,
    C,
    Cpp,
    Mixed(Vec<ProjectType>),
    Other(String),
    Unknown,
}
