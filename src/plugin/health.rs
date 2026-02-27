use crate::plugin::{Plugin, SharedState};
use crate::config::PluginConfig;
use crate::types::DnsMessage;
use anyhow::Result;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub struct HealthPlugin {
    _handle: tokio::task::JoinHandle<()>, 
}

#[async_trait::async_trait]
impl Plugin for HealthPlugin {
    fn name(&self) -> &str { "health" }
    
    fn from_config(config: &PluginConfig, _: Arc<SharedState>) -> Result<Self> {
        let mut port = config.args.first().cloned().unwrap_or_else(|| ":8080".to_string());
        if !port.contains(':') { port = format!(":{}", port); }
        let addr = format!("0.0.0.0{}", port);
        
        let handle = tokio::spawn(async move {
            match TcpListener::bind(&addr).await {
                Ok(listener) => {
                    tracing::info!("[health] Successfully bound listener on {}", addr);
                    let mut buf = [0u8; 1024];
                    while let Ok((mut stream, _)) = listener.accept().await {
                        let _ = stream.read(&mut buf).await;
                        let response = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nOK";
                        let _ = stream.write_all(response).await;
                    }
                }
                Err(_) => {
                    // 【降级为 INFO】：多 Zone 配置同端口时，第一个已经成功绑定，后续的直接复用提示即可，不报红错
                    tracing::info!("[health] Port {} is already active (shared with another zone).", addr);
                }
            }
        });
        
        Ok(Self { _handle: handle })
    }
    
    async fn process(&self, msg: &mut DnsMessage) -> Result<DnsMessage> { Ok(msg.clone()) }
    fn priority(&self) -> u8 { 10 }
}

impl Drop for HealthPlugin {
    fn drop(&mut self) {
        self._handle.abort(); 
    }
}