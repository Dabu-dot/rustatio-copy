pub mod config;
pub mod faker;
pub mod grid;
pub mod logger;
pub mod max_active;
#[cfg(not(target_arch = "wasm32"))]
pub mod peer_listener;
pub mod protocol;
pub mod torrent;
pub mod validation;

// Re-export main types explicitly to avoid ambiguous Result types
pub use config::{AppConfig, ClientSettings, ConfigError, FakerSettings, UiSettings};
#[cfg(not(target_arch = "wasm32"))]
pub use faker::RatioFakerHandle;
pub use faker::{
    FakerConfig, FakerError, FakerState, FakerStats, PostStopAction, PresetSettings, RatioFaker,
};
pub use grid::{primary_tracker_host, GridImportSettings, GridMode, InstanceSummary};
pub use max_active::{MaxActiveSettings, TrackerMaxActiveSetting};
#[cfg(not(target_arch = "wasm32"))]
pub use peer_listener::{PeerCatalog, PeerListenerService, PeerListenerStatus, PeerLookup};
pub use torrent::{
    ClientConfig, ClientInfo, ClientType, HttpVersion, TorrentError, TorrentFile, TorrentInfo,
    TorrentSummary,
};
pub use validation::*;

pub fn now_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// Re-export reqwest::Client for downstream crates that need shared HTTP clients
#[cfg(any(feature = "native", feature = "wasm"))]
pub use reqwest;
