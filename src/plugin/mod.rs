pub mod cache;
pub mod errors;
pub mod forward;
pub mod log;
pub mod prometheus;
pub mod reload;
pub mod health;
pub mod whoami;
pub mod stubs;

use anyhow::Result;
use std::sync::Arc;
use crate::config::PluginConfig;
use crate::types::DnsMessage;

#[async_trait::async_trait]
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn from_config(config: &PluginConfig, shared: Arc<SharedState>) -> Result<Self> where Self: Sized;
    async fn process(&self, msg: &mut DnsMessage) -> Result<DnsMessage>;
    async fn post_process(&self, _msg: &mut DnsMessage) -> Result<()> {
        Ok(())
    }
    fn priority(&self) -> u8;
}

pub struct SharedState {
    pub cache_preserve: Arc<crate::plugin::cache::CacheStore>, 
    pub reload_tx: tokio::sync::watch::Sender<bool>,
    pub reload_rx: std::sync::Mutex<Option<tokio::sync::watch::Receiver<bool>>>,
    pub error_tx: tokio::sync::mpsc::Sender<String>,
    pub error_rx: std::sync::Mutex<Option<tokio::sync::mpsc::Receiver<String>>>,
    pub config_path: String,
}

impl SharedState {
    pub fn new_with_cache(cache_preserve: Arc<crate::plugin::cache::CacheStore>, config_path: String) -> Self {
        // 使用 watch channel 传递热重载信号，支持一对多广播
        let (reload_tx, reload_rx) = tokio::sync::watch::channel(false);
        let (error_tx, error_rx) = tokio::sync::mpsc::channel(100);
        Self {
            cache_preserve,
            reload_tx,
            reload_rx: std::sync::Mutex::new(Some(reload_rx)),
            error_tx,
            error_rx: std::sync::Mutex::new(Some(error_rx)),
            config_path,
        }
    }
}

// 恢复工厂函数，供 config.rs 使用
pub fn create_plugin(config: &PluginConfig, shared: Arc<SharedState>) -> Result<Box<dyn Plugin>> {
    match config.name.as_str() {
        "cache" => Ok(Box::new(cache::CachePlugin::from_config(config, shared)?)),
        "forward" => Ok(Box::new(forward::ForwardPlugin::from_config(config, shared)?)),
        "prometheus" => Ok(Box::new(prometheus::PrometheusPlugin::from_config(config, shared)?)),
        "log" => Ok(Box::new(log::LogPlugin::from_config(config, shared)?)),
        "errors" => Ok(Box::new(errors::ErrorsPlugin::from_config(config, shared)?)),
        "reload" => Ok(Box::new(reload::ReloadPlugin::from_config(config, shared)?)),
        "health" => Ok(Box::new(health::HealthPlugin::from_config(config, shared)?)),
        "whoami" => Ok(Box::new(whoami::WhoamiPlugin::from_config(config, shared)?)),
        
        // 【关键修复】：把 "stubs" 改为 "dummy"，并调用 stubs 模块里的 DummyPlugin
        "dummy" => Ok(Box::new(stubs::DummyPlugin::from_config(config, shared)?)),
        
        _ => anyhow::bail!("Unknown plugin: {}", config.name),
    }
}