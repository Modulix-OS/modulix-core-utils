use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct AppPlugin {
    pub name: String,
    pub description: String,
}
