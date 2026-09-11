use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MaxActiveSettings {
    #[serde(default)]
    pub global_max_active_enabled: bool,
    pub global_min_active: Option<u32>,
    pub global_max_active: Option<u32>,
    #[serde(default)]
    pub tracker_max_active: HashMap<String, TrackerMaxActiveSetting>,
    #[serde(default)]
    pub last_randomized_at: Option<u64>, // Unix timestamp in seconds (last_rotation_timestamp)
    #[serde(default)]
    pub current_effective_global_limit: Option<u32>,
    #[serde(default)]
    pub current_effective_tracker_limits: HashMap<String, u32>,
    #[serde(default)]
    pub last_scrape_timestamp: Option<u64>, // Backup timestamp saved during 30-min scrape cycle
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TrackerMaxActiveSetting {
    pub enabled: bool,
    pub min_active: u32,
    pub max_active: u32,
}
