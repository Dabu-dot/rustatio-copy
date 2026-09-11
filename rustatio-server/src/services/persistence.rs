use rustatio_core::{FakerConfig, FakerState, TorrentSummary};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstanceSource {
    #[default]
    Manual,
    WatchFolder,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedInstance {
    pub id: String,
    pub torrent: TorrentSummary,
    pub config: FakerConfig,
    pub cumulative_uploaded: u64,
    pub cumulative_downloaded: u64,
    pub state: FakerState,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub source: InstanceSource,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub runtime: Option<PersistedRuntime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedRuntime {
    pub uploaded: u64,
    pub downloaded: u64,
    pub ratio: f64,
    pub left: u64,
    pub torrent_completion: f64,
    pub seeders: i64,
    pub leechers: i64,
    pub session_uploaded: u64,
    pub session_downloaded: u64,
    pub session_ratio: f64,
    pub elapsed_secs: u64,
    pub current_upload_rate: f64,
    pub current_download_rate: f64,
    pub average_upload_rate: f64,
    pub average_download_rate: f64,
    pub upload_progress: f64,
    pub download_progress: f64,
    pub ratio_progress: f64,
    pub seed_time_progress: f64,
    pub effective_stop_at_ratio: Option<f64>,
    pub eta_ratio_secs: Option<u64>,
    pub eta_uploaded_secs: Option<u64>,
    pub eta_seed_time_secs: Option<u64>,
    pub eta_download_completion_secs: Option<u64>,
    pub stop_condition_met: bool,
    pub is_idling: bool,
    pub idling_reason: Option<String>,
    #[serde(default)]
    pub tracker_error: Option<String>,
    pub announce_count: u32,
    #[serde(default)]
    pub is_cyclic_inactive: bool,
    #[serde(default)]
    pub cyclic_next_switch_ms: Option<u64>,
    #[serde(default)]
    pub manually_stopped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CustomPreset {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    #[serde(default)]
    pub custom: bool,
    #[serde(alias = "createdAt")]
    pub created_at: String,
    #[schema(value_type = Object)]
    pub settings: rustatio_core::PresetSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DefaultPreset {
    pub id: String,
    pub name: String,
    #[schema(value_type = Object)]
    pub settings: rustatio_core::PresetSettings,
}

const fn default_watch_max_depth() -> u32 {
    1
}

fn default_watch_auto_start() -> bool {
    std::env::var("WATCH_AUTO_START").is_ok_and(|v| v.eq_ignore_ascii_case("true") || v == "1")
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
pub struct WatchSettings {
    #[serde(default = "default_watch_max_depth")]
    pub max_depth: u32,
    #[serde(default = "default_watch_auto_start")]
    pub auto_start: bool,
}

impl Default for WatchSettings {
    fn default() -> Self {
        Self { max_depth: default_watch_max_depth(), auto_start: default_watch_auto_start() }
    }
}

pub use rustatio_core::MaxActiveSettings;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersistedState {
    pub instances: HashMap<String, PersistedInstance>,
    #[serde(default)]
    pub default_config: Option<FakerConfig>,
    #[serde(default)]
    pub default_preset: Option<DefaultPreset>,
    #[serde(default)]
    pub watch_settings: Option<WatchSettings>,
    #[serde(default)]
    pub custom_presets: Vec<CustomPreset>,
    #[serde(default)]
    pub max_active_settings: Option<MaxActiveSettings>,
    pub version: u32,
}

impl PersistedState {
    pub fn new() -> Self {
        Self {
            instances: HashMap::new(),
            default_config: None,
            default_preset: None,
            watch_settings: None,
            custom_presets: Vec::new(),
            max_active_settings: None,
            version: 1,
        }
    }
}

pub struct Persistence {
    state_file: String,
}

impl Persistence {
    pub fn new(data_dir: &str) -> Self {
        Self { state_file: format!("{data_dir}/state.json") }
    }

    pub async fn load(&self) -> PersistedState {
        let path = Path::new(&self.state_file);

        if !path.exists() {
            tracing::info!("No saved state found at {}, starting fresh", self.state_file);
            return PersistedState::new();
        }

        match fs::File::open(path).await {
            Ok(mut file) => {
                let mut contents = String::new();
                if let Err(e) = file.read_to_string(&mut contents).await {
                    tracing::error!("Failed to read state file: {}", e);
                    return PersistedState::new();
                }

                match serde_json::from_str(&contents) {
                    Ok(state) => {
                        tracing::info!("Loaded saved state from {}", self.state_file);
                        state
                    }
                    Err(e) => {
                        tracing::error!("Failed to parse state file: {}", e);
                        let backup = format!("{}.corrupted", self.state_file);
                        let _ = fs::rename(path, &backup).await;
                        tracing::warn!("Backed up corrupted state to {}", backup);
                        PersistedState::new()
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to open state file: {}", e);
                PersistedState::new()
            }
        }
    }

    pub async fn save(&self, state: &PersistedState) -> Result<(), String> {
        if let Some(parent) = Path::new(&self.state_file).parent() {
            if let Err(e) = fs::create_dir_all(parent).await {
                return Err(format!("Failed to create data directory: {e}"));
            }
        }

        let temp_file = format!("{}.tmp", self.state_file);

        let mut file = fs::File::create(&temp_file)
            .await
            .map_err(|e| format!("Failed to create temp file: {e}"))?;

        let json =
            serde_json::to_vec(state).map_err(|e| format!("Failed to serialize state: {e}"))?;

        file.write_all(&json).await.map_err(|e| format!("Failed to write state: {e}"))?;

        file.sync_all().await.map_err(|e| format!("Failed to sync state file: {e}"))?;

        fs::rename(&temp_file, &self.state_file)
            .await
            .map_err(|e| format!("Failed to rename state file: {e}"))?;

        tracing::debug!("State saved to {}", self.state_file);
        Ok(())
    }
}

pub fn now_timestamp() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustatio_core::faker::InactiveMode;
    use rustatio_core::{FakerConfig, FakerState, PostStopAction, TorrentSummary};
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn watch_settings_default_uses_env_auto_start() {
        let guard = env_lock().lock();
        assert!(guard.is_ok(), "failed to acquire env mutex");

        std::env::set_var("WATCH_AUTO_START", "1");
        let settings = WatchSettings::default();
        assert!(settings.auto_start);

        std::env::set_var("WATCH_AUTO_START", "false");
        let settings = WatchSettings::default();
        assert!(!settings.auto_start);

        std::env::remove_var("WATCH_AUTO_START");
    }

    #[test]
    fn test_persisted_state_full_serialization_roundtrip() {
        let config = FakerConfig {
            upload_rate: 150.0,
            download_rate: 300.0,
            port: 51413,
            vpn_port_sync: true,
            client_type: rustatio_core::ClientType::QBittorrent,
            client_version: Some("4.6.0".to_string()),
            initial_uploaded: 102400,
            initial_downloaded: 51200,
            completion_percent: 75.0,
            num_want: 100,
            randomize_rates: true,
            random_range_percent: 15.0,
            randomize_ratio: true,
            random_ratio_range_percent: 5.0,
            stop_at_ratio: Some(2.5),
            effective_stop_at_ratio: Some(2.48),
            stop_at_uploaded: Some(1073741824),
            stop_at_downloaded: Some(536870912),
            stop_at_seed_time: Some(86400),
            idle_when_no_leechers: true,
            idle_when_no_seeders: true,
            scrape_interval: 120,
            progressive_rates: true,
            target_upload_rate: Some(500.0),
            target_download_rate: Some(1000.0),
            progressive_duration: 7200,
            post_stop_action: PostStopAction::StopSeeding,
            start_when_leechers_above: Some(5),
            start_when_seeders_above: Some(10),
            cyclic_enabled: true,
            min_active_duration: 18000,
            max_active_duration: 28800,
            min_inactive_duration: 3600,
            max_inactive_duration: 7200,
            reset_session_counters_on_cycle: false,
            inactive_mode: InactiveMode::Stopped,
        };

        let runtime = PersistedRuntime {
            uploaded: 204800,
            downloaded: 102400,
            ratio: 2.0,
            left: 25600,
            torrent_completion: 75.0,
            seeders: 42,
            leechers: 12,
            session_uploaded: 102400,
            session_downloaded: 51200,
            session_ratio: 1.0,
            elapsed_secs: 3600,
            current_upload_rate: 150.0,
            current_download_rate: 300.0,
            average_upload_rate: 140.0,
            average_download_rate: 280.0,
            upload_progress: 50.0,
            download_progress: 25.0,
            ratio_progress: 80.0,
            seed_time_progress: 100.0,
            effective_stop_at_ratio: Some(2.48),
            eta_ratio_secs: Some(1800),
            eta_uploaded_secs: Some(3600),
            eta_seed_time_secs: Some(0),
            eta_download_completion_secs: Some(600),
            stop_condition_met: false,
            is_idling: false,
            idling_reason: None,
            tracker_error: None,
            announce_count: 5,
            is_cyclic_inactive: false,
            cyclic_next_switch_ms: Some(1700000000000),
            manually_stopped: false,
        };

        let instance = PersistedInstance {
            id: "inst-1".to_string(),
            torrent: TorrentSummary {
                info_hash: [1u8; 20],
                name: "test-torrent.iso".to_string(),
                total_size: 1073741824,
                file_count: 1,
                announce: "https://tracker.example.com/announce".to_string(),
                announce_list: None,
                piece_length: 256,
                num_pieces: 4,
                creation_date: None,
                comment: None,
                created_by: None,
                is_single_file: true,
            },
            config: config.clone(),
            cumulative_uploaded: 204800,
            cumulative_downloaded: 102400,
            state: FakerState::Running,
            created_at: 1000,
            updated_at: 2000,
            source: InstanceSource::WatchFolder,
            tags: vec!["ubuntu".to_string(), "iso".to_string()],
            runtime: Some(runtime.clone()),
        };

        let mut instances = HashMap::new();
        instances.insert("inst-1".to_string(), instance);

        let max_active = MaxActiveSettings {
            global_max_active_enabled: true,
            global_min_active: Some(2),
            global_max_active: Some(5),
            current_effective_global_limit: Some(3),
            tracker_max_active: HashMap::new(),
            current_effective_tracker_limits: HashMap::new(),
            last_randomized_at: Some(1700000000),
            last_scrape_timestamp: Some(1700000000),
        };

        let state = PersistedState {
            instances,
            default_config: Some(config),
            default_preset: None,
            watch_settings: Some(WatchSettings { max_depth: 2, auto_start: true }),
            custom_presets: Vec::new(),
            max_active_settings: Some(max_active),
            version: 1,
        };

        let json_str = serde_json::to_string(&state).expect("serialization failed");
        let deserialized: PersistedState =
            serde_json::from_str(&json_str).expect("deserialization failed");

        assert_eq!(deserialized.version, 1);
        let restored_inst = deserialized.instances.get("inst-1").expect("instance missing");
        assert_eq!(restored_inst.id, "inst-1");
        assert_eq!(restored_inst.config.upload_rate, 150.0);
        assert_eq!(restored_inst.config.inactive_mode, InactiveMode::Stopped);
        assert_eq!(restored_inst.tags, vec!["ubuntu", "iso"]);

        let restored_rt = restored_inst.runtime.as_ref().expect("runtime missing");
        assert_eq!(restored_rt.uploaded, 204800);
        assert_eq!(restored_rt.seeders, 42);
        assert_eq!(restored_rt.cyclic_next_switch_ms, Some(1700000000000));

        let restored_ma = deserialized.max_active_settings.as_ref().expect("max_active missing");
        assert!(restored_ma.global_max_active_enabled);
        assert_eq!(restored_ma.current_effective_global_limit, Some(3));
    }
}
