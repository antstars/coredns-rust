//! CoreDNS Rust - A DNS server written in Rust

pub mod config;
pub mod dns_server;
pub mod plugin;
pub mod types;

use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use tracing::info;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use chrono::Local;
use rolling_file::{RollingConditionBasic, RollingFileAppender};

// 自定义本地时间格式化器，解决日志默认输出 UTC 时间的问题
struct LocalTimer;
impl fmt::time::FormatTime for LocalTimer {
    fn format_time(&self, w: &mut fmt::format::Writer<'_>) -> std::fmt::Result {
        write!(w, "{}", Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z"))
    }
}

#[derive(Parser, Debug)]
#[command(name = "coredns-rust")]
#[command(about = "A DNS server written in Rust", long_about = None)]
struct Args {
    #[arg(short, long, default_value = "Corefile")]
    config: String,

    #[arg(long, default_value = "0.0.0.0:53")]
    address: String,
}

// 【硬核改造】：去掉了 #[tokio::main] 宏，改为手动配置多核引擎
fn main() -> Result<()> {
    // 1. 动态嗅探系统真实的 CPU 核心数 (支持 8核、10核甚至 128核 EPYC)
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4); // 兜底：如果获取失败，默认分配 4 个线程

    // 2. 手动构建并深度定制 Tokio 多线程运行时
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(cores) // 核心参数：1:1 绑定 CPU 核心数量
        .thread_name("coredns-worker") // 为线程命名，方便在 htop/top 中排查负载均衡情况
        .enable_all()
        .build()?;

    // 3. 将主网关逻辑塞入定制的引擎中全速运行
    runtime.block_on(async_main(cores))
}

// 这是原本的主逻辑，现在被我们装进了手动构建的 runtime 里
async fn async_main(cores: usize) -> Result<()> {
    // 确保日志目录存在
    std::fs::create_dir_all("logs").unwrap_or_default();
    
    // 完美支持本地时区 00:00 准时切割的日志轮转器
    let file_appender = RollingFileAppender::new(
        "logs/coredns.log",
        RollingConditionBasic::new().daily(),
        30, // 仅保留最近 30 天的历史日志
    )?;
    
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(fmt::layer().with_writer(non_blocking).with_ansi(false).with_timer(LocalTimer))
        .with(fmt::layer().with_writer(std::io::stdout).with_timer(LocalTimer))
        .init();

    let args = Args::parse();
    info!("Starting CoreDNS Rust version {}", env!("CARGO_PKG_VERSION"));
    
    // 明确把多核优化的状态打印到日志里，让你对服务器的算力了如指掌
    info!(">>> Multi-core optimization enabled: utilizing {} independent worker threads", cores);

    let abs_path = std::fs::canonicalize(&args.config)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| args.config.clone());
    info!(">>> Locked configuration absolute path: {}", abs_path);

    // Moka LRU 缓存池初始化在此处，使得热重载时能够无损继承原有的 DNS 解析缓存
    let cache_preserve = Arc::new(plugin::cache::CacheStore::new());

    // 核心热重载事件循环
    loop {
        info!("--- Starting/Reloading CoreDNS configuration ---");
        let shared = Arc::new(plugin::SharedState::new_with_cache(cache_preserve.clone(), abs_path.clone()));
        let cfg = config::Config::load(&abs_path, shared.clone())?;

        for zone_config in &cfg.zones {
            info!("Zone: {} loaded with {} root plugins", zone_config.name, zone_config.plugins.len());
        }

        let server = dns_server::DnsServer::new(cfg, shared.clone())?;
        
        let reload_rx = shared.reload_rx.lock().unwrap().take().unwrap();

        let is_reload = server.run(args.address.clone(), reload_rx).await?;
        
        if !is_reload {
            break; 
        }
        
        info!("Hot reload triggered, rebuilding server instances...");
    }

    Ok(())
}