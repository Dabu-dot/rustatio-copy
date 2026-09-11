use super::state::AppState;
use async_trait::async_trait;
use rustatio_core::logger::set_instance_context_str;
use rustatio_core::FakerStats;
use std::sync::Arc;

fn resolve_instance_label(
    instances: &std::collections::HashMap<String, super::instance::FakerInstance>,
    id: &str,
) -> String {
    instances
        .get(id)
        .map(|instance| instance.summary.name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| id.to_string())
}

#[async_trait]
pub trait InstanceLifecycle {
    async fn start_instance(&self, id: &str) -> Result<(), String>;
    async fn recover_tracker_instance(&self, id: &str) -> Result<FakerStats, String>;
    async fn stop_instance(&self, id: &str) -> Result<FakerStats, String>;
    async fn pause_instance(&self, id: &str) -> Result<(), String>;
    async fn resume_instance(&self, id: &str) -> Result<(), String>;
    async fn update_instance(&self, id: &str) -> Result<FakerStats, String>;
    async fn update_stats_only(&self, id: &str) -> Result<FakerStats, String>;
}

#[async_trait]
impl InstanceLifecycle for AppState {
    async fn start_instance(&self, id: &str) -> Result<(), String> {
        let label = {
            let instances = self.instances.read().await;
            resolve_instance_label(&instances, id)
        };
        set_instance_context_str(Some(&label));

        let (faker, restore, is_already_active, tracker_host) = {
            let instances = self.instances.read().await;
            let instance = instances.get(id).ok_or("Instance not found")?;
            let stats = instance.faker.stats_snapshot();
            let is_already_active = matches!(
                stats.state,
                rustatio_core::FakerState::Running | rustatio_core::FakerState::Starting
            );
            let restore = is_already_active && stats.elapsed_time.as_secs() > 0;
            let tracker_host =
                rustatio_core::primary_tracker_host(&instance.summary.announce).unwrap_or_default();
            (Arc::clone(&instance.faker), restore, is_already_active, tracker_host)
        };

        if !is_already_active {
            if let Some(settings) = self.get_max_active_settings().await {
                let instances = self.instances.read().await;
                let mut current_total_running = 0u32;
                let mut current_tracker_running = 0u32;

                for inst in instances.values() {
                    let st = inst.faker.stats_snapshot().state;
                    if matches!(
                        st,
                        rustatio_core::FakerState::Running | rustatio_core::FakerState::Starting
                    ) {
                        current_total_running += 1;
                        let host = rustatio_core::primary_tracker_host(&inst.summary.announce)
                            .unwrap_or_default();
                        if host == tracker_host {
                            current_tracker_running += 1;
                        }
                    }
                }

                if settings.global_max_active_enabled {
                    let global_limit =
                        settings.current_effective_global_limit.or(settings.global_max_active);
                    if let Some(limit) = global_limit {
                        if current_total_running >= limit {
                            return Err(format!(
                                "Maximum active instances limit ({limit}) reached"
                            ));
                        }
                    }
                }

                if let Some(tr_setting) = settings.tracker_max_active.get(&tracker_host) {
                    if tr_setting.enabled {
                        let tr_limit = settings
                            .current_effective_tracker_limits
                            .get(&tracker_host)
                            .copied()
                            .unwrap_or(tr_setting.max_active);
                        if current_tracker_running >= tr_limit {
                            return Err(format!(
                                "Maximum active instances limit for tracker '{tracker_host}' ({tr_limit}) reached"
                            ));
                        }
                    }
                }
            }
        }

        if restore {
            faker.restore_running().await.map_err(|e| e.to_string())?;
        } else {
            faker.start().await.map_err(|e| e.to_string())?;
        }
        if let Err(e) = self.save_state().await {
            tracing::warn!("Failed to save state after starting instance: {}", e);
        }

        self.refresh_peer_listener_port().await;

        Ok(())
    }

    async fn recover_tracker_instance(&self, id: &str) -> Result<FakerStats, String> {
        let label = {
            let instances = self.instances.read().await;
            resolve_instance_label(&instances, id)
        };
        set_instance_context_str(Some(&label));

        let faker = {
            let instances = self.instances.read().await;
            let instance = instances.get(id).ok_or("Instance not found")?;
            Arc::clone(&instance.faker)
        };

        let stats = faker.recover_tracker().await.map_err(|e| e.to_string())?;

        {
            let mut instances = self.instances.write().await;
            if let Some(instance) = instances.get_mut(id) {
                instance.cumulative_uploaded = stats.uploaded;
                instance.cumulative_downloaded = stats.downloaded;
                instance.config.completion_percent = stats.torrent_completion;
            }
        }

        if let Err(e) = self.save_state().await {
            tracing::warn!("Failed to save state after tracker recovery attempt: {}", e);
        }

        self.refresh_peer_listener_port().await;

        Ok(stats)
    }

    async fn stop_instance(&self, id: &str) -> Result<FakerStats, String> {
        let label = {
            let instances = self.instances.read().await;
            resolve_instance_label(&instances, id)
        };
        set_instance_context_str(Some(&label));

        let faker = {
            let instances = self.instances.read().await;
            let instance = instances.get(id).ok_or("Instance not found")?;
            Arc::clone(&instance.faker)
        };

        faker.stop().await.map_err(|e| e.to_string())?;
        let mut stats = faker.stats_snapshot();
        stats.manually_stopped = true;
        faker.restore_snapshot(stats.clone()).await;

        {
            let mut instances = self.instances.write().await;
            if let Some(instance) = instances.get_mut(id) {
                instance.cumulative_uploaded = stats.uploaded;
                instance.cumulative_downloaded = stats.downloaded;
                instance.config.completion_percent = stats.torrent_completion;
            }
        }

        if let Err(e) = self.save_state().await {
            tracing::warn!("Failed to save state after stopping instance: {}", e);
        }

        self.refresh_peer_listener_port().await;

        Ok(stats)
    }

    async fn pause_instance(&self, id: &str) -> Result<(), String> {
        let label = {
            let instances = self.instances.read().await;
            resolve_instance_label(&instances, id)
        };
        set_instance_context_str(Some(&label));

        let faker = {
            let instances = self.instances.read().await;
            let instance = instances.get(id).ok_or("Instance not found")?;
            Arc::clone(&instance.faker)
        };

        faker.pause().await.map_err(|e| e.to_string())?;
        if let Err(e) = self.save_state().await {
            tracing::warn!("Failed to save state after pausing instance: {}", e);
        }

        self.refresh_peer_listener_port().await;

        Ok(())
    }

    async fn resume_instance(&self, id: &str) -> Result<(), String> {
        let label = {
            let instances = self.instances.read().await;
            resolve_instance_label(&instances, id)
        };
        set_instance_context_str(Some(&label));

        let faker = {
            let instances = self.instances.read().await;
            let instance = instances.get(id).ok_or("Instance not found")?;
            Arc::clone(&instance.faker)
        };

        faker.resume().await.map_err(|e| e.to_string())?;
        if let Err(e) = self.save_state().await {
            tracing::warn!("Failed to save state after resuming instance: {}", e);
        }

        self.refresh_peer_listener_port().await;

        Ok(())
    }

    async fn update_instance(&self, id: &str) -> Result<FakerStats, String> {
        let label = {
            let instances = self.instances.read().await;
            resolve_instance_label(&instances, id)
        };
        set_instance_context_str(Some(&label));

        let faker = {
            let instances = self.instances.read().await;
            let instance = instances.get(id).ok_or("Instance not found")?;
            Arc::clone(&instance.faker)
        };

        faker.update().await.map_err(|e| e.to_string())?;
        let stats = faker.stats_snapshot();

        {
            let mut instances = self.instances.write().await;
            if let Some(instance) = instances.get_mut(id) {
                instance.cumulative_uploaded = stats.uploaded;
                instance.cumulative_downloaded = stats.downloaded;
                instance.config.completion_percent = stats.torrent_completion;
            }
        }

        Ok(stats)
    }

    async fn update_stats_only(&self, id: &str) -> Result<FakerStats, String> {
        let label = {
            let instances = self.instances.read().await;
            resolve_instance_label(&instances, id)
        };
        set_instance_context_str(Some(&label));

        let faker = {
            let instances = self.instances.read().await;
            let instance = instances.get(id).ok_or("Instance not found")?;
            Arc::clone(&instance.faker)
        };

        faker.update_stats_only().await.map_err(|e| e.to_string())?;
        let stats = faker.stats_snapshot();

        {
            let mut instances = self.instances.write().await;
            if let Some(instance) = instances.get_mut(id) {
                instance.cumulative_uploaded = stats.uploaded;
                instance.cumulative_downloaded = stats.downloaded;
                instance.config.completion_percent = stats.torrent_completion;
            }
        }

        Ok(stats)
    }
}
