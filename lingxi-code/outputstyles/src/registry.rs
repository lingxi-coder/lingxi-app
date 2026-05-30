//! Output-style registry with builtin markdown default and async switching.

use crate::model::{OutputFormat, OutputStyle, OutputStyleFrontmatter, OutputStyleSource};
use protocol::PluginId;
use std::collections::HashMap;
use thiserror::Error;
use tokio::sync::RwLock;

/// Owns all known output styles plus a (mutable) pointer to the active one.
pub struct OutputStyleRegistry {
    styles: HashMap<String, OutputStyle>,
    current_active: RwLock<String>,
    plugin_styles: HashMap<PluginId, Vec<String>>,
}

/// Errors raised by [`OutputStyleRegistry`].
#[derive(Debug, Clone, Error)]
pub enum OutputStyleError {
    /// The requested style is not registered.
    #[error("not found: {0}")]
    NotFound(String),
}

impl OutputStyleRegistry {
    /// Construct a new registry pre-populated with the builtin `markdown`
    /// style, which is set active.
    #[must_use]
    pub fn new() -> Self {
        let mut s = HashMap::new();
        s.insert(
            "markdown".into(),
            OutputStyle {
                name: "markdown".into(),
                description: "Default markdown output".into(),
                source: OutputStyleSource::Builtin,
                frontmatter: OutputStyleFrontmatter {
                    name: "markdown".into(),
                    description: String::new(),
                    default: true,
                    format: OutputFormat::Markdown,
                },
                system_prompt_addendum: String::new(),
                source_path: None,
            },
        );
        Self {
            styles: s,
            current_active: RwLock::new("markdown".into()),
            plugin_styles: HashMap::new(),
        }
    }

    /// Return a clone of the currently-active style.
    pub async fn active(&self) -> OutputStyle {
        let name = self.current_active.read().await.clone();
        self.styles
            .get(&name)
            .cloned()
            .expect("active style must exist")
    }

    /// Switch the active style by name.
    ///
    /// # Errors
    /// Returns [`OutputStyleError::NotFound`] if `name` is not registered.
    pub async fn switch(&self, name: &str) -> Result<(), OutputStyleError> {
        if !self.styles.contains_key(name) {
            return Err(OutputStyleError::NotFound(name.into()));
        }
        *self.current_active.write().await = name.into();
        Ok(())
    }

    /// Register a single style (or overwrite an existing one with the same name).
    pub fn register(&mut self, style: OutputStyle) {
        self.styles.insert(style.name.clone(), style);
    }

    /// Register a batch of styles owned by `plugin_id`.
    pub fn register_plugin_styles(&mut self, plugin_id: PluginId, styles: Vec<OutputStyle>) {
        let names: Vec<String> = styles.iter().map(|s| s.name.clone()).collect();
        for s in styles {
            self.styles.insert(s.name.clone(), s);
        }
        self.plugin_styles.insert(plugin_id, names);
    }

    /// Remove every style previously registered under `plugin_id`.
    pub fn unregister_plugin(&mut self, plugin_id: &PluginId) {
        if let Some(names) = self.plugin_styles.remove(plugin_id) {
            for n in &names {
                self.styles.remove(n);
            }
        }
    }
}

impl Default for OutputStyleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_is_markdown() {
        let r = OutputStyleRegistry::new();
        assert_eq!(r.active().await.name, "markdown");
    }

    #[tokio::test]
    async fn switch_to_unknown_errors() {
        let r = OutputStyleRegistry::new();
        assert!(r.switch("nope").await.is_err());
    }
}
