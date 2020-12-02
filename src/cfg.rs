use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use std::fs::File;
use serde_yaml;
use serde_json;

#[derive(PartialEq, Eq, Debug, Serialize, Deserialize, Clone)]
pub struct OAuthConfig {
    pub app_id: String,
    pub secret: String,
    pub base_url: String
}

#[derive(PartialEq, Eq, Debug, Serialize, Deserialize, Clone)]
pub struct AuthConfig {
    pub gitlab: Option<OAuthConfig>
}

pub fn default_buffered() -> bool {
    true
}

pub fn default_meta() -> serde_json::Value {
    serde_json::Value::Null
}

#[derive(PartialEq, Eq, Debug, Serialize, Deserialize, Clone)]
pub struct TaskConfig {
    pub command: Vec<String>,
    #[serde(default="default_buffered")]
    pub buffered: bool,
    #[serde(default="default_meta")]
    pub meta: serde_json::Value,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

#[derive(PartialEq, Eq, Debug, Serialize, Deserialize, Clone)]
pub struct User {
    #[serde(default)]
    pub can_run: Vec<String>,
    #[serde(default)]
    pub can_view_output: Vec<String>,
    #[serde(default)]
    pub can_view_status: Vec<String>,
    #[serde(default)]
    pub can_change_data: Vec<String>,
}

#[derive(PartialEq, Eq, Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub tasks: HashMap<String, TaskConfig>,
    #[serde(default)]
    pub users: HashMap<String, User>,
    pub heartbeat: Option<u64>,
}

impl Config {
    pub fn read(path: &str) -> Self {
        let f = File::open(path).unwrap();
        serde_yaml::from_reader(f).unwrap()
    }
}
