use crate::plugin::{Plugin, SharedState};
use crate::config::PluginConfig;
use crate::types::DnsMessage;
use crate::plugin::prometheus::{RELOAD_FAILED_TOTAL, RELOAD_VERSION_INFO};
use anyhow::Result;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use sha2::{Sha512, Digest};
use rand::Rng;

pub struct ReloadPlugin {
    _handle: tokio::task::JoinHandle<()>,
}

#[async_trait::async_trait]
impl Plugin for ReloadPlugin {
    fn name(&self) -> &str { "reload" }

    fn from_config(config: &PluginConfig, shared: Arc<SharedState>) -> Result<Self> {
        let mut interval = Duration::from_secs(30);
        let mut jitter = Duration::from_secs(15);

        if config.args.len() > 0 {
            interval = parse_duration(&config.args[0]).unwrap_or(Duration::from_secs(30));
            if interval < Duration::from_secs(2) { interval = Duration::from_secs(2); }
        }
        if config.args.len() > 1 {
            jitter = parse_duration(&config.args[1]).unwrap_or(Duration::from_secs(15));
            if jitter < Duration::from_secs(1) { jitter = Duration::from_secs(1); }
        }

        if jitter > interval / 2 { jitter = interval / 2; }

        let path = shared.config_path.clone();
        
        // 【核心 Bug 修复点】：
        // tokio::sync::watch::Sender 无法被 clone。
        // 我们改为 clone 整个 SharedState 的 Arc 智能指针，这样就能安全地在子协程中调用 send。
        let shared_clone = shared.clone();

        let initial_hash = hash_file(&path).unwrap_or_default();
        RELOAD_VERSION_INFO.with_label_values(&["sha512", &initial_hash]).set(1.0);

        tracing::info!("[reload] Watching changes for {} (Interval: {:?}, Jitter: {:?})", path, interval, jitter);

        let handle = tokio::spawn(async move {
            let current_hash = initial_hash.clone();
            loop {
                let sleep_time = {
                    let mut rng = rand::thread_rng();
                    let j = rng.gen_range(0..=(jitter.as_millis() as u64 * 2));
                    let j_offset = j as i64 - jitter.as_millis() as i64;
                    
                    if j_offset > 0 { interval + Duration::from_millis(j_offset as u64) } 
                    else { interval - Duration::from_millis(-j_offset as u64) }
                };

                sleep(sleep_time).await;

                match hash_file(&path) {
                    Ok(new_hash) => {
                        if new_hash != current_hash {
                            tracing::info!("[reload] Corefile change detected! New SHA512: {}", new_hash);
                            RELOAD_VERSION_INFO.with_label_values(&["sha512", &current_hash]).set(0.0);
                            RELOAD_VERSION_INFO.with_label_values(&["sha512", &new_hash]).set(1.0);
                            
                            // 【核心 Bug 修复点】：安全地通过 Arc 引用发送热重载信号
                            let _ = shared_clone.reload_tx.send(true);
                            break; 
                        }
                    }
                    Err(e) => {
                        tracing::error!("[reload] Failed to read Corefile: {}", e);
                        RELOAD_FAILED_TOTAL.inc();
                    }
                }
            }
        });

        Ok(Self { _handle: handle })
    }

    async fn process(&self, msg: &mut DnsMessage) -> Result<DnsMessage> { Ok(msg.clone()) }
    fn priority(&self) -> u8 { 190 }
}

impl Drop for ReloadPlugin {
    fn drop(&mut self) {
        self._handle.abort();
    }
}

fn hash_file(path: &str) -> Result<String> {
    let content = std::fs::read(path)?;
    let mut hasher = Sha512::new();
    hasher.update(&content);
    Ok(hex::encode(hasher.finalize()))
}

fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    if let Some(stripped) = s.strip_suffix("ms") { Ok(Duration::from_millis(stripped.parse()?)) }
    else if let Some(stripped) = s.strip_suffix('s') { Ok(Duration::from_secs(stripped.parse()?)) }
    else if let Some(stripped) = s.strip_suffix('m') { Ok(Duration::from_secs(stripped.parse::<u64>()? * 60)) }
    else if let Some(stripped) = s.strip_suffix('h') { Ok(Duration::from_secs(stripped.parse::<u64>()? * 3600)) }
    else { anyhow::bail!("invalid duration") }
}