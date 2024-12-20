use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;

use crate::hook::Hook;
use crate::languages::node::installer::NodeInstaller;
use crate::languages::LanguageImpl;
use crate::store::{Store, ToolBucket};

#[derive(Debug, Copy, Clone)]
pub struct Node;

impl LanguageImpl for Node {
    fn environment_dir(&self) -> Option<&str> {
        Some("node_env")
    }

    async fn install(&self, hook: &Hook) -> Result<()> {
        let env = hook.environment_dir().expect("No environment dir found");
        fs_err::create_dir_all(env)?;

        let store = Store::from_settings()?;
        let node_dir = store.tools_path(ToolBucket::Node);

        let installer = NodeInstaller::new(node_dir);
        let (node, npm) = installer.install(&hook.language_version).await?;

        dbg!(node, npm);

        // TODO: Create an env

        Ok(())
    }

    async fn check_health(&self) -> Result<()> {
        todo!()
    }

    async fn run(
        &self,
        _hook: &Hook,
        _filenames: &[&String],
        _env_vars: Arc<HashMap<&'static str, String>>,
    ) -> Result<(i32, Vec<u8>)> {
        Ok((0, Vec::new()))
    }
}