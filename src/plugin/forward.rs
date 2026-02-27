use crate::plugin::{Plugin, SharedState};
use crate::config::PluginConfig;
use crate::types::DnsMessage;
use crate::plugin::prometheus::{
    PROXY_REQUEST_DURATION, PROXY_CONN_CACHE_HITS, PROXY_CONN_CACHE_MISSES, 
    FORWARD_MAX_CONCURRENT_REJECTS, rcode_to_str
};
use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
use tokio::net::{TcpStream, UdpSocket};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{timeout, sleep, Duration};
use tokio::sync::{Semaphore, Mutex as AsyncMutex};
use tokio_rustls::{TlsConnector, client::TlsStream, rustls::{ClientConfig, RootCertStore, ServerName}};
use rand::seq::SliceRandom;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Policy { Sequential, Random, RoundRobin }

struct IdleConnection {
    stream: TlsStream<TcpStream>,
    expires_at: std::time::Instant,
}

pub struct Upstream {
    pub ip: String,
    pub port: u16,
    pub is_tls: bool,
    pub is_healthy: Arc<AtomicBool>,
    pub fails: Arc<AtomicUsize>,
    idle_tls_conns: Arc<AsyncMutex<Vec<IdleConnection>>>, 
}

pub struct ForwardPlugin {
    pub upstreams: Vec<Arc<Upstream>>,
    pub tls_servername: Option<String>,
    pub failover_rcodes: Vec<u8>,
    pub next_rcodes: Vec<u8>,
    pub policy: Policy,
    pub except_domains: Vec<String>,
    pub force_tcp: bool,
    pub max_concurrent: Option<Arc<Semaphore>>,
    pub failfast: bool,
    pub max_idle_conns: usize,
    pub expire_duration: Duration,
    rr_counter: AtomicUsize,
    tls_connector: TlsConnector,
    error_tx: tokio::sync::mpsc::Sender<String>,
}

#[async_trait::async_trait]
impl Plugin for ForwardPlugin {
    fn name(&self) -> &str { "forward" }

    fn from_config(config: &PluginConfig, shared: Arc<SharedState>) -> Result<Self> {
        let mut upstreams = Vec::new();
        for arg in &config.args {
            if arg == "." || arg == "{}" { continue; }
            let is_tls = arg.starts_with("tls://");
            let clean_ip = arg.replace("tls://", "");
            let (ip, port) = if clean_ip.contains(':') {
                let parts: Vec<&str> = clean_ip.split(':').collect();
                (parts[0].to_string(), parts[1].parse().unwrap_or(if is_tls { 853 } else { 53 }))
            } else {
                (clean_ip, if is_tls { 853 } else { 53 })
            };
            upstreams.push(Arc::new(Upstream { 
                ip, port, is_tls,
                is_healthy: Arc::new(AtomicBool::new(true)),
                fails: Arc::new(AtomicUsize::new(0)),
                idle_tls_conns: Arc::new(AsyncMutex::new(Vec::new())),
            }));
        }

        let mut tls_servername = None;
        let mut failover_rcodes = Vec::new();
        let mut next_rcodes = Vec::new();
        let mut policy = Policy::Random; 
        let mut except_domains = Vec::new();
        let mut force_tcp = false;
        let mut failfast = false;
        let mut max_fails = 2;
        let mut health_check_interval = Duration::from_millis(500);
        let mut max_concurrent = None;
        let mut max_idle_conns = 0; 
        let mut expire_duration = Duration::from_secs(10);

        for sub in &config.block {
            match sub.name.as_str() {
                "tls_servername" => tls_servername = sub.args.first().cloned(),
                "failover" => { for arg in &sub.args { failover_rcodes.push(parse_rcode(arg)); } }
                "next" => { for arg in &sub.args { next_rcodes.push(parse_rcode(arg)); } }
                "except" => { except_domains = sub.args.clone(); }
                "force_tcp" => { force_tcp = true; }
                "failfast_all_unhealthy_upstreams" => { failfast = true; }
                "max_fails" => { if let Some(a) = sub.args.first() { max_fails = a.parse().unwrap_or(2); } }
                "max_idle_conns" => { if let Some(a) = sub.args.first() { max_idle_conns = a.parse().unwrap_or(0); } }
                "expire" => { if let Some(a) = sub.args.first() { expire_duration = parse_duration(a).unwrap_or(Duration::from_secs(10)); } }
                "max_concurrent" => { 
                    if let Some(a) = sub.args.first() { 
                        if let Ok(limit) = a.parse::<usize>() {
                            max_concurrent = Some(Arc::new(Semaphore::new(limit)));
                        }
                    } 
                }
                "health_check" => { 
                    if let Some(a) = sub.args.first() { health_check_interval = parse_duration(a).unwrap_or(Duration::from_millis(500)); } 
                }
                "policy" => {
                    if let Some(p) = sub.args.first() {
                        policy = match p.as_str() {
                            "sequential" => Policy::Sequential,
                            "round_robin" => Policy::RoundRobin,
                            _ => Policy::Random,
                        };
                    }
                }
                _ => {}
            }
        }

        let mut root_store = RootCertStore::empty();
        root_store.add_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.iter().map(|ta| {
            tokio_rustls::rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(ta.subject, ta.spki, ta.name_constraints)
        }));
        let client_config = ClientConfig::builder().with_safe_defaults().with_root_certificates(root_store).with_no_client_auth();
        let tls_connector = TlsConnector::from(Arc::new(client_config));

        if max_fails > 0 {
            for upstream in &upstreams {
                let up_clone = upstream.clone();
                let interval = health_check_interval;
                let fails_limit = max_fails;
                let tls_conn_clone = tls_connector.clone();
                let sni = tls_servername.clone();
                
                tokio::spawn(async move {
                    loop {
                        sleep(interval).await;
                        let probe_query = build_health_probe();
                        let is_ok = if up_clone.is_tls {
                            ping_tls(&up_clone, &probe_query, &tls_conn_clone, sni.as_deref()).await.is_ok()
                        } else {
                            ping_udp(&up_clone, &probe_query).await.is_ok()
                        };

                        if is_ok {
                            up_clone.fails.store(0, Ordering::Relaxed);
                            up_clone.is_healthy.store(true, Ordering::Relaxed);
                        } else {
                            let current_fails = up_clone.fails.fetch_add(1, Ordering::Relaxed) + 1;
                            if current_fails >= fails_limit {
                                if up_clone.is_healthy.swap(false, Ordering::Relaxed) {
                                    tracing::warn!("Upstream {}:{} marked as UNHEALTHY (Failed {} times)", up_clone.ip, up_clone.port, current_fails);
                                }
                            }
                        }
                    }
                });
            }
        }

        Ok(Self {
            upstreams, tls_servername, failover_rcodes, next_rcodes, policy,
            except_domains, force_tcp, max_concurrent, failfast, 
            max_idle_conns: if max_idle_conns == 0 { 1000 } else { max_idle_conns }, 
            expire_duration,
            rr_counter: AtomicUsize::new(0),
            tls_connector,
            error_tx: shared.error_tx.clone(),
        })
    }

    async fn process(&self, msg: &mut DnsMessage) -> Result<DnsMessage> {
        if msg.halt_chain || self.upstreams.is_empty() || msg.raw_query.is_empty() { return Ok(msg.clone()); }

        // 【新增】：在入口处统一提取并解析域名，方便后续全局日志打印
        let qname = extract_qname_string(&msg.raw_query).unwrap_or_else(|| ".".to_string());

        if !self.except_domains.is_empty() {
            for ex in &self.except_domains {
                if qname.ends_with(ex) { 
                    tracing::debug!("Domain '{}' matches except rule {}, skipping forward.", qname, ex);
                    return Ok(msg.clone()); 
                }
            }
        }

        let _permit = if let Some(sema) = &self.max_concurrent {
            match sema.try_acquire() {
                Ok(p) => Some(p),
                Err(_) => {
                    tracing::warn!("Max concurrent queries reached! Rejecting '{}' with REFUSED.", qname);
                    FORWARD_MAX_CONCURRENT_REJECTS.inc();
                    msg.raw_response = Some(build_error_response(&msg.raw_query, 5)); 
                    msg.halt_chain = true;
                    msg.answered_by = "forward".to_string();
                    return Ok(msg.clone());
                }
            }
        } else { None };

        let mut healthy_upstreams = Vec::new();
        for (idx, up) in self.upstreams.iter().enumerate() {
            if up.is_healthy.load(Ordering::Relaxed) { healthy_upstreams.push(idx); }
        }

        if healthy_upstreams.is_empty() {
            if self.failfast {
                tracing::warn!("failfast triggered: all upstreams are unhealthy, returning SERVFAIL for '{}'", qname);
                msg.raw_response = Some(build_error_response(&msg.raw_query, 2)); 
                msg.halt_chain = true;
                msg.answered_by = "forward".to_string();
                return Ok(msg.clone());
            } else {
                healthy_upstreams = (0..self.upstreams.len()).collect(); 
            }
        }

        match self.policy {
            Policy::Sequential => {}
            Policy::Random => { healthy_upstreams.shuffle(&mut rand::thread_rng()); }
            Policy::RoundRobin => {
                if !healthy_upstreams.is_empty() {
                    let start = self.rr_counter.fetch_add(1, Ordering::Relaxed) % healthy_upstreams.len();
                    healthy_upstreams.rotate_left(start); 
                }
            }
        }

        for &idx in &healthy_upstreams {
            let upstream = &self.upstreams[idx];
            let upstream_addr = format!("{}:{}", upstream.ip, upstream.port);
            
            tracing::debug!("TxID: {:#06x} -> Trying {}://{} for '{}' (Policy: {:?})", msg.header.id, if upstream.is_tls {"tls"} else {"udp"}, upstream_addr, qname, self.policy);

            let start_req = std::time::Instant::now();
            let result = if upstream.is_tls || self.force_tcp { 
                self.send_tls_with_pool(upstream, &msg.raw_query).await 
            } else { 
                PROXY_CONN_CACHE_MISSES.with_label_values(&["udp", "forward", &upstream_addr]).inc();
                self.send_udp(upstream, &msg.raw_query).await 
            };

            let duration = start_req.elapsed().as_secs_f64();

            match result {
                Ok(response_bytes) => {
                    let rcode = response_bytes[3] & 0x0F;
                    let rcode_str = rcode_to_str(rcode);
                    
                    PROXY_REQUEST_DURATION.with_label_values(&["forward", rcode_str, &upstream_addr]).observe(duration);
                    
                    if self.failover_rcodes.contains(&rcode) { 
                        // 【改进】：打印重试状态，带上域名和耗时
                        tracing::warn!("Upstream {} returned failover RCODE {} for '{}' in {:.4}s, triggering retry...", upstream_addr, rcode_str, qname, duration);
                        continue; 
                    }

                    msg.raw_response = Some(response_bytes);
                    msg.answered_by = "forward".to_string(); 

                    if self.next_rcodes.contains(&rcode) {
                        // 【改进】：打印转入下一层的日志，带上域名和耗时
                        tracing::info!("Upstream {} returned next RCODE {} for '{}' in {:.4}s, pushing to next tier!", upstream_addr, rcode_str, qname, duration);
                        msg.halt_chain = false; 
                        return Ok(msg.clone());
                    }

                    // 【核心改进】：最直观的成功解析日志，包含域名、上游节点、耗时以及 RCODE
                    tracing::info!("Success resolution for '{}' from {} in {:.4}s, RCODE: {}", qname, upstream_addr, duration, rcode_str);
                    msg.halt_chain = true;
                    return Ok(msg.clone());
                }
                Err(e) => {
                    PROXY_REQUEST_DURATION.with_label_values(&["forward", "SERVFAIL", &upstream_addr]).observe(duration);
                    let err_msg = format!("Failed to connect to {} for '{}': {:?}", upstream_addr, qname, e);
                    let _ = self.error_tx.send(err_msg).await;
                    // 【改进】：打印超时或网络失败，带上域名和耗时
                    tracing::debug!("Upstream {} timeout or failed for '{}' in {:.4}s, trying next...", upstream_addr, qname, duration);
                }
            }
        }
        Ok(msg.clone())
    }
    fn priority(&self) -> u8 { 100 }
}

impl ForwardPlugin {
    async fn send_udp(&self, up: &Upstream, query: &[u8]) -> Result<Vec<u8>> {
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect(format!("{}:{}", up.ip, up.port)).await?;
        socket.send(query).await?;
        let mut buf = vec![0u8; 4096];
        let len = timeout(Duration::from_secs(2), socket.recv(&mut buf)).await??;
        buf.truncate(len);
        Ok(buf)
    }

    async fn send_tls_with_pool(&self, up: &Upstream, query: &[u8]) -> Result<Vec<u8>> {
        let mut pooled_stream = None;
        let now = std::time::Instant::now();
        let upstream_addr = format!("{}:{}", up.ip, up.port);
        
        {
            let mut pool = up.idle_tls_conns.lock().await;
            while let Some(idle) = pool.pop() {
                if idle.expires_at > now {
                    pooled_stream = Some(idle.stream);
                    break;
                }
            }
        }

        let mut tls_stream = match pooled_stream {
            Some(stream) => {
                tracing::debug!("Reusing cached TLS connection for {}", up.ip);
                PROXY_CONN_CACHE_HITS.with_label_values(&["tcp-tls", "forward", &upstream_addr]).inc();
                stream
            },
            None => {
                tracing::debug!("Establishing new TLS connection to {}", up.ip);
                PROXY_CONN_CACHE_MISSES.with_label_values(&["tcp-tls", "forward", &upstream_addr]).inc();
                let domain_str = self.tls_servername.clone().unwrap_or_else(|| up.ip.clone());
                let domain = ServerName::try_from(domain_str.as_str()).map_err(|_| anyhow::anyhow!("Invalid SNI"))?;
                let stream = timeout(Duration::from_secs(2), TcpStream::connect(&upstream_addr)).await??;
                self.tls_connector.connect(domain, stream).await?
            }
        };

        let len = query.len() as u16;
        let mut req = vec![(len >> 8) as u8, (len & 0xFF) as u8];
        req.extend_from_slice(query);
        
        if tls_stream.write_all(&req).await.is_err() { anyhow::bail!("Broken TLS connection pipe"); }

        let mut len_buf = [0u8; 2];
        timeout(Duration::from_secs(2), tls_stream.read_exact(&mut len_buf)).await??;
        let resp_len = ((len_buf[0] as usize) << 8) | (len_buf[1] as usize);

        let mut resp = vec![0u8; resp_len];
        timeout(Duration::from_secs(2), tls_stream.read_exact(&mut resp)).await??;

        {
            let mut pool = up.idle_tls_conns.lock().await;
            if pool.len() < self.max_idle_conns {
                pool.push(IdleConnection {
                    stream: tls_stream,
                    expires_at: std::time::Instant::now() + self.expire_duration,
                });
            }
        }

        Ok(resp)
    }
}

async fn ping_udp(up: &Upstream, query: &[u8]) -> Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.connect(format!("{}:{}", up.ip, up.port)).await?;
    socket.send(query).await?;
    let mut buf = vec![0u8; 512];
    timeout(Duration::from_millis(1500), socket.recv(&mut buf)).await??;
    Ok(())
}

async fn ping_tls(up: &Upstream, query: &[u8], connector: &TlsConnector, sni: Option<&str>) -> Result<()> {
    let domain_str = sni.unwrap_or(&up.ip);
    let domain = ServerName::try_from(domain_str).map_err(|_| anyhow::anyhow!("Invalid SNI"))?;
    let stream = timeout(Duration::from_millis(1500), TcpStream::connect(format!("{}:{}", up.ip, up.port))).await??;
    let mut tls_stream = connector.connect(domain, stream).await?;
    let len = query.len() as u16;
    let mut req = vec![(len >> 8) as u8, (len & 0xFF) as u8];
    req.extend_from_slice(query);
    tls_stream.write_all(&req).await?;
    let mut buf = [0u8; 2];
    timeout(Duration::from_millis(1500), tls_stream.read_exact(&mut buf)).await??;
    Ok(())
}

fn build_health_probe() -> Vec<u8> {
    vec![ 0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01 ]
}

fn build_error_response(query: &[u8], rcode: u8) -> Vec<u8> {
    let mut resp = query.to_vec();
    if resp.len() >= 4 { resp[2] |= 0x80; resp[3] |= rcode & 0x0F; }
    resp
}

fn parse_rcode(s: &str) -> u8 {
    match s.to_uppercase().as_str() {
        "NOERROR" => 0, "FORMERR" => 1, "SERVFAIL" => 2,
        "NXDOMAIN" => 3, "NOTIMP" => 4, "REFUSED" => 5, _ => 2, 
    }
}

fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    if let Some(stripped) = s.strip_suffix("ms") { Ok(Duration::from_millis(stripped.parse()?)) }
    else if let Some(stripped) = s.strip_suffix('s') { Ok(Duration::from_secs(stripped.parse()?)) }
    else if let Some(stripped) = s.strip_suffix('m') { Ok(Duration::from_secs(stripped.parse::<u64>()? * 60)) }
    else { anyhow::bail!("invalid duration") }
}

fn extract_qname_string(query: &[u8]) -> Option<String> {
    if query.len() < 12 { return None; }
    let mut offset = 12;
    let mut parts = Vec::new();
    while offset < query.len() {
        let len = query[offset] as usize;
        offset += 1;
        if len == 0 { break; }
        if offset + len <= query.len() {
            if let Ok(s) = std::str::from_utf8(&query[offset..offset+len]) { parts.push(s.to_string()); }
            offset += len;
        } else { break; }
    }
    if parts.is_empty() { Some(".".to_string()) } else { Some(parts.join(".")) }
}