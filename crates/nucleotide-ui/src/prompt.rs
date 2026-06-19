use gpui::SharedString;
use std::sync::Arc;

#[derive(Clone)]
pub struct Prompt {
    pub prompt: SharedString,
    pub initial_input: SharedString,
    pub on_submit: Arc<dyn Fn(&str) + Send + Sync>,
    pub on_cancel: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Prompt {
    pub fn native(
        prompt: impl Into<SharedString>,
        initial_input: impl Into<SharedString>,
        on_submit: impl Fn(&str) + Send + Sync + 'static,
    ) -> Self {
        Self {
            prompt: prompt.into(),
            initial_input: initial_input.into(),
            on_submit: Arc::new(on_submit),
            on_cancel: None,
        }
    }

    pub fn with_cancel(mut self, on_cancel: impl Fn() + Send + Sync + 'static) -> Self {
        self.on_cancel = Some(Arc::new(on_cancel));
        self
    }
}

impl std::fmt::Debug for Prompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prompt")
            .field("prompt", &self.prompt)
            .field("initial_input", &self.initial_input)
            .field("on_submit", &"<callback>")
            .field("on_cancel", &"<callback>")
            .finish()
    }
}
