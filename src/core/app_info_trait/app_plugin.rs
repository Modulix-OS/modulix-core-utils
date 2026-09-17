use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppPlugin {
    pub name: String,
    pub description: String,
}
