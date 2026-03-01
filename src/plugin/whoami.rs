use crate::plugin::{Plugin, SharedState};
use crate::config::PluginConfig;
use crate::types::DnsMessage;
use anyhow::Result;
use std::sync::Arc;
use std::net::IpAddr;

pub struct WhoamiPlugin;

#[async_trait::async_trait]
impl Plugin for WhoamiPlugin {
    fn name(&self) -> &str { "whoami" }

    fn from_config(_config: &PluginConfig, _shared: Arc<SharedState>) -> Result<Self> {
        tracing::info!("[whoami] Plugin initialized");
        Ok(Self)
    }

    async fn process(&self, msg: &mut DnsMessage) -> Result<DnsMessage> {
        if msg.halt_chain || msg.raw_query.len() < 12 || msg.client_addr.is_none() {
            return Ok(msg.clone());
        }

        let mut offset = 12;
        let mut qname_bytes = Vec::new();
        while offset < msg.raw_query.len() {
            let len = msg.raw_query[offset] as usize;
            if len == 0 {
                qname_bytes.push(0);
                offset += 1;
                break;
            }
            qname_bytes.push(len as u8);
            offset += 1;
            for _ in 0..len {
                if offset < msg.raw_query.len() {
                    qname_bytes.push(msg.raw_query[offset]);
                    offset += 1;
                }
            }
        }

        let qtype = if offset + 1 < msg.raw_query.len() {
            ((msg.raw_query[offset] as u16) << 8) | (msg.raw_query[offset + 1] as u16)
        } else { 0 };

        if qtype != 1 && qtype != 28 { return Ok(msg.clone()); }

        // Safely extract client address (already checked is_none above, but use if let for safety)
        let client = match &msg.client_addr {
            Some(addr) => addr,
            None => {
                tracing::debug!("[whoami] No client address available, skipping");
                return Ok(msg.clone());
            }
        };
        let client_ip = client.ip();
        let client_port = client.port();

        let mut resp = Vec::with_capacity(512);
        resp.extend_from_slice(&msg.raw_query[0..2]); 
        resp.extend_from_slice(&[0x81, 0x80]); 
        resp.extend_from_slice(&[0x00, 0x01]); 
        resp.extend_from_slice(&[0x00, 0x00]); 
        resp.extend_from_slice(&[0x00, 0x00]); 
        resp.extend_from_slice(&[0x00, 0x02]); 

        resp.extend_from_slice(&qname_bytes);
        resp.extend_from_slice(&msg.raw_query[offset..offset + 4]); 

        resp.extend_from_slice(&[0xC0, 0x0C]); 
        match client_ip {
            IpAddr::V4(ipv4) => {
                resp.extend_from_slice(&[0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04]); 
                resp.extend_from_slice(&ipv4.octets());
            }
            IpAddr::V6(ipv6) => {
                resp.extend_from_slice(&[0x00, 0x1C, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10]); 
                resp.extend_from_slice(&ipv6.octets());
            }
        }

        let proto = if msg.protocol == "tcp" { b"_tcp" } else { b"_udp" };
        resp.push(proto.len() as u8);
        resp.extend_from_slice(proto);
        resp.extend_from_slice(&[0xC0, 0x0C]); 

        resp.extend_from_slice(&[0x00, 0x21, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07]); 
        resp.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); 
        resp.extend_from_slice(&[(client_port >> 8) as u8, (client_port & 0xFF) as u8]); 
        resp.push(0x00); 

        msg.raw_response = Some(resp);
        msg.halt_chain = true; 
        
        tracing::info!("    |-- [whoami] Responded to client {}:{}", client_ip, client_port);
        Ok(msg.clone())
    }
    fn priority(&self) -> u8 { 200 }
}