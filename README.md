# CoreDNS-Rust 🦀 🛡️

**CoreDNS-Rust** 是一个基于 Rust 异步运行时 (Tokio) 编写的高性能、防污染 DNS 网关。它高度兼容官方 CoreDNS 的 `Corefile` 配置语法，但在底层专为 **复杂的 DNS-over-TLS (DoT) 级联容灾** 、**智能错误折叠**以及**0 丢包热重载**进行了硬核重构。

适合用作企业级 DNS 分流网关、家庭防污染旁路由 DNS，或任何需要极致性能与高可用性的 DNS 场景。

## ✨ 核心特性

* 🚀  **纯异步架构** ：基于 `tokio` 构建，轻松应对极高并发请求，内存占用极低。
* 🔒  **重火力 Forward 引擎** ：
  * 原生支持 **DNS-over-TLS (DoT)** 及自定义 SNI (`tls_servername`)，完美穿透网络阻断。
  * **高级负载均衡** ：支持 `sequential` (主备容灾)、`round_robin` (双活轮询)、`random` 策略。
  * **主动健康检查与熔断** (`health_check` / `max_fails`)：毫秒级剔除宕机节点，绝生死磕。
  * **状态机穿透控制** ：`failover` 自动重试 SERVFAIL，`next` 自动下沉 NXDOMAIN 防止漏网之鱼。
  * **连接池复用** (`max_idle_conns`)：复用 TLS 握手通道，大幅降低高频查询延迟。
* 🧠  **智能错误折叠 (Errors)** ：通过 Actor 模型与正则表达式，在时间窗口内聚合相同的底层网络错误日志（如超时），防止网络抖动时的日志风暴写满磁盘。
* ⚡  **极速内存缓存 (Cache)** ：洋葱模型实现的 LRU 并发安全缓存，分别独立控制 Success 和 Denial (NXDOMAIN/SERVFAIL) 的 TTL。
* 🔄  **无损热重载 (Graceful Reload)** ：后台抖动轮询 `Corefile` 的 SHA512 哈希，文件变更时瞬间无缝切换监听器句柄，实现 **0 丢包、0 停机** 热更新。
* 📊  **原生 Prometheus 监控** ：内置 `/metrics` 端点，提供 QPS、缓存命中率、上游 RCODE 分布、重载状态等企业级大盘指标。

## 📦 快速开始

### 编译与安装

确保你的系统已安装 [Rust 工具链](https://rustup.rs/)：

**Bash**

```
# 克隆仓库
git clone https://github.com/yourusername/coredns-rust.git
cd coredns-rust

# 编译 Release 版本
cargo build --release

# 运行服务器
./target/release/coredns-rust --config Corefile
```

## 🛠️ 配置示例 (Corefile)

CoreDNS-Rust 兼容标准的 `Corefile` 语法。以下是一个典型的**国内外分流 + 高可用防污染**配置方案：

**Plaintext**

```
# 国内解析组 (UDP)
.:1053 {
    log
    cache 3600
    prometheus 0.0.0.0:9153
    errors {
        # 将 30 秒内的超时报错折叠为一条警告
        consolidate 30s "^Failed to .+" warning
    }
  
    # 国内多级容灾（顺序策略）
    forward . 119.29.29.29 223.5.5.5 114.114.114.114 {
        policy sequential
        health_check 1s
        max_fails 3
    }
}

# 海外解析组 (加密 DoT + 防漏报穿透)
.:1054 {
    log
    cache {
        success 10000 3600
        denial 5000 1800
    }
  
    # 第一梯队：Google DNS (轮询负载均衡)
    forward . tls://8.8.8.8 tls://8.8.4.4 {
        tls_servername dns.google
        policy round_robin
        max_idle_conns 1000
        health_check 0.5s
        max_fails 2
        failover SERVFAIL REFUSED
        next NXDOMAIN  # 如果 Google 返回不存在，丢给下一梯队继续查！
    }
  
    # 第二梯队：Cloudflare (兜底)
    forward . tls://1.1.1.1 tls://1.0.0.1 {
        tls_servername cloudflare-dns.com
        policy round_robin
    }
}

# 全局后台组件
. {
    # 每 10 秒检查一次配置文件变更，并添加 5 秒随机抖动防止集群并发重启风暴
    reload 10s 5s
    health :8100
}
```

## 🧩 已支持的插件列表

目前项目中采用模块化设计，每个插件独立解耦，已实现以下核心插件：

| **插件名称** | **状态** | **描述**                                     |
| ------------------ | -------------- | -------------------------------------------------- |
| `forward`        | 🟢 核心        | 高级转发引擎 (DoT, 并发控制, 熔断, 连接池, Policy) |
| `cache`          | 🟢 核心        | 高性能读写锁缓存 (基于洋葱模型拦截)                |
| `errors`         | 🟢 核心        | 异步正则聚合日志管理器 (Consolidate)               |
| `reload`         | 🟢 核心        | SHA512 文件系统监控与热重载 (Graceful Restart)     |
| `prometheus`     | 🟢 核心        | 标准的 Metrics 监控端点暴露                        |
| `whoami`         | 🟢 基础        | 回显客户端 IP 和端口 (纯字节流构建)                |
| `health`         | 🟢 基础        | 健康检查端点 (`/health`readiness probe)          |
| `log`            | 🟢 基础        | 标准查询访问日志记录                               |

## 📈 监控大盘

配置 `prometheus` 插件后，可使用 Prometheus Server 抓取 `http://127.0.0.1:9153/metrics`。

支持的查询维度示例：

* 缓存命中率：`coredns_cache_hits_total{type="success"}`
* 上游节点超时频率：`coredns_forward_responses_total{rcode="SERVFAIL"}`
* 动态重载状态：`coredns_reload_version_info`

## 🤝 贡献与开发

欢迎提交 Issue 和 Pull Request！

如果要扩展新插件，只需在 `src/plugin/` 目录下新建模块，实现 `Plugin` trait 的 `process` (去程拦截) 和 `post_process` (回程缓存) 方法，并在 `mod.rs` 中注册即可。

## 📄 许可证

本项目基于 [MIT License](https://www.google.com/search?q=LICENSE&authuser=2) 许可开源。

---
