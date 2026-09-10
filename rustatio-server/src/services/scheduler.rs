use super::instance::FakerInstance;
use super::lifecycle::InstanceLifecycle;
use super::persistence::now_timestamp;
use super::state::AppState;
use rand::Rng;
use rustatio_core::logger::set_instance_context_str;
use rustatio_core::{primary_tracker_host, FakerState, RatioFakerHandle};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

pub struct Scheduler {
    shutdown_tx: Option<mpsc::Sender<()>>,
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

impl Scheduler {
    pub const fn new() -> Self {
        Self { shutdown_tx: None, task_handle: None }
    }

    pub fn start(
        &mut self,
        state: AppState,
        instances: Arc<RwLock<HashMap<String, FakerInstance>>>,
    ) {
        if self.task_handle.is_some() {
            return;
        }

        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        let handle = tokio::spawn(scheduler_loop(state, instances, shutdown_rx));

        self.shutdown_tx = Some(shutdown_tx);
        self.task_handle = Some(handle);

        tracing::info!("Centralized scheduler started");
    }

    pub async fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }
        if let Some(handle) = self.task_handle.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
        }
        tracing::info!("Centralized scheduler stopped");
    }
}

async fn scheduler_loop(
    state: AppState,
    instances: Arc<RwLock<HashMap<String, FakerInstance>>>,
    mut shutdown_rx: mpsc::Receiver<()>,
) {
    let update_interval = Duration::from_secs(5);
    let save_interval = Duration::from_secs(30);
    let mut last_save = std::time::Instant::now();

    tracing::info!("Scheduler loop started");

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                tracing::info!("Scheduler received shutdown signal");
                break;
            }
            () = tokio::time::sleep(update_interval) => {
                let mut dirty = update_instances(&state, &instances).await;

                if manage_max_active_and_queue(&state, &instances).await {
                    dirty = true;
                }

                if dirty {
                    if let Err(e) = state.save_state().await {
                        tracing::warn!("Scheduler: failed to save state after runtime change: {}", e);
                    }
                }

                if last_save.elapsed() >= save_interval {
                    if let Err(e) = state.save_state().await {
                        tracing::warn!("Scheduler: failed to save state: {}", e);
                    }
                    last_save = std::time::Instant::now();
                }
            }
        }
    }

    tracing::info!("Scheduler loop stopped");
}

async fn update_instances(
    state: &AppState,
    instances: &Arc<RwLock<HashMap<String, FakerInstance>>>,
) -> bool {
    let items: Vec<(String, Arc<RatioFakerHandle>)> = {
        let guard = instances.read().await;
        guard.iter().map(|(id, inst)| (id.clone(), Arc::clone(&inst.faker))).collect()
    };

    let mut dirty = false;

    for (id, faker) in items {
        let before = faker.stats_snapshot();
        let should_update = matches!(before.state, FakerState::Running);
        let should_retry = matches!(before.state, FakerState::Stopped)
            && before.tracker_error.as_deref() == Some("Tracker unavailable")
            && faker.tracker_retry_due_now().await;

        if !should_update && !should_retry {
            continue;
        }

        let label = {
            let guard = instances.read().await;
            guard
                .get(&id)
                .map(|instance| instance.summary.name.clone())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| id.clone())
        };
        set_instance_context_str(Some(&label));
        let result = if should_retry {
            state.recover_tracker_instance(&id).await.map(|_| ())
        } else {
            faker.update().await.map_err(|e| e.to_string())
        };
        if let Err(e) = result {
            let action = if should_retry { "tracker recovery" } else { "update" };
            tracing::warn!("Scheduler: {} failed for instance {}: {}", action, id, e);
            continue;
        }

        let after = faker.stats_snapshot();
        {
            let mut guard = instances.write().await;
            if let Some(instance) = guard.get_mut(&id) {
                instance.cumulative_uploaded = after.uploaded;
                instance.cumulative_downloaded = after.downloaded;
                instance.config.completion_percent = after.torrent_completion;
            }
        }

        if std::mem::discriminant(&after.state) != std::mem::discriminant(&before.state)
            || after.stop_condition_met != before.stop_condition_met
            || after.is_idling != before.is_idling
            || after.tracker_error != before.tracker_error
            || after.tracker_retry_attempt != before.tracker_retry_attempt
            || after.tracker_retry_at_ms != before.tracker_retry_at_ms
        {
            dirty = true;
        }
    }

    dirty
}

async fn manage_max_active_and_queue(
    state: &AppState,
    instances: &Arc<RwLock<HashMap<String, FakerInstance>>>,
) -> bool {
    let Some(mut settings) = state.get_max_active_settings().await else {
        return false;
    };

    let now_secs = now_timestamp();
    let mut settings_modified = false;

    // Check if daily re-randomization is needed (every 86400 seconds)
    let needs_randomization = settings.last_randomized_at.map_or(true, |last| {
        now_secs.saturating_sub(last) >= 86400 || settings.current_effective_global_limit.is_none()
    });

    if needs_randomization {
        let mut rng = rand::rng();

        if settings.global_max_active_enabled {
            if let (Some(min_val), Some(max_val)) = (settings.global_min_active, settings.global_max_active) {
                let limit = if min_val < max_val {
                    rng.random_range(min_val..=max_val)
                } else {
                    min_val
                };
                settings.current_effective_global_limit = Some(limit);
            }
        } else {
            settings.current_effective_global_limit = None;
        }

        settings.current_effective_tracker_limits.clear();
        for (host, tr_setting) in &settings.tracker_max_active {
            if tr_setting.enabled {
                let limit = if tr_setting.min_active < tr_setting.max_active {
                    rng.random_range(tr_setting.min_active..=tr_setting.max_active)
                } else {
                    tr_setting.min_active
                };
                settings.current_effective_tracker_limits.insert(host.clone(), limit);
            }
        }

        settings.last_randomized_at = Some(now_secs);
        settings_modified = true;
    }

    if settings_modified {
        let _ = state.set_max_active_settings(settings.clone()).await;
    }

    // Inspect running vs queued instances
    struct InstanceStateInfo {
        id: String,
        state: FakerState,
        tracker_host: String,
        is_paused: bool,
    }

    let instance_states: Vec<InstanceStateInfo> = {
        let guard = instances.read().await;
        guard
            .iter()
            .map(|(id, inst)| {
                let stats = inst.faker.stats_snapshot();
                let tracker_host = primary_tracker_host(&inst.summary.announce).unwrap_or_default();
                let is_paused = matches!(stats.state, FakerState::Paused);
                InstanceStateInfo {
                    id: id.clone(),
                    state: stats.state,
                    tracker_host,
                    is_paused,
                }
            })
            .collect()
    };

    let mut running_by_tracker: HashMap<String, u32> = HashMap::new();
    let mut total_running = 0u32;
    let mut candidates_to_start = Vec::new();

    for item in &instance_states {
        if matches!(item.state, FakerState::Running | FakerState::Starting) {
            total_running += 1;
            *running_by_tracker.entry(item.tracker_host.clone()).or_default() += 1;
        } else if matches!(item.state, FakerState::Stopped | FakerState::Idle) && !item.is_paused {
            // Eligible to be started if room allows
            candidates_to_start.push(item);
        }
    }

    if candidates_to_start.is_empty() {
        return settings_modified;
    }

    // Check global limit constraint
    if let Some(global_limit) = settings.current_effective_global_limit {
        if total_running >= global_limit {
            return settings_modified;
        }
    }

    // Filter candidates that fit under their tracker's limit
    let eligible_candidates: Vec<&InstanceStateInfo> = candidates_to_start
        .into_iter()
        .filter(|item| {
            if let Some(&tr_limit) = settings.current_effective_tracker_limits.get(&item.tracker_host) {
                let current_tr_running = running_by_tracker.get(&item.tracker_host).copied().unwrap_or(0);
                if current_tr_running >= tr_limit {
                    return false;
                }
            }
            true
        })
        .collect();

    if eligible_candidates.is_empty() {
        return settings_modified;
    }

    // Randomly select one eligible candidate to start
    let selected_id = {
        let mut rng = rand::rng();
        let idx = rng.random_range(0..eligible_candidates.len());
        eligible_candidates[idx].id.clone()
    };

    tracing::info!(
        "Queue scheduler: Starting next queued instance {}",
        selected_id
    );

    if let Err(e) = state.start_instance(&selected_id).await {
        tracing::warn!("Queue scheduler: Failed to start instance {}: {}", selected_id, e);
    } else {
        settings_modified = true;
    }

    settings_modified
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::persistence::MaxActiveSettings;
    use rustatio_core::{FakerConfig, TorrentInfo};

    fn sample_torrent(hash_byte: u8) -> TorrentInfo {
        TorrentInfo {
            info_hash: [hash_byte; 20],
            announce: "https://tracker.test/announce".to_string(),
            announce_list: None,
            name: format!("sample-{hash_byte}"),
            total_size: 1024,
            piece_length: 256,
            num_pieces: 4,
            creation_date: None,
            comment: None,
            created_by: None,
            is_single_file: true,
            file_count: 1,
            files: Vec::new(),
        }
    }

    #[tokio::test]
    async fn test_queue_scheduler_starts_queued_instance_under_limit() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::new(&temp.path().to_string_lossy());

        // Create 2 instances
        state
            .create_instance("inst-1", sample_torrent(1), FakerConfig::default())
            .await
            .unwrap();
        state
            .create_instance("inst-2", sample_torrent(2), FakerConfig::default())
            .await
            .unwrap();

        // Enable global max active = 1
        let mut settings = MaxActiveSettings::default();
        settings.global_max_active_enabled = true;
        settings.global_min_active = Some(1);
        settings.global_max_active = Some(1);
        state.set_max_active_settings(settings).await.unwrap();

        let instances_map = state.instances.clone();

        // Initially both inst-1 and inst-2 are Stopped.
        // Queue scheduler should attempt to start one instance up to global limit = 1.
        let changed = manage_max_active_and_queue(&state, &instances_map).await;
        assert!(changed);

        let effective_limit = state
            .get_max_active_settings()
            .await
            .and_then(|s| s.current_effective_global_limit);
        assert_eq!(effective_limit, Some(1));
    }
}
