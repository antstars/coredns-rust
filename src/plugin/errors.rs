use crate::plugin::{Plugin, SharedState};
use crate::config::PluginConfig;
use crate::types::DnsMessage;
use anyhow::Result;
use regex::Regex;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::Duration;

#[derive(Clone)]
struct Rule {
    pattern: Regex,
    raw_pattern: String,
    duration: Duration,
    level: String,
    show_first: bool,
}

pub struct ErrorsPlugin {
    _handle: Option<tokio::task::JoinHandle<()>>,
}

#[async_trait::async_trait]
impl Plugin for ErrorsPlugin {
    fn name(&self) -> &str { "errors" }

    fn from_config(config: &PluginConfig, shared: Arc<SharedState>) -> Result<Self> {
        let mut rules = Vec::new();
        
        for sub in &config.block {
            if sub.name == "consolidate" {
                if sub.args.len() < 2 { continue; }
                let dur = parse_duration(&sub.args[0]).unwrap_or(Duration::from_secs(30));
                let raw_pattern = sub.args[1].clone();
                let pattern = Regex::new(&raw_pattern).unwrap_or_else(|_| Regex::new(".*").unwrap());
                
                let mut level = "error".to_string();
                let mut show_first = false;

                for arg in sub.args.iter().skip(2) {
                    if arg == "show_first" { show_first = true; } 
                    else { level = arg.clone(); }
                }
                rules.push(Rule { pattern, raw_pattern, duration: dur, level, show_first });
            }
        }

        let mut _handle = None;
        if let Ok(mut lock) = shared.error_rx.lock() {
            if let Some(mut rx) = lock.take() {
                let rules_clone = rules.clone();
                
                _handle = Some(tokio::spawn(async move {
                    let mut counts = vec![0u32; rules_clone.len()];
                    let (timeout_tx, mut timeout_rx) = mpsc::channel::<usize>(100);

                    loop {
                        tokio::select! {
                            Some(err) = rx.recv() => {
                                let mut matched = false;
                                for (i, rule) in rules_clone.iter().enumerate() {
                                    if rule.pattern.is_match(&err) {
                                        matched = true;
                                        counts[i] += 1;
                                        
                                        if counts[i] == 1 {
                                            if rule.show_first { log_msg(&rule.level, &err); }
                                            let tx = timeout_tx.clone();
                                            let dur = rule.duration;
                                            tokio::spawn(async move {
                                                tokio::time::sleep(dur).await;
                                                let _ = tx.send(i).await;
                                            });
                                        }
                                        break; 
                                    }
                                }
                                if !matched { tracing::error!("{}", err); }
                            }
                            Some(idx) = timeout_rx.recv() => {
                                let count = counts[idx];
                                let rule = &rules_clone[idx];
                                if count > 1 || (count == 1 && !rule.show_first) {
                                    let msg = format!("{} errors like '{}' occurred in last {:?}", count, rule.raw_pattern, rule.duration);
                                    log_msg(&rule.level, &msg);
                                }
                                counts[idx] = 0;
                            }
                        }
                    }
                }));
            }
        }

        tracing::info!("[errors] Plugin initialized with {} consolidate rules", rules.len());
        Ok(Self { _handle })
    }

    async fn process(&self, msg: &mut DnsMessage) -> Result<DnsMessage> { Ok(msg.clone()) }
    fn priority(&self) -> u8 { 220 }
}

impl Drop for ErrorsPlugin {
    fn drop(&mut self) {
        if let Some(handle) = &self._handle {
            handle.abort();
        }
    }
}

fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    if let Some(stripped) = s.strip_suffix("ms") { Ok(Duration::from_millis(stripped.parse()?)) }
    else if let Some(stripped) = s.strip_suffix('s') { Ok(Duration::from_secs(stripped.parse()?)) }
    else if let Some(stripped) = s.strip_suffix('m') { Ok(Duration::from_secs(stripped.parse::<u64>()? * 60)) }
    else if let Some(stripped) = s.strip_suffix('h') { Ok(Duration::from_secs(stripped.parse::<u64>()? * 3600)) }
    else { anyhow::bail!("invalid duration") }
}

fn log_msg(level: &str, msg: &str) {
    match level.to_lowercase().as_str() {
        "warning" | "warn" => tracing::warn!("{}", msg),
        "info" => tracing::info!("{}", msg),
        "debug" => tracing::debug!("{}", msg),
        _ => tracing::error!("{}", msg),
    }
}