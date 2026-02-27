use crate::plugin::{Plugin, SharedState};
use crate::config::PluginConfig;
use crate::types::DnsMessage;
use crate::plugin::prometheus::{CACHE_REQUESTS_TOTAL, CACHE_HITS_TOTAL, CACHE_MISSES_TOTAL, CACHE_ENTRIES};
use anyhow::Result;
use std::sync::Arc;
use std::time::{Duration, Instant};
use moka::sync::Cache;

#[derive(Clone)]
pub struct CachedItem {
    pub response: Vec<u8>,
    pub expires_at: Instant,
}

pub struct CacheStore {
    pub success: Cache<Vec<u8>, CachedItem>,
    pub denial: Cache<Vec<u8>, CachedItem>,
}

impl CacheStore {
    pub fn new() -> Self {
        Self {
            // Moka 会使用高效的 W-TinyLFU 算法自动淘汰，无需手动遍历锁
            success: Cache::builder().max_capacity(50_000).build(),
            denial: Cache::builder().max_capacity(50_000).build(),
        }
    }
}

pub struct CachePlugin {
    success_ttl: Duration,
    denial_ttl: Duration,
    servfail_ttl: Duration,
    store: Arc<CacheStore>,
}

#[async_trait::async_trait]
impl Plugin for CachePlugin {
    fn name(&self) -> &str { "cache" }

    fn from_config(config: &PluginConfig, shared: Arc<SharedState>) -> Result<Self> {
        let mut success_ttl = Duration::from_secs(3600);
        let mut denial_ttl = Duration::from_secs(1800);
        let mut servfail_ttl = Duration::from_secs(5);

        for sub in &config.block {
            match sub.name.as_str() {
                "success" => { if sub.args.len() > 1 { success_ttl = Duration::from_secs(sub.args[1].parse().unwrap_or(3600)); } }
                "denial" => { if sub.args.len() > 1 { denial_ttl = Duration::from_secs(sub.args[1].parse().unwrap_or(1800)); } }
                "servfail" => {
                    if sub.args.len() > 0 {
                        let secs = sub.args[0].strip_suffix('s').unwrap_or(&sub.args[0]).parse().unwrap_or(5);
                        servfail_ttl = Duration::from_secs(secs);
                    }
                }
                _ => {}
            }
        }

        tracing::info!("[cache] Initialized (Success TTL: {}s, Denial TTL: {}s). Bound to Global LRU Pool.", success_ttl.as_secs(), denial_ttl.as_secs());

        Ok(Self {
            success_ttl, denial_ttl, servfail_ttl,
            store: shared.cache_preserve.clone(), // 继承全局缓存，无惧热重载！
        })
    }

    async fn process(&self, msg: &mut DnsMessage) -> Result<DnsMessage> {
        if msg.halt_chain || msg.raw_query.len() < 12 { return Ok(msg.clone()); }

        let server_label = format!("dns://:{}", msg.server_port.unwrap_or(53));
        CACHE_REQUESTS_TOTAL.with_label_values(&[&server_label, "", "."]).inc();

        if let Some(key) = extract_question_bytes(&msg.raw_query) {
            let now = Instant::now();
            
            // 无锁高并发读取
            if let Some(item) = self.store.success.get(&key) {
                if item.expires_at > now {
                    tracing::info!("     |-- [cache] HIT Success! TxID: {:#06x}", msg.header.id);
                    return Ok(build_cached_response(msg, item, &server_label, "success"));
                } else {
                    self.store.success.invalidate(&key);
                }
            }

            if let Some(item) = self.store.denial.get(&key) {
                if item.expires_at > now {
                    tracing::info!("     |-- [cache] HIT Denial! TxID: {:#06x}", msg.header.id);
                    return Ok(build_cached_response(msg, item, &server_label, "denial"));
                } else {
                    self.store.denial.invalidate(&key);
                }
            }
        }
        
        CACHE_MISSES_TOTAL.with_label_values(&[&server_label, "", "."]).inc();
        Ok(msg.clone())
    }

    async fn post_process(&self, msg: &mut DnsMessage) -> Result<()> {
        let server_label = format!("dns://:{}", msg.server_port.unwrap_or(53));

        if let Some(resp) = &msg.raw_response {
            if let Some(key) = extract_question_bytes(&msg.raw_query) {
                let rcode = resp[3] & 0x0F;
                let now = Instant::now();
                
                if rcode == 0 { 
                    self.store.success.insert(key, CachedItem { response: resp.clone(), expires_at: now + self.success_ttl });
                    CACHE_ENTRIES.with_label_values(&[&server_label, "success", "", "."]).set(self.store.success.entry_count() as f64);
                } else if rcode == 3 || (rcode == 2 && self.servfail_ttl.as_secs() > 0) { 
                    let ttl = if rcode == 3 { self.denial_ttl } else { self.servfail_ttl };
                    self.store.denial.insert(key, CachedItem { response: resp.clone(), expires_at: now + ttl });
                    CACHE_ENTRIES.with_label_values(&[&server_label, "denial", "", "."]).set(self.store.denial.entry_count() as f64);
                }
            }
        }
        Ok(())
    }

    fn priority(&self) -> u8 { 120 }
}

fn build_cached_response(msg: &mut DnsMessage, item: CachedItem, server_label: &str, cache_type: &str) -> DnsMessage {
    let mut resp = item.response;
    resp[0] = msg.raw_query[0]; 
    resp[1] = msg.raw_query[1];
    msg.raw_response = Some(resp);
    msg.halt_chain = true;
    msg.answered_by = "cache".to_string();
    CACHE_HITS_TOTAL.with_label_values(&[server_label, cache_type, "", "."]).inc();
    msg.clone()
}

fn extract_question_bytes(query: &[u8]) -> Option<Vec<u8>> {
    if query.len() < 12 { return None; }
    let mut offset = 12;
    while offset < query.len() {
        let len = query[offset] as usize;
        offset += 1;
        if len == 0 { break; }
        offset += len;
    }
    if offset + 4 <= query.len() { return Some(query[12..offset+4].to_vec()); }
    None
}