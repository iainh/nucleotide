// ABOUTME: Language-specific manifest providers for project detection
// ABOUTME: Implements concrete providers for major programming languages and build systems

pub mod cpp;
pub mod csharp;
pub mod go;
pub mod java;
pub mod python;
pub mod rust;
pub mod typescript;

pub use cpp::CppManifestProvider;
pub use csharp::CSharpManifestProvider;
pub use go::GoManifestProvider;
pub use java::JavaManifestProvider;
pub use python::PythonManifestProvider;
pub use rust::RustManifestProvider;
pub use typescript::TypeScriptManifestProvider;

use crate::manifest::ManifestProvider;

/// Get all built-in providers
pub fn builtin_providers() -> Vec<Box<dyn ManifestProvider>> {
    vec![
        Box::new(RustManifestProvider::new()),
        Box::new(PythonManifestProvider::new()),
        Box::new(TypeScriptManifestProvider::new()),
        Box::new(GoManifestProvider::new()),
        Box::new(JavaManifestProvider::new()),
        Box::new(CSharpManifestProvider::new()),
        Box::new(CppManifestProvider::new()),
    ]
}

/// Get providers sorted by priority (highest first)
pub fn builtin_providers_by_priority() -> Vec<Box<dyn ManifestProvider>> {
    let mut providers = builtin_providers();
    providers.sort_by(|a, b| b.priority().cmp(&a.priority()));
    providers
}

/// Get provider names for all built-in providers
pub fn builtin_provider_names() -> Vec<String> {
    builtin_providers()
        .into_iter()
        .map(|p| p.name().as_str().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_providers() {
        let providers = builtin_providers();
        assert!(!providers.is_empty());

        // Check that we have all expected providers
        let names: Vec<String> = providers
            .iter()
            .map(|p| p.name().as_str().to_string())
            .collect();
        assert!(names.contains(&"Cargo.toml".to_string()));
        assert!(names.contains(&"pyproject.toml".to_string()));
        assert!(names.contains(&"package.json".to_string()));
        assert!(names.contains(&"go.mod".to_string()));
        assert!(names.contains(&"pom.xml".to_string()));
        assert!(names.contains(&"project.csproj".to_string()));
        assert!(names.contains(&"CMakeLists.txt".to_string()));
    }

    #[test]
    fn test_providers_by_priority() {
        let providers = builtin_providers_by_priority();

        // Verify sorting by priority
        for i in 1..providers.len() {
            assert!(providers[i - 1].priority() >= providers[i].priority());
        }
    }

    #[test]
    fn test_provider_names() {
        let names = builtin_provider_names();
        assert!(!names.is_empty());
        assert!(names.iter().all(|name| !name.is_empty()));
    }
}
