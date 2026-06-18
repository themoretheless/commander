//! Plugin system for extensibility (inspired by FAR/TC plugins, VSCode extensions).
//! SRP: central registry for columns, commands etc.
//! Integrated here for dynamic columns etc.

use crate::panel::{ColumnConfig, FileColumn, GitColumn, NameColumn};

pub struct PluginRegistry {
    columns: Vec<Box<dyn FileColumn>>,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        PluginRegistry { columns: vec![] }
    }
}

impl PluginRegistry {
    pub fn new() -> Self {
        let mut reg = Self::default();
        reg.register_default_columns();
        reg
    }

    fn register_default_columns(&mut self) {
        self.columns.push(Box::new(NameColumn));
        self.columns.push(Box::new(GitColumn));
        // TODO: plugins can register more
    }

    pub fn get_columns(&self, _config: &ColumnConfig) -> Vec<Box<dyn FileColumn>> {
        // recreate to avoid clone issue with dyn
        vec![Box::new(NameColumn), Box::new(GitColumn)]
    }
}

pub fn register_default_columns(config: &ColumnConfig) -> Vec<Box<dyn FileColumn>> {
    PluginRegistry::new().get_columns(config)
}

// TODO: plugin for commands, etc.
