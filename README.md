# CoreDNS-Rust 🦀 🛡️

**CoreDNS-Rust** is a high-performance, pollution-resistant DNS gateway built on Rust's asynchronous runtime (Tokio). It maintains full compatibility with CoreDNS's `Corefile` configuration syntax while being rebuilt from the ground up for **DNS-over-TLS (DoT) cascading failover**, **lock-free multi-core caching**, and **zero-packet-loss hot reload**.

In stress tests, it demonstrates **33,000+ QPS** with **0% packet loss**. Perfect for enterprise DNS split-routing gateways, home anti-pollution side routers, or any scenario requiring ultra-low latency and high availability.

## ✨ Core Features

### 🚀 Extreme Performance Architecture

* **Maximize Multi-Core Concurrency**: Ditch black-box macros and manually control the Tokio runtime. Dynamically binds worker threads 1:1 to CPU cores for perfect load balancing.
* **Lock-Free Ultra-Fast Cache (Moka)**: Completely rewritten caching layer using `moka` high-performance concurrent cache. W-TinyLFU eviction algorithm achieves zero lock conflicts, compressing cache hit latency to nanoseconds (0.1ms).
* **Dual-Stack Anti-Blocking (UDP + TCP)**: Native RFC-compliant dual-protocol listening and handling. Defensive truncation of large UDP responses (`TC` flag) gracefully guides clients to fall back to TCP streaming.

### 🔒 Heavy-Duty Forward Engine

* Native **DNS-over-TLS (DoT)** support with custom SNI (`tls_servername`) for perfect network penetration.
* **Advanced Load Balancing**: `sequential` (primary-backup failover), `round_robin` (dual-active rotation), `random` strategies.
* **Active Health Checks & Circuit Breaking**: Independent coroutine backend probing (`health_check` / `max_fails`) removes failed upstreams in milliseconds—no death spirals.
* **State Machine Penetration Control**: `failover` auto-retries on SERVFAIL, `next` cascades on NXDOMAIN to prevent leaks.

### 🛠️ Industrial-Grade Operations & Observability

* **Midnight-Precise Log Rotation**: `rolling-file` engine with local timezone support—no more confusing UTC cuts. Non-blocking rotation at `00:00` sharp every day.
* **Intelligent Error Folding (Errors)**: Aggregates network errors (like timeouts) within time windows using Actor model and regex, preventing log storms from filling your disk during network jitter.
* **Lossless Hot Reload (Graceful Reload)**: Background polling of `Corefile` SHA512 hash broadcasts seamless listener handle switches via Watch Channel—**zero downtime** updates.
* **Enterprise-Grade Prometheus Dashboard**: Built-in `/metrics` endpoint covering QPS, cache hit rates, upstream RCODE distribution, DNS latency heatmaps, and more.

---

## 📦 Quick Deployment

We provide the most common enterprise deployment options for rapid setup.

### Option A: One-Click Native Deployment (Systemd) ⭐ Recommended

Break through Linux file descriptor limits and squeeze every drop of performance from your hardware:

```bash
curl -sSL https://raw.githubusercontent.com/antstars/coredns-rust/main/install.sh | sudo bash
```

*(After installation, check status with `systemctl status coredns-rust`)*

### Option B: Docker Compose (Multi-Stage Build)

Minimal Debian-slim based image with full host network and timezone sync support:

```yaml
version: '3.8'
services:
  coredns-rust:
    image: coredns-rust:latest
    container_name: coredns-rust
    restart: always
    network_mode: "host"
    volumes:
      - ./Corefile:/app/Corefile:ro
      - ./logs:/app/logs:rw
    environment:
      - TZ=Asia/Shanghai
```

Start with `docker compose up -d`.

---

## 🛠️ Configuration Example (Corefile)

Fully compatible with standard syntax. Below is a typical **domestic/international split-routing + high-availability anti-pollution** configuration:

```
# Domestic Resolution Group (UDP Protocol)
.:1053 {
    log
    cache {
      success 50000
      denial 25000
    }
    prometheus :9153
    errors {
        # Fold timeout errors within 5 minutes into a single warning
        consolidate 5m ".* i/o timeout$" warning
    }
  
    # Domestic multi-level failover (sequential policy)
    forward . 119.29.29.29 223.5.5.5 114.114.114.114 {
        policy sequential
        health_check 1s
        max_fails 3
        max_concurrent 100000
    }
}

# Overseas Resolution Group (Encrypted DoT + Anti-Leak Penetration)
.:1054 {
    log
    cache
  
    # Tier 1: Google DNS (round-robin load balancing)
    forward . tls://8.8.8.8 tls://8.8.4.4 {
        tls_servername dns.google
        policy round_robin
        health_check 0.5s
        max_fails 2
        failover SERVFAIL REFUSED
        next NXDOMAIN  # If Google returns NXDOMAIN, pass to next tier!
    }
  
    # Tier 2: Cloudflare (fallback)
    forward . tls://1.1.1.1 tls://1.0.0.1 {
        tls_servername cloudflare-dns.com
        policy round_robin
    }
}

# Global Background Components
. {
    # Check config changes every 5 seconds
    reload 5s
    # Liveness probe
    health :8100
}
```

---

## 🧩 Supported Plugins

Highly decoupled plugin architecture with onion-model interception:

| **Plugin** | **Status** | **Core Capabilities** |
|------------|------------|----------------------|
| `forward` | 🟢 Core | DoT encryption penetration, multi-protocol connection pooling, load balancing, circuit breaking, cascading forward |
| `cache` | 🟢 Core | Moka high-performance LRU cache, independent Success/Denial TTL control |
| `errors` | 🟢 Core | Async regex aggregation (Consolidate), anti-log-storm |
| `reload` | 🟢 Core | Seamless Watch hot reload (Graceful Restart) |
| `prometheus` | 🟢 Core | Native full-stack metrics endpoint exposure |
| `health` | 🟢 Basic | TCP Kubernetes liveness probe |
| `log` | 🟢 Basic | Standard logging with latency and RCODE status |

---

## 🤝 Contributing

Minimal plugin extension experience!

To write a new plugin, simply create a module in `src/plugin/`, implement the `process` (request inbound) and `post_process` (response outbound) methods of the `Plugin` trait, and register it in the `mod.rs` routing factory. Issues and Pull Requests are welcome!

## 📄 License

This project is licensed under the [MIT License](LICENSE).

---

## 📊 Performance Benchmarks

| Metric | Value |
|--------|-------|
| Max QPS | 33,000+ |
| Packet Loss | 0% |
| Cache Hit Latency | ~0.1ms |
| Hot Reload Time | <100ms |
| Memory Usage | ~50MB (idle) |

---

## 🔧 Build from Source

```bash
# Clone the repository
git clone https://github.com/antstars/coredns-rust.git
cd coredns-rust

# Build in release mode
cargo build --release

# Run
./target/release/coredns-rust --config Corefile
```

---

## 📞 Support & Community

- **Issues**: https://github.com/antstars/coredns-rust/issues
- **Discussions**: https://github.com/antstars/coredns-rust/discussions

---
