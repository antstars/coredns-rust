# ==========================================
# 第一阶段：编译构建阶段 (Builder)
# 使用包含完整 Rust 工具链的官方镜像
# ==========================================
FROM rust:bookworm AS builder
WORKDIR /usr/src/coredns-rust

# 复制 Cargo 配置文件和源代码
COPY Cargo.toml ./
COPY src ./src

# 编译极限优化 Release 版本
RUN cargo build --release

# ==========================================
# 第二阶段：生产运行阶段 (Runner)
# 使用极简的 debian-slim 镜像，极大减小最终镜像体积
# ==========================================
FROM debian:bookworm-slim
WORKDIR /app

# 1. 安装 CA 证书（用于上游 TLS 请求验证）
# 2. 安装 tzdata（极其关键！为了让你的 chrono::Local 每天 00:00 准时切割日志）
RUN apt-get update && \
    apt-get install -y tzdata ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# 设置容器默认时区为东八区 (Asia/Shanghai)
ENV TZ=Asia/Shanghai

# 创建日志存放目录
RUN mkdir -p /app/logs

# 从 builder 阶段把编译好的二进制文件捞过来
COPY --from=builder /usr/src/coredns-rust/target/release/coredns-rust /app/coredns-rust

# 暴露服务端口 (与你 Corefile 里的配置对应)
# 1053/1054: DNS 服务
# 8100: 健康检查探针
# 9153: Prometheus 监控大盘
EXPOSE 1053/udp 1053/tcp
EXPOSE 1054/udp 1054/tcp
EXPOSE 8100/tcp
EXPOSE 9153/tcp

# 启动容器时默认执行的命令
ENTRYPOINT ["/app/coredns-rust", "--config", "/app/Corefile"]