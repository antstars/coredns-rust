use crate::plugin::{Plugin, SharedState};
use crate::config::PluginConfig;
use crate::types::DnsMessage;
use anyhow::Result;
use std::sync::Arc;

pub struct LogPlugin;

#[async_trait::async_trait]
impl Plugin for LogPlugin {
    fn name(&self) -> &str { "log" }
    fn from_config(config: &PluginConfig, _: Arc<SharedState>) -> Result<Self> { 
        tracing::info!("[log] Initialized for zones: {:?}", config.args);
        Ok(Self) 
    }
    async fn process(&self, msg: &mut DnsMessage) -> Result<DnsMessage> {
        tracing::info!("=> [Incoming Query] TxID: {:#06x}", msg.header.id);
        Ok(msg.clone())
    }
    fn priority(&self) -> u8 { 255 }
}