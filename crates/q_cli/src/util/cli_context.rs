use std::sync::Arc;

use fig_os_shim::Context;
use fig_settings::Settings;

#[derive(Debug, Clone)]
pub struct CliContext {
    settings: Settings,
    context: Arc<Context>,
}

impl Default for CliContext {
    fn default() -> Self {
        Self::new()
    }
}

impl CliContext {
    pub fn new() -> Self {
        let settings = Settings::new();
        let context = Context::new();

        Self { settings, context }
    }

    pub fn new_fake() -> Self {
        let settings = Settings::new_fake();
        let context = Context::new_fake();

        Self { settings, context }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn context(&self) -> &Context {
        &self.context
    }
}
