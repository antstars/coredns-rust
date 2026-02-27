use crate::plugin::{Plugin, SharedState};
use crate::config::PluginConfig;
use crate::types::DnsMessage;
use anyhow::Result;
use std::sync::Arc;

pub struct DummyPlugin;

#[async_trait::async_trait]
impl Plugin for DummyPlugin {
    fn name(&self) -> &str { "dummy" }
    fn from_config(_: &PluginConfig, _: Arc<SharedState>) -> Result<Self> { Ok(Self) }
    async fn process(&self, msg: &mut DnsMessage) -> Result<DnsMessage> { Ok(msg.clone()) }
    fn priority(&self) -> u8 { 0 }
}