use crate::plugin::{Plugin, SharedState};
use crate::config::PluginConfig;
use crate::types::DnsMessage;
use anyhow::Result;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use prometheus::{
    Encoder, TextEncoder, IntCounterVec, HistogramVec, GaugeVec,
    register_int_counter_vec, register_histogram_vec, register_gauge_vec,
};
use lazy_static::lazy_static;

lazy_static! {
    pub static ref DNS_REQUESTS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "coredns_dns_requests_total",
        "Counter of DNS requests made per zone, protocol and family.",
        &["family", "proto", "server", "type", "view", "zone"]
    ).unwrap();

    pub static ref DNS_RESPONSES_TOTAL: IntCounterVec = register_int_counter_vec!(
        "coredns_dns_responses_total",
        "Counter of response status codes.",
        &["plugin", "rcode", "server", "view", "zone"]
    ).unwrap();

    pub static ref DNS_REQUEST_DURATION: HistogramVec = register_histogram_vec!(
        "coredns_dns_request_duration_seconds",
        "Histogram of the time (in seconds) each request took per zone.",
        &["server", "view", "zone"],
        vec![0.00025, 0.0005, 0.001, 0.002, 0.004, 0.008, 0.016, 0.032, 0.064, 0.128, 0.256, 0.512, 1.024, 2.048, 4.096, 8.192]
    ).unwrap();

    pub static ref DNS_REQUEST_SIZE: HistogramVec = register_histogram_vec!(
        "coredns_dns_request_size_bytes",
        "Size of the EDNS0 UDP buffer in bytes (64K for TCP) per zone and protocol.",
        &["proto", "server", "view", "zone"],
        vec![0.0, 100.0, 200.0, 300.0, 400.0, 511.0, 1023.0, 2047.0, 4095.0, 8291.0, 16000.0, 32000.0, 48000.0, 64000.0]
    ).unwrap();

    pub static ref DNS_RESPONSE_SIZE: HistogramVec = register_histogram_vec!(
        "coredns_dns_response_size_bytes",
        "Size of the returned response in bytes.",
        &["proto", "server", "view", "zone"],
        vec![0.0, 100.0, 200.0, 300.0, 400.0, 511.0, 1023.0, 2047.0, 4095.0, 8291.0, 16000.0, 32000.0, 48000.0, 64000.0]
    ).unwrap();

    pub static ref CACHE_ENTRIES: GaugeVec = register_gauge_vec!(
        "coredns_cache_entries",
        "The number of elements in the cache.",
        &["server", "type", "view", "zones"]
    ).unwrap();

    pub static ref CACHE_REQUESTS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "coredns_cache_requests_total",
        "The count of cache requests.",
        &["server", "view", "zones"]
    ).unwrap();

    pub static ref CACHE_HITS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "coredns_cache_hits_total",
        "The count of cache hits.",
        &["server", "type", "view", "zones"]
    ).unwrap();

    pub static ref CACHE_MISSES_TOTAL: IntCounterVec = register_int_counter_vec!(
        "coredns_cache_misses_total",
        "The count of cache misses. Deprecated, derive misses from cache hits/requests counters.",
        &["server", "view", "zones"]
    ).unwrap();

    pub static ref PROXY_REQUEST_DURATION: HistogramVec = register_histogram_vec!(
        "coredns_proxy_request_duration_seconds",
        "Histogram of the time each request took.",
        &["proxy_name", "rcode", "to"],
        vec![0.00025, 0.0005, 0.001, 0.002, 0.004, 0.008, 0.016, 0.032, 0.064, 0.128, 0.256, 0.512, 1.024, 2.048, 4.096, 8.192]
    ).unwrap();

    pub static ref PROXY_CONN_CACHE_HITS: IntCounterVec = register_int_counter_vec!(
        "coredns_proxy_conn_cache_hits_total",
        "Counter of connection cache hits per upstream and protocol.",
        &["proto", "proxy_name", "to"]
    ).unwrap();

    pub static ref PROXY_CONN_CACHE_MISSES: IntCounterVec = register_int_counter_vec!(
        "coredns_proxy_conn_cache_misses_total",
        "Counter of connection cache misses per upstream and protocol.",
        &["proto", "proxy_name", "to"]
    ).unwrap();

    pub static ref FORWARD_MAX_CONCURRENT_REJECTS: prometheus::IntCounter = prometheus::register_int_counter!(
        "coredns_forward_max_concurrent_rejects_total",
        "Counter of the number of queries rejected because the concurrent queries were at maximum."
    ).unwrap();

    pub static ref PLUGIN_ENABLED: GaugeVec = register_gauge_vec!(
        "coredns_plugin_enabled",
        "A metric that indicates whether a plugin is enabled on per server and zone basis.",
        &["name", "server", "view", "zone"]
    ).unwrap();

    pub static ref BUILD_INFO: GaugeVec = register_gauge_vec!(
        "coredns_build_info",
        "A metric with a constant '1' value labeled by version, revision, and rust_version from which CoreDNS was built.",
        &["rust_version", "revision", "version"]
    ).unwrap();
    
    pub static ref RELOAD_VERSION_INFO: GaugeVec = register_gauge_vec!(
        "coredns_reload_version_info",
        "Record the hash value during reload.",
        &["hash", "value"]
    ).unwrap();

    pub static ref RELOAD_FAILED_TOTAL: prometheus::IntCounter = prometheus::register_int_counter!(
        "coredns_reload_failed_total",
        "Counter of the number of failed reload attempts."
    ).unwrap();
}

pub struct PrometheusPlugin {
    _handle: tokio::task::JoinHandle<()>,
}

#[async_trait::async_trait]
impl Plugin for PrometheusPlugin {
    fn name(&self) -> &str { "prometheus" }

    fn from_config(config: &PluginConfig, _shared: Arc<SharedState>) -> Result<Self> {
        let mut port = config.args.first().cloned().unwrap_or_else(|| ":9153".to_string());
        if !port.contains(':') { port = format!(":{}", port); }
        let addr = format!("0.0.0.0{}", port);
        
        let pkg_version = env!("CARGO_PKG_VERSION");
        BUILD_INFO.with_label_values(&["rustc", "rust-rewrite", pkg_version]).set(1.0);

        let handle = tokio::spawn(async move {
            match tokio::net::TcpListener::bind(&addr).await {
                Ok(listener) => {
                    tracing::info!("[prometheus] Successfully bound metrics listener on {}", addr);
                    
                    while let Ok((mut stream, _)) = listener.accept().await {
                        tokio::spawn(async move {
                            // 1. 【核心修复】：扩大缓冲区到 8KB，确保一口气吞下所有浏览器的冗长请求头
                            let mut buf = [0u8; 8192]; 
                            
                            if let Ok(Ok(n)) = tokio::time::timeout(std::time::Duration::from_secs(2), stream.read(&mut buf)).await {
                                if n > 0 && buf.starts_with(b"GET ") {
                                    use prometheus::Encoder;
                                    let encoder = prometheus::TextEncoder::new();
                                    let metric_families = prometheus::gather();
                                    let mut buffer = vec![];
                                    
                                    if encoder.encode(&metric_families, &mut buffer).is_ok() {
                                        let header = format!(
                                            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                            buffer.len()
                                        );
                                        
                                        let mut response = header.into_bytes();
                                        response.extend_from_slice(&buffer);
                                        
                                        // 2. 超时保护写回数据，并确保发送队列清空
                                        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), stream.write_all(&response)).await;
                                        let _ = stream.flush().await;
                                        
                                        // 3. 【极其关键】：优雅关闭 TCP 的发送端 (发送 FIN 包)
                                        // 这等于明确告诉浏览器："我的数据发完了，你可以安心渲染了"，彻底杜绝 RST 报错！
                                        let _ = stream.shutdown().await;
                                    }
                                }
                            }
                        });
                    }
                }
                Err(_) => {
                    tracing::info!("[prometheus] Port {} is already active (shared with another zone).", addr);
                }
            }
        });
        
        Ok(Self { _handle: handle })
    }

    async fn process(&self, msg: &mut DnsMessage) -> Result<DnsMessage> {
        let req_size = msg.raw_query.len() as f64;
        let server_label = format!("dns://:{}", get_port_from_msg(msg));
        let qtype = get_qtype_str(&msg.raw_query);
        
        let family = match msg.client_addr {
            Some(std::net::SocketAddr::V6(_)) => "2",
            _ => "1",
        };

        DNS_REQUESTS_TOTAL.with_label_values(&[family, "udp", &server_label, qtype, "", "."]).inc();
        DNS_REQUEST_SIZE.with_label_values(&["udp", &server_label, "", "."]).observe(req_size);

        let plugins = ["cache", "errors", "forward", "log", "prometheus"];
        for p in plugins {
            PLUGIN_ENABLED.with_label_values(&[p, &server_label, "", "."]).set(1.0);
        }

        msg.start_time = Some(std::time::Instant::now());
        Ok(msg.clone())
    }
    
    async fn post_process(&self, msg: &mut DnsMessage) -> Result<()> {
        let server_label = format!("dns://:{}", get_port_from_msg(msg));

        if let Some(start) = msg.start_time {
            let duration = start.elapsed().as_secs_f64();
            DNS_REQUEST_DURATION.with_label_values(&[&server_label, "", "."]).observe(duration);
        }

        if let Some(resp) = &msg.raw_response {
            let resp_size = resp.len() as f64;
            DNS_RESPONSE_SIZE.with_label_values(&["udp", &server_label, "", "."]).observe(resp_size);
            
            let rcode = resp[3] & 0x0F;
            let rcode_str = rcode_to_str(rcode);
            let plugin_name = if msg.answered_by.is_empty() { "unknown" } else { &msg.answered_by };
            
            DNS_RESPONSES_TOTAL.with_label_values(&[plugin_name, rcode_str, &server_label, "", "."]).inc();
        }
        
        Ok(())
    }

    fn priority(&self) -> u8 { 150 }
}

impl Drop for PrometheusPlugin {
    fn drop(&mut self) { self._handle.abort(); }
}

pub fn rcode_to_str(rcode: u8) -> &'static str {
    match rcode { 0 => "NOERROR", 1 => "FORMERR", 2 => "SERVFAIL", 3 => "NXDOMAIN", 4 => "NOTIMP", 5 => "REFUSED", _ => "UNKNOWN" }
}

fn get_port_from_msg(msg: &DnsMessage) -> u16 {
    msg.server_port.unwrap_or(53)
}

fn get_qtype_str(query: &[u8]) -> &'static str {
    if query.len() < 12 { return "UNKNOWN"; }
    let mut offset = 12;
    while offset < query.len() {
        let len = query[offset] as usize;
        if len == 0 { offset += 1; break; }
        offset += len + 1;
    }
    if offset + 2 <= query.len() {
        let qtype = ((query[offset] as u16) << 8) | (query[offset + 1] as u16);
        match qtype {
            1 => "A", 28 => "AAAA", 33 => "SRV", 5 => "CNAME", 15 => "MX", 16 => "TXT", 2 => "NS", 6 => "SOA", 12 => "PTR", 255 => "ANY", _ => "OTHER"
        }
    } else { "UNKNOWN" }
}