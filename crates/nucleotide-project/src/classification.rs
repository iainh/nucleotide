// ABOUTME: Pure project classification from workspace marker names.
// ABOUTME: Local and remote backends gather names; this module owns their interpretation.

use std::collections::HashSet;

pub use nucleotide_events::ProjectType;

/// Classify a known workspace root from its immediate file names.
///
/// The result is deterministic and preserves independent ecosystems as a mixed
/// project. Markers from the same ecosystem are collapsed, so TypeScript takes
/// precedence over JavaScript and strong C++ markers take precedence over a
/// generic Makefile.
pub fn classify_project_markers<I, S>(markers: I) -> ProjectType
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let markers = markers
        .into_iter()
        .map(|marker| marker.as_ref().to_string())
        .collect::<HashSet<_>>();
    let lower_markers = markers
        .iter()
        .map(|marker| marker.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let has = |marker: &str| markers.contains(marker);
    let has_any = |candidates: &[&str]| candidates.iter().any(|candidate| has(candidate));
    let has_extension = |extension: &str| {
        lower_markers
            .iter()
            .any(|marker| marker.ends_with(extension))
    };

    let mut project_types = Vec::new();
    if has("Cargo.toml") {
        project_types.push(ProjectType::Rust);
    }
    if has_any(&["pyproject.toml", "requirements.txt", "setup.py", "Pipfile"]) {
        project_types.push(ProjectType::Python);
    }
    if has("tsconfig.json") {
        project_types.push(ProjectType::TypeScript);
    } else if has("package.json") {
        project_types.push(ProjectType::JavaScript);
    }
    if has_any(&["go.mod", "go.sum"]) {
        project_types.push(ProjectType::Go);
    }
    if has_extension(".csproj")
        || has_extension(".slnx")
        || has_extension(".sln")
        || has_extension(".fsproj")
        || has_extension(".vbproj")
        || has_any(&[
            "Directory.Build.props",
            "Directory.Build.targets",
            "global.json",
        ])
    {
        project_types.push(ProjectType::CSharp);
    }
    if has_any(&[
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
    ]) {
        project_types.push(ProjectType::Java);
    }

    let has_cpp_marker = has_any(&[
        "CMakeLists.txt",
        "CMakePresets.json",
        "meson.build",
        "conanfile.txt",
        "conanfile.py",
        "vcpkg.json",
        ".clangd",
        "compile_commands.json",
    ]);
    if has_cpp_marker {
        project_types.push(ProjectType::Cpp);
    } else if project_types.is_empty() && has_any(&["Makefile", "makefile", "GNUmakefile"]) {
        project_types.push(ProjectType::C);
    }

    match project_types.len() {
        0 => ProjectType::Unknown,
        1 => project_types.pop().expect("one project type"),
        _ => ProjectType::Mixed(project_types),
    }
}

/// Return the primary Helix language IDs represented by a project type.
pub fn project_language_ids(project_type: &ProjectType) -> Vec<String> {
    fn collect(project_type: &ProjectType, languages: &mut Vec<String>) {
        let language = match project_type {
            ProjectType::Rust => Some("rust"),
            ProjectType::TypeScript => Some("typescript"),
            ProjectType::JavaScript => Some("javascript"),
            ProjectType::Python => Some("python"),
            ProjectType::Go => Some("go"),
            ProjectType::Java => Some("java"),
            ProjectType::CSharp => Some("c-sharp"),
            ProjectType::C => Some("c"),
            ProjectType::Cpp => Some("cpp"),
            ProjectType::Mixed(project_types) => {
                for project_type in project_types {
                    collect(project_type, languages);
                }
                None
            }
            ProjectType::Other(name) => {
                let language = name.to_ascii_lowercase().replace(' ', "_");
                if !languages.contains(&language) {
                    languages.push(language);
                }
                None
            }
            ProjectType::Unknown => None,
        };

        if let Some(language) = language
            && !languages.iter().any(|candidate| candidate == language)
        {
            languages.push(language.to_string());
        }
    }

    let mut languages = Vec::new();
    collect(project_type, &mut languages);
    languages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_unknown_and_single_projects() {
        assert_eq!(
            classify_project_markers([] as [&str; 0]),
            ProjectType::Unknown
        );
        assert_eq!(classify_project_markers(["Cargo.toml"]), ProjectType::Rust);
        assert_eq!(classify_project_markers(["pom.xml"]), ProjectType::Java);
        assert_eq!(classify_project_markers(["Demo.slnx"]), ProjectType::CSharp);
        assert_eq!(project_language_ids(&ProjectType::CSharp), vec!["c-sharp"]);
        assert_eq!(
            classify_project_markers(["CMakeLists.txt"]),
            ProjectType::Cpp
        );
        assert_eq!(classify_project_markers(["Makefile"]), ProjectType::C);
    }

    #[test]
    fn typescript_takes_precedence_over_javascript_markers() {
        assert_eq!(
            classify_project_markers(["package.json", "tsconfig.json"]),
            ProjectType::TypeScript
        );
    }

    #[test]
    fn preserves_independent_ecosystems_in_priority_order() {
        let project_type =
            classify_project_markers(["requirements.txt", "package.json", "Cargo.toml", "go.mod"]);

        assert_eq!(
            project_type,
            ProjectType::Mixed(vec![
                ProjectType::Rust,
                ProjectType::Python,
                ProjectType::JavaScript,
                ProjectType::Go,
            ])
        );
        assert_eq!(
            project_language_ids(&project_type),
            vec!["rust", "python", "javascript", "go"]
        );
    }

    #[test]
    fn generic_makefile_does_not_add_a_second_project_ecosystem() {
        assert_eq!(
            classify_project_markers(["go.mod", "Makefile"]),
            ProjectType::Go
        );
        assert_eq!(
            classify_project_markers(["CMakeLists.txt", "Makefile"]),
            ProjectType::Cpp
        );
    }

    #[test]
    fn marker_order_does_not_change_classification() {
        let local = classify_project_markers(["Cargo.toml", "pyproject.toml"]);
        let remote = classify_project_markers(["pyproject.toml", "Cargo.toml"]);

        assert_eq!(local, remote);
    }
}
