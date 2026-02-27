use crate::config::Config;
use crate::types::DnsMessage;
use crate::plugin::SharedState;
use anyhow::Result;
use std::sync::Arc;
use tokio::net::{UdpSocket, TcpListener};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;
use std::collections::HashMap;

pub struct DnsServer {
    config: Arc<Config>,
    _shared: Arc<SharedState>,
}

impl DnsServer {
    pub fn new(config: Config, shared: Arc<SharedState>) -> Result<Self> {
        Ok(Self { config: Arc::new(config), _shared: shared })
    }

    pub async fn run(&self, default_address: String, mut reload_rx: watch::Receiver<bool>) -> Result<bool> {
        // 提取默认绑定的 IP（比如 0.0.0.0），但舍弃默认的 53 端口
        let base_ip = default_address.split(':').next().unwrap_or("0.0.0.0");

        // 按监听端口分组 (bind_addr -> Vec<Zone Index>)
        // 这样可以支持在同一个端口上配置多个不同的域名后缀 (如 a.com:53 和 b.com:53)
        let mut bind_map: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, zone) in self.config.zones.iter().enumerate() {
            // 解析 Corefile 中定义的端口，比如 ".:1053" 提取出 "1053"
            let port = if let Some(idx) = zone.name.rfind(':') {
                &zone.name[idx + 1..]
            } else {
                "53"
            };
            let bind_addr = format!("{}:{}", base_ip, port);
            bind_map.entry(bind_addr).or_default().push(i);
        }

        // 存放所有异步监听任务的句柄，方便重载时安全销毁
        let mut tasks = Vec::new();

        // 为 Corefile 里定义的每一个独立端口，分配专属的 UDP 和 TCP 监听器
        for (bind_addr, zone_indices) in bind_map {
            let udp_socket = match UdpSocket::bind(&bind_addr).await {
                Ok(s) => Arc::new(s),
                Err(e) => {
                    tracing::error!("Failed to bind UDP {}: {}", bind_addr, e);
                    continue;
                }
            };
            let tcp_listener = match TcpListener::bind(&bind_addr).await {
                Ok(s) => Arc::new(s),
                Err(e) => {
                    tracing::error!("Failed to bind TCP {}: {}", bind_addr, e);
                    continue;
                }
            };

            let port = bind_addr.split(':').last().unwrap_or("53").parse::<u16>().unwrap_or(53);
            tracing::info!("🚀 Server successfully bound to TCP & UDP on {} for {} zone(s)", bind_addr, zone_indices.len());

            // ==============================
            // 分支 1: UDP 协议处理流水线
            // ==============================
            let config_udp = self.config.clone();
            let socket_udp = udp_socket.clone();
            let zones_udp = zone_indices.clone();
            
            let udp_task = tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                loop {
                    if let Ok((size, src)) = socket_udp.recv_from(&mut buf).await {
                        let query = buf[..size].to_vec();
                        let config = config_udp.clone();
                        let socket = socket_udp.clone();
                        let z_indices = zones_udp.clone();

                        tokio::spawn(async move {
                            let mut msg = DnsMessage::default();
                            msg.raw_query = query;
                            msg.client_addr = Some(src);
                            msg.protocol = "udp".to_string();
                            msg.server_port = Some(port);

                            if msg.raw_query.len() >= 12 {
                                msg.header.id = ((msg.raw_query[0] as u16) << 8) | (msg.raw_query[1] as u16);
                            }

                            // 默认分配给绑定在该端口上的第一个 Zone 块配置
                            let target_zone_idx = z_indices[0]; 
                            let mut final_msg = msg.clone();

                            for plugin in &config.zones[target_zone_idx].plugins {
                                if final_msg.halt_chain { break; }
                                if let Ok(new_msg) = plugin.process(&mut final_msg).await { final_msg = new_msg; }
                            }
                            for plugin in config.zones[target_zone_idx].plugins.iter().rev() {
                                let _ = plugin.post_process(&mut final_msg).await;
                            }

                            if let Some(resp) = final_msg.raw_response {
                                let mut final_resp = resp;
                                if final_resp.len() > 1232 {
                                    final_resp.truncate(1232);
                                    final_resp[2] |= 0x02; // 打上 TC(Truncated) 截断标志
                                }
                                let _ = socket.send_to(&final_resp, src).await;
                            }
                        });
                    }
                }
            });
            tasks.push(udp_task);

            // ==============================
            // 分支 2: TCP 协议处理流水线
            // ==============================
            let config_tcp = self.config.clone();
            let listener_tcp = tcp_listener.clone();
            let zones_tcp = zone_indices.clone();
            
            let tcp_task = tokio::spawn(async move {
                loop {
                    if let Ok((mut stream, src)) = listener_tcp.accept().await {
                        let config = config_tcp.clone();
                        let z_indices = zones_tcp.clone();

                        tokio::spawn(async move {
                            let mut len_buf = [0u8; 2];
                            if stream.read_exact(&mut len_buf).await.is_err() { return; }
                            let len = u16::from_be_bytes(len_buf) as usize;
                            
                            let mut query = vec![0u8; len];
                            if stream.read_exact(&mut query).await.is_err() { return; }

                            let mut msg = DnsMessage::default();
                            msg.raw_query = query;
                            msg.client_addr = Some(src);
                            msg.protocol = "tcp".to_string();
                            msg.server_port = Some(port);

                            if msg.raw_query.len() >= 12 {
                                msg.header.id = ((msg.raw_query[0] as u16) << 8) | (msg.raw_query[1] as u16);
                            }

                            let target_zone_idx = z_indices[0];
                            let mut final_msg = msg.clone();

                            for plugin in &config.zones[target_zone_idx].plugins {
                                if final_msg.halt_chain { break; }
                                if let Ok(new_msg) = plugin.process(&mut final_msg).await { final_msg = new_msg; }
                            }
                            for plugin in config.zones[target_zone_idx].plugins.iter().rev() {
                                let _ = plugin.post_process(&mut final_msg).await;
                            }

                            if let Some(resp) = final_msg.raw_response {
                                let resp_len = resp.len() as u16;
                                let _ = stream.write_all(&resp_len.to_be_bytes()).await;
                                let _ = stream.write_all(&resp).await;
                            }
                        });
                    }
                }
            });
            tasks.push(tcp_task);
        }

        // ==============================
        // 分支 3: 监听热重载与平滑退出
        // ==============================
        tokio::select! {
            _ = reload_rx.changed() => {
                // 如果收到重载信号，立刻取消当前所有端口的监听任务，释放端口
                for task in tasks {
                    task.abort();
                }
                return Ok(true);
            }
        }
    }
}