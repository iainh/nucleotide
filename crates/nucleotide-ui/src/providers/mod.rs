// ABOUTME: Typed global UI state shared by components that cannot access a GPUI context.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

pub mod config_provider;
pub mod theme_provider;

pub use config_provider::*;
pub use theme_provider::*;

static PROVIDER_CONTEXT: OnceLock<Arc<RwLock<ProviderContext>>> = OnceLock::new();

pub fn init_provider_system() {
    PROVIDER_CONTEXT.get_or_init(|| Arc::new(RwLock::new(ProviderContext::default())));
}

pub fn with_provider_context<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&ProviderContext) -> R,
{
    PROVIDER_CONTEXT
        .get()
        .and_then(|context| context.read().ok().map(|guard| f(&guard)))
}

pub fn update_provider_context<F>(f: F) -> bool
where
    F: FnOnce(&mut ProviderContext),
{
    PROVIDER_CONTEXT.get().is_some_and(|context| {
        context
            .write()
            .map(|mut guard| {
                f(&mut guard);
                true
            })
            .unwrap_or(false)
    })
}

#[derive(Debug, Default)]
pub struct ProviderContext {
    providers: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl ProviderContext {
    pub fn register_global_provider<T>(&mut self, provider: T)
    where
        T: Any + Send + Sync,
    {
        self.providers.insert(TypeId::of::<T>(), Box::new(provider));
    }

    pub fn get_global_provider<T>(&self) -> Option<&T>
    where
        T: Any + Send + Sync,
    {
        self.providers.get(&TypeId::of::<T>())?.downcast_ref()
    }
}

pub fn use_provider<T>() -> Option<T>
where
    T: Any + Send + Sync + Clone,
{
    with_provider_context(|context| context.get_global_provider::<T>().cloned()).flatten()
}

pub fn use_provider_or_default<T>() -> T
where
    T: Any + Send + Sync + Clone + Default,
{
    use_provider::<T>().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct TestProvider(&'static str);

    #[test]
    fn stores_providers_by_type() {
        let mut context = ProviderContext::default();
        context.register_global_provider(TestProvider("test"));

        assert_eq!(
            context.get_global_provider::<TestProvider>(),
            Some(&TestProvider("test"))
        );
    }
}
