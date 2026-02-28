#!/bin/bash

# ============================================================================
# CoreDNS Rust - 极速二进制安装脚本 (基于 GitHub Releases)
# ============================================================================

set -e # 遇到错误立即退出

# ==============================
# 变量配置区
# ==============================
REPO="antstarse/coredns-rust"
APP_NAME="coredns-rust"
BIN_PATH="/usr/local/bin/${APP_NAME}"
CONF_DIR="/etc/${APP_NAME}"
LOG_DIR="/var/log/${APP_NAME}"
TMP_DIR="/tmp/${APP_NAME}-install"

# 打印日志函数
info()  { echo -e "\033[1;32m[INFO]\033[0m $1"; }
warn()  { echo -e "\033[1;33m[WARN]\033[0m $1"; }
error() { echo -e "\033[1;31m[ERROR]\033[0m $1"; exit 1; }

# 1. 检查是否为 Root 权限
if [ "$EUID" -ne 0 ]; then
  error "请使用 sudo 或 root 权限运行此安装脚本！"
fi

# 2. 依赖检查 (只需要 curl 和 tar)
if ! command -v curl &> /dev/null || ! command -v tar &> /dev/null; then
    error "未检测到 curl 或 tar，请先安装它们 (例如: apt install curl tar)。"
fi

# 3. 自动探测系统与架构
info "正在探测系统环境..."
OS="$(uname -s)"
ARCH="$(uname -m)"

if [ "$OS" = "Linux" ]; then
    TARGET_OS="linux"
elif [ "$OS" = "Darwin" ]; then
    TARGET_OS="apple"
else
    error "不支持的操作系统: $OS"
fi

if [ "$ARCH" = "x86_64" ]; then
    TARGET_ARCH="x86_64"
elif [ "$ARCH" = "aarch64" ] || [ "$ARCH" = "arm64" ]; then
    TARGET_ARCH="aarch64"
else
    error "不支持的 CPU 架构: $ARCH (当前仅支持 x86_64 和 aarch64/arm64)"
fi

info "检测到环境: $TARGET_OS ($TARGET_ARCH)"

# 4. 获取最新版本号 (完美绕过 GitHub API 限制的黑魔法)
info "正在向 GitHub 获取最新版本信息..."

# 通过追踪 /releases/latest 的 302 重定向 URL 来提取版本号
LATEST_TAG=$(curl -Ls -o /dev/null -w %{url_effective} "https://github.com/${REPO}/releases/latest" | awk -F '/' '{print $NF}')

if [ -z "$LATEST_TAG" ] || [ "$LATEST_TAG" = "latest" ]; then
    error "获取最新版本失败！请检查网络，或确认 ${REPO} 仓库是否已设为公开 (Public)。"
fi
info "发现最新版本: ${LATEST_TAG}"

# 5. 下载并解压
FILENAME="${APP_NAME}-${TARGET_OS}-${TARGET_ARCH}.tar.gz"
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${LATEST_TAG}/${FILENAME}"

info "正在下载: $DOWNLOAD_URL"
rm -rf "$TMP_DIR" && mkdir -p "$TMP_DIR"
cd "$TMP_DIR"

# 下载文件 (带进度条)
curl -L -o "$FILENAME" "$DOWNLOAD_URL"

# 解压并安装到系统目录
info "正在解压并安装..."
tar -xzf "$FILENAME"
cp "$APP_NAME" "$BIN_PATH"
chmod 755 "$BIN_PATH"

# 6. 设置配置目录与用户
info "正在配置运行环境..."
mkdir -p "$CONF_DIR"
mkdir -p "$LOG_DIR"

if ! id "coredns" &>/dev/null; then
    useradd -r -M -s /bin/false coredns
fi

# 从你的 GitHub 仓库主分支直接拉取默认配置兜底
if [ ! -f "$CONF_DIR/Corefile" ]; then
    info "未找到本地配置，正在拉取默认 Corefile..."
    if ! curl -sSL "https://raw.githubusercontent.com/${REPO}/main/Corefile" -o "$CONF_DIR/Corefile"; then
        warn "拉取 Corefile 失败，将创建极其基础的兜底配置..."
        echo ".:53 { forward . 8.8.8.8 }" > "$CONF_DIR/Corefile"
    fi
fi

chown -R coredns:coredns "$CONF_DIR"
chown -R coredns:coredns "$LOG_DIR"

# 7. 生成并启动 Systemd 守护进程
info "正在注册 systemd 服务..."
cat <<EOF > /etc/systemd/system/${APP_NAME}.service
[Unit]
Description=CoreDNS Rust - High Performance DNS Server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=coredns
Group=coredns
WorkingDirectory=${LOG_DIR}
ExecStart=${BIN_PATH} --config ${CONF_DIR}/Corefile

LimitNOFILE=1048576
LimitNPROC=1048576
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE

Restart=always
RestartSec=3s
TimeoutStopSec=10s

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable --now ${APP_NAME}.service

# 8. 检查状态
sleep 2
if systemctl is-active --quiet ${APP_NAME}.service; then
    info "=========================================================="
    info "🎉 安装成功！${APP_NAME} (${LATEST_TAG}) 已全速启动！"
    info "查看运行状态: systemctl status ${APP_NAME}"
    info "查看实时日志: journalctl -u ${APP_NAME} -f"
    info "修改配置文件: nano ${CONF_DIR}/Corefile"
    info "=========================================================="
else
    error "服务启动异常，请运行 'journalctl -u ${APP_NAME} -n 50' 检查原因。"
fi

# 清理临时目录
rm -rf "$TMP_DIR"