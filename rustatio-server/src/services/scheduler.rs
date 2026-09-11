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

    let mut last_30min_scan = std::time::Instant::now();

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                tracing::info!("Scheduler received shutdown signal");
                break;
            }
            () = tokio::time::sleep(update_interval) => {
                let mut dirty = update_instances(&state, &instances).await;

                let is_30min_scan = last_30min_scan.elapsed() >= Duration::from_secs(1800);
                if is_30min_scan {
                    last_30min_scan = std::time::Instant::now();
                    dirty = true;
                }

                if manage_max_active_and_queue(&state, &instances, is_30min_scan).await {
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

    // Evaluate scrape conditions on active / cyclic instances
    let active_items: Vec<(String, Arc<RatioFakerHandle>)> = {
        let guard = instances.read().await;
        guard
            .iter()
            .filter(|(_, inst)| {
                let stats = inst.faker.stats_snapshot();
                matches!(stats.state, FakerState::Running | FakerState::Starting)
                    || stats.is_cyclic_inactive
            })
            .map(|(id, inst)| (id.clone(), Arc::clone(&inst.faker)))
            .collect()
    };

    for (id, faker) in active_items {
        if !faker.check_scrape_start_conditions().await {
            tracing::info!(
                "Instance {} lost scrape conditions during update, transitioning to Idle",
                id
            );
            let _ = faker.stop().await; // Sends Stopped announce to tracker
            let mut new_stats = faker.stats_snapshot();
            new_stats.state = FakerState::Idle;
            new_stats.is_idling = true;
            new_stats.idling_reason = Some("lost_scrape_conditions".to_string());
            faker.restore_snapshot(new_stats).await;
            dirty = true;
        }
    }

    dirty
}

fn roll_limits(settings: &mut rustatio_core::MaxActiveSettings, now_secs: u64) {
    let mut rng = rand::rng();

    if settings.global_max_active_enabled {
        if let (Some(min_val), Some(max_val)) =
            (settings.global_min_active, settings.global_max_active)
        {
            let limit =
                if min_val < max_val { rng.random_range(min_val..=max_val) } else { min_val };
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
}

pub async fn roll_and_apply_max_active_limits(
    state: &AppState,
) -> Result<rustatio_core::MaxActiveSettings, String> {
    let mut settings = state.get_max_active_settings().await.unwrap_or_default();
    let now_secs = now_timestamp();
    roll_limits(&mut settings, now_secs);

    state.set_max_active_settings(settings.clone()).await?;

    let instances_map = state.instances.clone();
    manage_max_active_and_queue(state, &instances_map, false).await;

    Ok(state.get_max_active_settings().await.unwrap_or(settings))
}

async fn manage_max_active_and_queue(
    state: &AppState,
    instances: &Arc<RwLock<HashMap<String, FakerInstance>>>,
    is_30min_scan: bool,
) -> bool {
    let Some(mut settings) = state.get_max_active_settings().await else {
        return false;
    };

    let now_secs = now_timestamp();
    let mut settings_modified = false;

    if is_30min_scan {
        settings.last_scrape_timestamp = Some(now_secs);
        settings_modified = true;

        // Clear manually_stopped flag on all instances on 30-min scheduled scan
        let items: Vec<(String, Arc<RatioFakerHandle>)> = {
            let guard = instances.read().await;
            guard.iter().map(|(id, inst)| (id.clone(), Arc::clone(&inst.faker))).collect()
        };
        for (_id, faker) in items {
            let mut stats = faker.stats_snapshot();
            if stats.manually_stopped {
                stats.manually_stopped = false;
                faker.restore_snapshot(stats).await;
            }
        }
    }

    // 24h Timer Downtime Handling & Roll Trigger:
    // Primary timestamp: last_randomized_at. Fallback: last_scrape_timestamp.
    let effective_last_timestamp = settings.last_randomized_at.or(settings.last_scrape_timestamp);
    let needs_randomization = effective_last_timestamp.map_or(true, |last| {
        now_secs.saturating_sub(last) >= 86400
            || (settings.global_max_active_enabled
                && settings.current_effective_global_limit.is_none())
    });

    if needs_randomization {
        roll_limits(&mut settings, now_secs);
        settings_modified = true;
    }

    if settings_modified {
        let _ = state.set_max_active_settings(settings.clone()).await;
    }

    // Inspect instance info
    struct InstanceStateInfo {
        id: String,
        state: FakerState,
        tracker_host: String,
        is_paused: bool,
        is_cyclic_inactive: bool,
        manually_stopped: bool,
        elapsed_secs: u64,
        faker: Arc<RatioFakerHandle>,
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
                    is_cyclic_inactive: stats.is_cyclic_inactive,
                    manually_stopped: stats.manually_stopped,
                    elapsed_secs: stats.elapsed_time.as_secs(),
                    faker: Arc::clone(&inst.faker),
                }
            })
            .collect()
    };

    // Rule #1: Trackers do NOT share a combined pool.
    // Group running instances by tracker_host.
    // "Active" instances count includes Running, Starting, and Cyclic Inactive (unless lost scrape conditions).
    let mut tracker_running_map: HashMap<String, Vec<&InstanceStateInfo>> = HashMap::new();
    for item in &instance_states {
        if matches!(item.state, FakerState::Running | FakerState::Starting)
            || item.is_cyclic_inactive
        {
            tracker_running_map.entry(item.tracker_host.clone()).or_default().push(item);
        }
    }

    // Collect all tracker hosts present in instances or settings
    let mut all_trackers: Vec<String> = tracker_running_map.keys().cloned().collect();
    for host in settings.tracker_max_active.keys() {
        if !all_trackers.contains(host) {
            all_trackers.push(host.clone());
        }
    }

    let default_limit = if settings.global_max_active_enabled {
        settings.current_effective_global_limit
    } else {
        None
    };

    // 1. Reconciliation: Scale Down (Stop excess running instances per-tracker)
    for host in &all_trackers {
        let tracker_limit =
            settings.current_effective_tracker_limits.get(host).copied().or(default_limit);

        let Some(limit) = tracker_limit else {
            continue;
        };

        if let Some(running_list) = tracker_running_map.get_mut(host) {
            if running_list.len() > limit as usize {
                // Sort by elapsed_secs ascending (lowest elapsed = most recently started = stopped first)
                running_list.sort_by_key(|item| item.elapsed_secs);
                let excess = running_list.len() - limit as usize;
                for item in running_list.iter().take(excess) {
                    tracing::info!(
                        "Queue scheduler: Tracker limit ({}) exceeded for {}. Stopping excess instance {}",
                        limit,
                        host,
                        item.id
                    );
                    let _ = state.stop_instance(&item.id).await;
                    settings_modified = true;
                }
            }
        }
    }

    // Re-evaluate instance states after scale down
    let updated_states: Vec<InstanceStateInfo> = {
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
                    is_cyclic_inactive: stats.is_cyclic_inactive,
                    manually_stopped: stats.manually_stopped,
                    elapsed_secs: stats.elapsed_time.as_secs(),
                    faker: Arc::clone(&inst.faker),
                }
            })
            .collect()
    };

    // 2. Reconciliation: Scale Up (Fill available slots per tracker)
    let mut running_count_by_tracker: HashMap<String, usize> = HashMap::new();
    let mut candidates_by_tracker: HashMap<String, Vec<&InstanceStateInfo>> = HashMap::new();

    for item in &updated_states {
        if matches!(item.state, FakerState::Running | FakerState::Starting)
            || item.is_cyclic_inactive
        {
            *running_count_by_tracker.entry(item.tracker_host.clone()).or_default() += 1;
        } else if matches!(item.state, FakerState::Stopped | FakerState::Idle)
            && !item.is_paused
            && !item.manually_stopped
        {
            candidates_by_tracker.entry(item.tracker_host.clone()).or_default().push(item);
        }
    }

    for (host, candidates) in candidates_by_tracker.iter_mut() {
        let tracker_limit =
            settings.current_effective_tracker_limits.get(host).copied().or(default_limit);

        let Some(limit) = tracker_limit else {
            continue;
        };

        let mut current_running = running_count_by_tracker.get(host).copied().unwrap_or(0);

        while current_running < limit as usize && !candidates.is_empty() {
            // Randomly pick candidate
            let idx = {
                let mut rng = rand::rng();
                rng.random_range(0..candidates.len())
            };
            let candidate = candidates.remove(idx);

            // Scrape Conditions Check (Eligibility Filter)
            if candidate.faker.check_scrape_start_conditions().await {
                tracing::info!(
                    "Queue scheduler: Starting eligible candidate {} for tracker {}",
                    candidate.id,
                    host
                );
                if state.start_instance(&candidate.id).await.is_ok() {
                    current_running += 1;
                    running_count_by_tracker.insert(host.clone(), current_running);
                    settings_modified = true;
                }
            } else {
                // If candidate fails scrape conditions, mark/leave as Idle and test another candidate
                let mut new_stats = candidate.faker.stats_snapshot();
                new_stats.state = FakerState::Idle;
                new_stats.is_idling = true;
                new_stats.idling_reason = Some("lost_scrape_conditions".to_string());
                candidate.faker.restore_snapshot(new_stats).await;
                tracing::info!("Candidate {} failed scrape conditions, keeping Idle", candidate.id);
            }
        }
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
        state.create_instance("inst-1", sample_torrent(1), FakerConfig::default()).await.unwrap();
        state.create_instance("inst-2", sample_torrent(2), FakerConfig::default()).await.unwrap();

        // Enable global max active = 1
        let mut settings = MaxActiveSettings::default();
        settings.global_max_active_enabled = true;
        settings.global_min_active = Some(1);
        settings.global_max_active = Some(1);
        state.set_max_active_settings(settings).await.unwrap();

        let instances_map = state.instances.clone();

        // Initially both inst-1 and inst-2 are Stopped.
        // Queue scheduler should attempt to start one instance up to global limit = 1.
        let changed = manage_max_active_and_queue(&state, &instances_map, false).await;
        assert!(changed);

        let effective_limit =
            state.get_max_active_settings().await.and_then(|s| s.current_effective_global_limit);
        assert_eq!(effective_limit, Some(1));
    }

    #[tokio::test]
    async fn test_reconciliation_stops_excess_running_instances_most_recent_first() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::new(&temp.path().to_string_lossy());

        state.create_instance("inst-1", sample_torrent(1), FakerConfig::default()).await.unwrap();
        state.create_instance("inst-2", sample_torrent(2), FakerConfig::default()).await.unwrap();
        state.create_instance("inst-3", sample_torrent(3), FakerConfig::default()).await.unwrap();

        // Set all 3 to Running with different elapsed times:
        // inst-1 = 100s (longest), inst-2 = 50s, inst-3 = 10s (most recent)
        {
            let instances = state.instances.read().await;
            for (id, elapsed) in [("inst-1", 100), ("inst-2", 50), ("inst-3", 10)] {
                let inst = instances.get(id).unwrap();
                let mut stats = inst.faker.stats_snapshot();
                stats.state = FakerState::Running;
                stats.elapsed_time = Duration::from_secs(elapsed);
                inst.faker.restore_snapshot(stats).await;
            }
        }

        // Set limit to 1 active
        let mut settings = MaxActiveSettings::default();
        settings.global_max_active_enabled = true;
        settings.global_min_active = Some(1);
        settings.global_max_active = Some(1);
        state.set_max_active_settings(settings).await.unwrap();

        let instances_map = state.instances.clone();
        manage_max_active_and_queue(&state, &instances_map, false).await;
        manage_max_active_and_queue(&state, &instances_map, false).await;

        // Verify inst-1 (100s elapsed, longest running) is still running
        // and inst-2 and inst-3 (most recently started) were stopped
        let inst1_state = state.get_stats("inst-1").await.unwrap().state;
        let inst2_state = state.get_stats("inst-2").await.unwrap().state;
        let inst3_state = state.get_stats("inst-3").await.unwrap().state;

        assert!(matches!(inst1_state, FakerState::Running));
        assert!(matches!(inst2_state, FakerState::Stopped));
        assert!(matches!(inst3_state, FakerState::Stopped));
    }

    #[tokio::test]
    async fn test_start_instance_blocked_when_limit_reached() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::new(&temp.path().to_string_lossy());

        state.create_instance("inst-1", sample_torrent(1), FakerConfig::default()).await.unwrap();
        state.create_instance("inst-2", sample_torrent(2), FakerConfig::default()).await.unwrap();

        // Set limit to 1 active
        let mut settings = MaxActiveSettings::default();
        settings.global_max_active_enabled = true;
        settings.global_min_active = Some(1);
        settings.global_max_active = Some(1);
        state.set_max_active_settings(settings).await.unwrap();

        // Set inst-1 to Running
        {
            let instances = state.instances.read().await;
            let inst1 = instances.get("inst-1").unwrap();
            let mut stats = inst1.faker.stats_snapshot();
            stats.state = FakerState::Running;
            inst1.faker.restore_snapshot(stats).await;
        }

        // Start inst-2 should be blocked
        let err = state.start_instance("inst-2").await;
        assert!(err.is_err());
        let err_msg = err.unwrap_err();
        assert!(err_msg.contains("Maximum active instances limit"));
    }

    #[tokio::test]
    async fn test_max_active_settings_persistence() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_string_lossy().to_string();

        let state = AppState::new(&path);

        let mut settings = MaxActiveSettings::default();
        settings.global_max_active_enabled = true;
        settings.global_min_active = Some(2);
        settings.global_max_active = Some(4);
        state.set_max_active_settings(settings.clone()).await.unwrap();

        // Load new AppState from same directory
        let restored_state = AppState::new(&path);
        restored_state.load_saved_state().await.unwrap();

        let loaded = restored_state.get_max_active_settings().await;
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert!(loaded.global_max_active_enabled);
        assert_eq!(loaded.global_min_active, Some(2));
        assert_eq!(loaded.global_max_active, Some(4));
    }

    #[tokio::test]
    async fn test_roll_and_apply_max_active_limits() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::new(&temp.path().to_string_lossy());

        let mut settings = MaxActiveSettings::default();
        settings.global_max_active_enabled = true;
        settings.global_min_active = Some(3);
        settings.global_max_active = Some(5);
        state.set_max_active_settings(settings).await.unwrap();

        let updated = roll_and_apply_max_active_limits(&state).await.unwrap();
        assert!(updated.current_effective_global_limit.is_some());
        let limit = updated.current_effective_global_limit.unwrap();
        assert!((3..=5).contains(&limit));
        assert!(updated.last_randomized_at.is_some());
    }

    #[tokio::test]
    async fn test_scaling_up_respects_start_conditions() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::new(&temp.path().to_string_lossy());

        let mut cfg = FakerConfig::default();
        cfg.start_when_leechers_above = Some(10); // Condition: leechers > 10

        state.create_instance("inst-1", sample_torrent(1), cfg).await.unwrap();

        let mut settings = MaxActiveSettings::default();
        settings.global_max_active_enabled = true;
        settings.global_min_active = Some(1);
        settings.global_max_active = Some(1);
        state.set_max_active_settings(settings).await.unwrap();

        let instances_map = state.instances.clone();

        // inst-1 has leechers = 0, so condition (leechers > 10) is NOT met.
        manage_max_active_and_queue(&state, &instances_map, false).await;

        // inst-1 should NOT be started because start condition was not satisfied (marked Idle)
        let inst_state = state.get_stats("inst-1").await.unwrap().state;
        assert!(matches!(inst_state, FakerState::Idle));
    }
}
