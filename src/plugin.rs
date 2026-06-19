//! Plugin system for extensibility (inspired by FAR/TC plugins, VSCode extensions).
//! SRP: central registry for columns, commands etc.
//! Integrated here for dynamic columns etc.

use crate::panel::{ColumnConfig, FileColumn, GitColumn, NameColumn};
use crate::workspace::CommandHandler;

pub struct PluginRegistry {
    columns: Vec<Box<dyn FileColumn>>,
    // Command handlers for plugins (trait impls can be registered).
    #[allow(dead_code)]
    command_handlers: Vec<Box<dyn CommandHandler>>,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        PluginRegistry { columns: vec![], command_handlers: vec![] }
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
        // TODO: plugins can register more columns and CommandHandler impls
    }

    pub fn get_columns(&self, _config: &ColumnConfig) -> Vec<Box<dyn FileColumn>> {
        // recreate to avoid clone issue with dyn
        vec![Box::new(NameColumn), Box::new(GitColumn)]
    }

    // Plugins can register CommandHandler here in future.
    #[allow(dead_code)]
    pub fn register_command_handler(&mut self, h: Box<dyn CommandHandler>) {
        self.command_handlers.push(h);
    }
}

pub fn register_default_columns(config: &ColumnConfig) -> Vec<Box<dyn FileColumn>> {
    PluginRegistry::new().get_columns(config)
}

// CommandHandler support ready for plugins (trait in command_handlers).

