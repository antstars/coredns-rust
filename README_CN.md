# CoreDNS-Rust 🦀 🛡️

**[🇨🇳 中文版](README_CN.md)** | **[🇺🇸 English](README.md)**

---

**CoreDNS-Rust** 是一个基于 Rust 异步运行时 (Tokio) 深度定制的高性能、防污染 DNS 网关。它高度兼容官方 CoreDNS 的 `Corefile` 配置语法，但在底层专为 **DNS-over-TLS (DoT) 级联容灾**、**多核无锁缓存**以及 **0 丢包热重载** 进行了硬核重构。

在单机压测中，它展现出了 **33,000+ QPS** 且 **0% 丢包率** 的极致并发承载力。适合用作企业级 DNS 分流网关、家庭防污染旁路由，或任何需要极低延迟与高可用性的网络场景。

## ✨ 核心特性

### 🚀 极致的性能架构

* **榨干多核并发**：摒弃黑盒宏，手动接管 Tokio 运行时，根据系统 CPU 核心数 1:1 动态绑定工作线程 (Worker Threads)，实现完美的负载均衡。
* **无锁极速缓存 (Moka)**：彻底重写缓存层，接入 `moka` 高性能并发缓存。利用 W-TinyLFU 淘汰算法实现 0 锁冲突，将缓存命中延迟压缩至纳秒级 (0.1ms)。
* **双栈防阻断 (UDP + TCP)**：原生实现 RFC 规范的双协议监听与处理。具备 UDP 响应大包防御性截断 (`TC` flag) 能力，完美引导客户端降级为 TCP 流式请求。

### 🔒 重火力 Forward 引擎

* 原生支持 **DNS-over-TLS (DoT)** 及自定义 SNI (`tls_servername`)，完美穿透网络阻断。
* **高级负载均衡**：支持 `sequential` (主备容灾)、`round_robin` (双活轮询)、`random` 策略。
* **主动健康检查与熔断**：独立协程后台探活 (`health_check` / `max_fails`)，毫秒级剔除宕机上游，绝生死磕。
* **状态机穿透控制**：`failover` 自动重试 SERVFAIL，`next` 自动下沉 NXDOMAIN 防止漏网之鱼。

### 🛠️ 工业级运维与可观测性

* **午夜精准日志切割**：采用 `rolling-file` 引擎结合本地时区 (Local TimeZone)，抛弃反人类的 UTC 切割，每天 `00:00` 准时无阻塞轮转日志。
* **智能错误折叠 (Errors)**：通过 Actor 模型与正则表达式，在时间窗口内聚合底层网络错误日志（如 Timeout），防止网络抖动时的日志风暴写满磁盘。
* **无损热重载 (Graceful Reload)**：后台抖动轮询 `Corefile` 的 SHA512 哈希，变更时通过 Watch Channel 一对多广播无缝切换监听器句柄，实现 **0 停机** 热更新。
* **企业级 Prometheus 大盘**：内置 `/metrics` 端点，全面覆盖 QPS、缓存拦截率、上游 RCODE 分布、DNS 延迟热力图等核心指标。

---

## 📦 快速部署

我们提供了最主流的企业级部署方案，极速上线。

### 方案 A：一键原生部署 (Systemd) ⭐ 推荐

突破 Linux 系统文件描述符限制，榨干物理机极限性能：

```bash
curl -sSL https://raw.githubusercontent.com/antstars/coredns-rust/main/install.sh | sudo bash
```

*(安装后可使用 `systemctl status coredns-rust` 查看运行状态)*

### 方案 B：Docker Compose (多阶段构建)

基于 Debian-slim 的极简镜像，完美支持 Host 网络与时区同步：

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

执行 `docker compose up -d` 即可启动。

### 方案 C：源码编译

```bash
# 克隆仓库
git clone https://github.com/antstars/coredns-rust.git
cd coredns-rust

# Release 模式编译
cargo build --release

# 运行
./target/release/coredns-rust --config Corefile
```

---

## 🛠️ 配置示例 (Corefile)

高度兼容标准语法。以下是一个典型的**国内外分流 + 高可用防污染**配置方案：

```
# 国内解析组 (UDP 协议)
.:1053 {
    log
    cache {
      success 50000
      denial 25000
    }
    prometheus :9153
    errors {
        # 将 5 分钟内的超时报错折叠为一条警告
        consolidate 5m ".* i/o timeout$" warning
    }
  
    # 国内多级容灾（主备策略）
    forward . 119.29.29.29 223.5.5.5 114.114.114.114 {
        policy sequential
        health_check 1s
        max_fails 3
        max_concurrent 100000
    }
}

# 海外解析组 (加密 DoT + 防漏报穿透)
.:1054 {
    log
    cache
  
    # 第一梯队：Google DNS (轮询负载均衡)
    forward . tls://8.8.8.8 tls://8.8.4.4 {
        tls_servername dns.google
        policy round_robin
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
    # 每 5 秒检查一次配置变更
    reload 5s
    # 存活探针
    health :8100
}
```

### 配置选项参考

| 选项 | 说明 | 默认值 | 示例 |
|------|------|--------|------|
| `policy` | 负载均衡策略 | `random` | `sequential`, `round_robin`, `random` |
| `health_check` | 健康检查间隔 | `500ms` | `1s`, `500ms`, `2m` |
| `max_fails` | 标记为不健康的失败次数 | `2` | `1-10` |
| `max_concurrent` | 最大并发查询数 | 无限制 | `100000` |
| `tls_servername` | DoT 的 SNI | 上游 IP | `dns.google` |
| `failover` | 触发故障转移的 RCODE | 无 | `SERVFAIL REFUSED` |
| `next` | 转入下一梯队的 RCODE | 无 | `NXDOMAIN` |
| `except` | 排除的域名 | 全部 | `internal.local` |
| `force_tcp` | 强制使用 TCP | `false` | `true` |

---

## 🧩 已支持的插件列表

插件体系采用高度解耦设计，洋葱模型拦截：

| **插件名称** | **状态** | **核心能力** |
|--------------|----------|--------------|
| `forward` | 🟢 核心 | DoT 加密穿透，多协议连接池，负载均衡，熔断探活，穿透转发 |
| `cache` | 🟢 核心 | Moka 高性能 LRU 缓存，独立管控 Success/Denial TTL |
| `errors` | 🟢 核心 | 异步正则聚合 (Consolidate)，防日志风暴 |
| `reload` | 🟢 核心 | 无缝 Watch 热更新 (Graceful Restart) |
| `prometheus` | 🟢 核心 | 原生全栈 Metrics 监控端点暴露 |
| `health` | 🟢 基础 | TCP Kubernetes 存活探针探测 |
| `log` | 🟢 基础 | 耗时与 RCODE 状态标准日志记录 |

---

## 📊 性能基准测试

| 指标 | 数值 | 测试环境 |
|------|------|----------|
| 最大 QPS | 33,000+ | 8 核，16GB 内存 |
| 丢包率 | 0% | 满载压力测试 |
| 缓存命中延迟 | ~0.1ms | LRU 缓存 |
| 热重载时间 | <100ms | 配置变更 |
| 内存占用 | ~50MB | 空闲状态 |
| 内存占用 | ~200MB | 负载状态 |

---

## 🤝 贡献与二次开发

极简的插件扩展体验！

若需编写新插件，只需在 `src/plugin/` 目录下新建模块，实现 `Plugin` trait 中的 `process` (请求去程) 和 `post_process` (响应回程) 方法，并在 `mod.rs` 路由工厂中注册即可。欢迎提交 Issue 和 Pull Request！

### 开发环境搭建

```bash
# 克隆仓库
git clone https://github.com/antstars/coredns-rust.git
cd coredns-rust

# Debug 模式编译
cargo build

# 运行测试
cargo test

# 使用自定义配置运行
cargo run -- --config Corefile
```

---

## 🔧 故障排查

### 常见问题

**问题："Address already in use"**
```bash
# 检查 53 端口是否被占用
sudo lsof -i :53
# 停止冲突的服务 (如 systemd-resolved)
sudo systemctl stop systemd-resolved
```

**问题："Too many open files"**
```bash
# 增加文件描述符限制
ulimit -n 65535
# 或编辑 /etc/security/limits.conf
```

**问题：53 端口权限拒绝**
```bash
# 使用 capabilities 替代 root 权限
sudo setcap 'cap_net_bind_service=+ep' ./target/release/coredns-rust
# 或使用 >1024 端口并用 iptables 转发
```

---

## 📞 支持与社区

- **Issues**: https://github.com/antstars/coredns-rust/issues
- **Discussions**: https://github.com/antstars/coredns-rust/discussions

---

## 📄 许可证

本项目基于 [MIT License](LICENSE) 许可开源。

---

## 🙏 致谢

- [CoreDNS](https://coredns.io/) - 原始 DNS 服务器
- [Tokio](https://tokio.rs/) - Rust 异步运行时
- [Moka](https://github.com/moka-rs/moka) - 高性能缓存库

---
