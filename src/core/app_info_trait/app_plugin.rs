//! One plugin a module exposes, as the module metadata declares it.

use serde::{Deserialize, Serialize};

/// A plugin offered by a module (a VS Code extension, an OBS plugin, …).
///
/// # Fields
/// * `name` - the plugin's key within its namespace, as passed to the daemon's
///   plugin install/uninstall calls.
/// * `description` - one-line description for the UI; empty when the module
///   metadata carries none.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppPlugin {
    pub name: String,
    pub description: String,
}
