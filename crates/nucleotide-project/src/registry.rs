// ABOUTME: Provider registration and lookup system for manifest providers
// ABOUTME: Manages global registry with priority-based ordering and efficient search

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use crate::error::{ProjectError, Result};
use crate::manifest::{
    FsDelegate, ManifestDelegate, ManifestName, ManifestProvider, ManifestQuery,
};

/// Global registry for manifest providers
#[derive(Default)]
pub struct ManifestProviders {
    providers: RwLock<HashMap<ManifestName, Arc<dyn ManifestProvider>>>,
    ordered_providers: RwLock<Vec<Arc<dyn ManifestProvider>>>,
}

/// Global instance of the provider registry
static GLOBAL_PROVIDERS: OnceLock<Arc<ManifestProviders>> = OnceLock::new();

impl ManifestProviders {
    /// Create a new provider registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the global provider registry
    pub fn global() -> Arc<Self> {
        GLOBAL_PROVIDERS
            .get_or_init(|| Arc::new(Self::new()))
            .clone()
    }

    /// Register a manifest provider
    pub fn register(&self, provider: Box<dyn ManifestProvider>) {
        let provider: Arc<dyn ManifestProvider> = Arc::from(provider);
        let name = provider.name();

        nucleotide_logging::info!(
            provider_name = %name,
            priority = provider.priority(),
            "Registering manifest provider"
        );

        // Check for duplicates
        {
            let providers = self.providers.read().unwrap();
            if providers.contains_key(&name) {
                nucleotide_logging::warn!(
                    provider_name = %name,
                    "Provider already registered, replacing existing"
                );
            }
        }

        // Insert into main registry
        {
            let mut providers = self.providers.write().unwrap();
            providers.insert(name, provider.clone());
        }

        // Update ordered list (sorted by priority, descending)
        {
            let mut ordered = self.ordered_providers.write().unwrap();
            ordered.push(provider);
            ordered.sort_by_key(|p| std::cmp::Reverse(p.priority()));
        }

        nucleotide_logging::debug!("Provider registration completed successfully");
    }

    /// Unregister a provider by name
    pub fn unregister(&self, name: &ManifestName) -> Result<()> {
        nucleotide_logging::info!(provider_name = %name, "Unregistering manifest provider");

        // Remove from main registry
        let removed = {
            let mut providers = self.providers.write().unwrap();
            providers.remove(name)
        };

        if removed.is_none() {
            return Err(ProjectError::provider_not_found(name.as_str()));
        }

        // Remove from ordered list
        {
            let mut ordered = self.ordered_providers.write().unwrap();
            ordered.retain(|p| p.name() != *name);
        }

        nucleotide_logging::debug!(provider_name = %name, "Provider unregistered successfully");
        Ok(())
    }

    /// Get a specific provider by name
    pub fn get(&self, name: &ManifestName) -> Option<Arc<dyn ManifestProvider>> {
        let providers = self.providers.read().unwrap();
        providers.get(name).cloned()
    }

    /// Get all registered providers, ordered by priority
    pub fn get_all(&self) -> Vec<Arc<dyn ManifestProvider>> {
        let ordered = self.ordered_providers.read().unwrap();
        ordered.clone()
    }

    /// List all registered provider names
    pub fn list_providers(&self) -> Vec<ManifestName> {
        let providers = self.providers.read().unwrap();
        let mut names: Vec<_> = providers.keys().cloned().collect();
        names.sort();
        names
    }

    /// Check if a provider is registered
    pub fn is_registered(&self, name: &ManifestName) -> bool {
        let providers = self.providers.read().unwrap();
        providers.contains_key(name)
    }

    /// Get the number of registered providers
    pub fn count(&self) -> usize {
        let providers = self.providers.read().unwrap();
        providers.len()
    }

    /// Clear all registered providers
    pub fn clear(&self) {
        nucleotide_logging::info!("Clearing all manifest providers");

        let mut providers = self.providers.write().unwrap();
        let mut ordered = self.ordered_providers.write().unwrap();

        providers.clear();
        ordered.clear();

        nucleotide_logging::debug!("All providers cleared successfully");
    }

    /// Detect project root for a given file path
    pub async fn detect_project_root(
        &self,
        file_path: &Path,
        max_depth: Option<usize>,
    ) -> Result<Option<PathBuf>> {
        let delegate = Arc::new(FsDelegate);
        self.detect_project_root_with_delegate(file_path, max_depth, delegate)
            .await
    }

    /// Detect project root with custom delegate
    pub async fn detect_project_root_with_delegate(
        &self,
        file_path: &Path,
        max_depth: Option<usize>,
        delegate: Arc<dyn ManifestDelegate>,
    ) -> Result<Option<PathBuf>> {
        let max_depth = max_depth.unwrap_or(20);
        let query = ManifestQuery::new(file_path, max_depth, delegate);

        nucleotide_logging::debug!(
            file_path = %file_path.display(),
            max_depth = max_depth,
            provider_count = self.count(),
            "Starting project root detection"
        );

        let providers = self.get_all();
        for provider in providers {
            nucleotide_logging::trace!(
                provider_name = %provider.name(),
                priority = provider.priority(),
                "Trying provider"
            );

            match provider.search(query.clone()).await {
                Ok(Some(root)) => {
                    nucleotide_logging::info!(
                        provider_name = %provider.name(),
                        project_root = %root.display(),
                        file_path = %file_path.display(),
                        "Project root detected"
                    );
                    return Ok(Some(root));
                }
                Ok(None) => {
                    nucleotide_logging::trace!(
                        provider_name = %provider.name(),
                        "No project root found by provider"
                    );
                }
                Err(e) if e.is_recoverable() => {
                    nucleotide_logging::warn!(
                        provider_name = %provider.name(),
                        error = %e,
                        "Recoverable error in provider, continuing search"
                    );
                }
                Err(e) => {
                    nucleotide_logging::error!(
                        provider_name = %provider.name(),
                        error = %e,
                        "Fatal error in provider"
                    );
                    return Err(e);
                }
            }
        }

        nucleotide_logging::debug!(
            file_path = %file_path.display(),
            "No project root detected by any provider"
        );
        Ok(None)
    }

    /// Detect project type (manifest name) for a given file path
    pub async fn detect_project_type(
        &self,
        file_path: &Path,
        max_depth: Option<usize>,
    ) -> Result<Option<ManifestName>> {
        let delegate = Arc::new(FsDelegate);
        let max_depth = max_depth.unwrap_or(20);
        let query = ManifestQuery::new(file_path, max_depth, delegate);

        let providers = self.get_all();
        for provider in providers {
            match provider.search(query.clone()).await {
                Ok(Some(_)) => {
                    return Ok(Some(provider.name()));
                }
                Ok(None) => continue,
                Err(e) if e.is_recoverable() => continue,
                Err(e) => return Err(e),
            }
        }

        Ok(None)
    }

    /// Get providers that match specific file patterns
    pub fn providers_for_patterns(&self, patterns: &[String]) -> Vec<Arc<dyn ManifestProvider>> {
        let providers = self.get_all();
        providers
            .into_iter()
            .filter(|provider| {
                let provider_patterns = provider.file_patterns();
                patterns
                    .iter()
                    .any(|pattern| provider_patterns.iter().any(|pp| pp == pattern))
            })
            .collect()
    }

    /// Bulk register multiple providers
    pub fn register_multiple(&self, providers: Vec<Box<dyn ManifestProvider>>) {
        nucleotide_logging::info!(count = providers.len(), "Bulk registering providers");

        for provider in providers {
            self.register(provider);
        }

        nucleotide_logging::debug!("Bulk provider registration completed");
    }
}

/// Builder for configuring provider registry
pub struct ProviderRegistryBuilder {
    providers: Vec<Box<dyn ManifestProvider>>,
}

impl ProviderRegistryBuilder {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    pub fn with_provider(mut self, provider: Box<dyn ManifestProvider>) -> Self {
        self.providers.push(provider);
        self
    }

    pub fn with_builtin_providers(mut self) -> Self {
        self.providers.extend(crate::providers::builtin_providers());
        self
    }

    pub fn build(self) -> Arc<ManifestProviders> {
        let registry = Arc::new(ManifestProviders::new());
        registry.register_multiple(self.providers);
        registry
    }

    pub fn build_and_set_global(self) -> Arc<ManifestProviders> {
        let registry = self.build();

        // Replace global instance if not already set
        if GLOBAL_PROVIDERS.get().is_none() {
            let _ = GLOBAL_PROVIDERS.set(registry.clone());
        }

        registry
    }
}

impl Default for ProviderRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{BaseManifestProvider, ProjectMetadata};
    use async_trait::async_trait;
    use tempfile::TempDir;

    struct TestProvider {
        base: BaseManifestProvider,
    }

    impl TestProvider {
        fn new(name: &str, patterns: Vec<String>) -> Self {
            Self {
                base: BaseManifestProvider::new(name, patterns),
            }
        }
    }

    #[async_trait]
    impl ManifestProvider for TestProvider {
        fn name(&self) -> ManifestName {
            self.base.name.clone()
        }

        fn priority(&self) -> u32 {
            self.base.priority
        }

        fn file_patterns(&self) -> Vec<String> {
            self.base.file_patterns.clone()
        }

        async fn search(&self, query: ManifestQuery) -> Result<Option<PathBuf>> {
            self.base.search_patterns(&query).await
        }

        async fn validate_manifest(
            &self,
            _path: &Path,
            _delegate: &dyn ManifestDelegate,
        ) -> Result<bool> {
            Ok(true)
        }

        async fn get_project_metadata(
            &self,
            _manifest_path: &Path,
            _delegate: &dyn ManifestDelegate,
        ) -> Result<ProjectMetadata> {
            Ok(ProjectMetadata::default())
        }
    }

    #[tokio::test]
    async fn test_provider_registration() {
        let registry = ManifestProviders::new();

        let provider = TestProvider::new("test.toml", vec!["test.toml".to_string()]);
        let name = provider.name();

        assert!(!registry.is_registered(&name));
        assert_eq!(registry.count(), 0);

        registry.register(Box::new(provider));

        assert!(registry.is_registered(&name));
        assert_eq!(registry.count(), 1);

        let retrieved = registry.get(&name);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name(), name);
    }

    #[tokio::test]
    async fn test_provider_priority_ordering() {
        let registry = ManifestProviders::new();

        let mut provider1 = TestProvider::new("low.toml", vec!["low.toml".to_string()]);
        provider1.base = provider1.base.with_priority(50);

        let mut provider2 = TestProvider::new("high.toml", vec!["high.toml".to_string()]);
        provider2.base = provider2.base.with_priority(200);

        let mut provider3 = TestProvider::new("medium.toml", vec!["medium.toml".to_string()]);
        provider3.base = provider3.base.with_priority(100);

        registry.register(Box::new(provider1));
        registry.register(Box::new(provider2));
        registry.register(Box::new(provider3));

        let providers = registry.get_all();
        assert_eq!(providers.len(), 3);

        // Should be ordered by priority (high to low)
        assert_eq!(providers[0].name().as_str(), "high.toml");
        assert_eq!(providers[1].name().as_str(), "medium.toml");
        assert_eq!(providers[2].name().as_str(), "low.toml");
    }

    #[tokio::test]
    async fn test_provider_unregistration() {
        let registry = ManifestProviders::new();

        let provider = TestProvider::new("test.toml", vec!["test.toml".to_string()]);
        let name = provider.name();

        registry.register(Box::new(provider));
        assert!(registry.is_registered(&name));

        registry.unregister(&name).unwrap();
        assert!(!registry.is_registered(&name));
        assert_eq!(registry.count(), 0);

        // Unregistering non-existent provider should fail
        let result = registry.unregister(&name);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_project_detection() {
        let temp_dir = TempDir::new().unwrap();
        let manifest_path = temp_dir.path().join("test.toml");
        tokio::fs::write(&manifest_path, "[package]\nname = \"test\"")
            .await
            .unwrap();

        let nested_dir = temp_dir.path().join("src");
        tokio::fs::create_dir_all(&nested_dir).await.unwrap();
        let file_path = nested_dir.join("main.rs");
        tokio::fs::write(&file_path, "fn main() {}").await.unwrap();

        let registry = ManifestProviders::new();
        let provider = TestProvider::new("test.toml", vec!["test.toml".to_string()]);
        registry.register(Box::new(provider));

        let detected_root = registry
            .detect_project_root(&file_path, None)
            .await
            .unwrap();
        assert!(detected_root.is_some());
        assert_eq!(detected_root.unwrap(), temp_dir.path());

        let project_type = registry
            .detect_project_type(&file_path, None)
            .await
            .unwrap();
        assert!(project_type.is_some());
        assert_eq!(project_type.unwrap().as_str(), "test.toml");
    }

    #[tokio::test]
    async fn test_provider_registry_builder() {
        let provider1 = TestProvider::new("test1.toml", vec!["test1.toml".to_string()]);
        let provider2 = TestProvider::new("test2.toml", vec!["test2.toml".to_string()]);

        let registry = ProviderRegistryBuilder::new()
            .with_provider(Box::new(provider1))
            .with_provider(Box::new(provider2))
            .build();

        assert_eq!(registry.count(), 2);
        assert!(registry.is_registered(&ManifestName::new("test1.toml")));
        assert!(registry.is_registered(&ManifestName::new("test2.toml")));
    }

    #[tokio::test]
    async fn test_global_registry() {
        let global1 = ManifestProviders::global();
        let global2 = ManifestProviders::global();

        // Should be the same instance
        assert!(Arc::ptr_eq(&global1, &global2));
    }
}
