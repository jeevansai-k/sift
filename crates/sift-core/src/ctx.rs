//! Everything an engine operation needs, bundled so it can be cloned into a
//! worker thread.

use crate::paths::Paths;
use crate::runner::{Runner, SystemRunner};
use crate::safety::Guard;
use crate::settings::Settings;
use crate::trash::Trash;
use std::sync::Arc;

#[derive(Clone)]
pub struct Ctx {
    pub paths: Paths,
    pub settings: Settings,
    pub runner: Arc<dyn Runner>,
    /// Who started the operation (`cli`, `gui`), recorded in Activity.
    pub source: String,
}

impl Ctx {
    pub fn detect(source: &str) -> Ctx {
        let paths = Paths::detect();
        let settings = Settings::load(&paths);
        Ctx { paths, settings, runner: Arc::new(SystemRunner), source: source.into() }
    }

    pub fn with(paths: Paths, settings: Settings, runner: Arc<dyn Runner>) -> Ctx {
        Ctx { paths, settings, runner, source: "test".into() }
    }

    pub fn guard(&self) -> Guard {
        Guard::new(&self.paths, &self.settings.whitelist_paths(&self.paths))
    }

    pub fn trash(&self) -> Trash {
        Trash::new(&self.paths)
    }

    pub fn runner(&self) -> &dyn Runner {
        self.runner.as_ref()
    }
}
