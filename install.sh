#!/usr/bin/env bash

# ============================================================================
# CoreDNS Rust - 高性能 DNS 网关一键安装脚本
# ============================================================================

set -e # 遇到错误立即退出

# ==============================
# 变量配置区 (请根据实际情况修改 REPO_URL)
# ==============================
REPO_URL="https://github.com/antstars/coredns-rust.git"
APP_NAME="coredns-rust"
BIN_PATH="/usr/local/bin/${APP_NAME}"
CONF_DIR="/etc/${APP_NAME}"
LOG_DIR="/var/log/${APP_NAME}"
WORK_DIR="/tmp/${APP_NAME}-build"

# 打印带颜色的日志
info()  { echo -e "\033[1;32m[INFO]\033[0m $1"; }
warn()  { echo -e "\033[1;33m[WARN]\033[0m $1"; }
error() { echo -e "\033[1;31m[ERROR]\033[0m $1"; exit 1; }

# 1. 检查是否为 Root 权限
if [ "$EUID" -ne 0 ]; then
  error "请使用 sudo 或 root 权限运行此安装脚本！"
fi

# 2. 检查依赖项 (Git & Rust)
info "正在检查系统依赖..."
if ! command -v git &> /dev/null; then
    error "未检测到 git，请先安装 git (例如: apt install git 或 yum install git)。"
fi

if ! command -v cargo &> /dev/null; then
    warn "未检测到 Rust 工具链，正在尝试为当前用户临时加载环境变量..."
    if [ -f "$HOME/.cargo/env" ]; then
        source "$HOME/.cargo/env"
    elif [ -f "/root/.cargo/env" ]; then
        source "/root/.cargo/env"
    else
        error "未找到 cargo。请先运行 'curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh' 安装 Rust。"
    fi
fi

# 3. 拉取源码
info "正在拉取最新源码..."
rm -rf "$WORK_DIR"
git clone "$REPO_URL" "$WORK_DIR"
cd "$WORK_DIR"

# 4. 编译极限性能版
info "正在使用 Cargo 编译 Release 版本 (这可能需要几分钟)..."
cargo build --release

# 5. 设置系统目录与用户
info "正在配置系统目录与运行权限..."
mkdir -p "$CONF_DIR"
mkdir -p "$LOG_DIR"

# 如果用户不存在，则创建专用的无登录权限系统用户
if ! id "coredns" &>/dev/null; then
    useradd -r -M -s /bin/false coredns
fi

# 拷贝二进制文件和配置文件
cp target/release/${APP_NAME} "$BIN_PATH"
# 如果仓库中有 Corefile 则拷贝，否则创建一个基本的兜底配置
if [ -f "Corefile" ]; then
    cp Corefile "$CONF_DIR/Corefile"
else
    warn "源码中未找到 Corefile，创建默认配置..."
    echo ".:53 { forward . 8.8.8.8 }" > "$CONF_DIR/Corefile"
fi

# 赋予目录权限
chown -R coredns:coredns "$CONF_DIR"
chown -R coredns:coredns "$LOG_DIR"
chmod 755 "$BIN_PATH"

# 6. 生成 Systemd 守护进程文件
info "正在生成 systemd 服务文件..."
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

# 性能与系统上限突破
LimitNOFILE=1048576
LimitNPROC=1048576
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE

# 容灾机制
Restart=always
RestartSec=3s
TimeoutStopSec=10s

[Install]
WantedBy=multi-user.target
EOF

# 7. 启动服务
info "重新加载 systemd 并启动 ${APP_NAME}..."
systemctl daemon-reload
systemctl enable ${APP_NAME}.service
systemctl restart ${APP_NAME}.service

# 8. 检查状态
sleep 2
if systemctl is-active --quiet ${APP_NAME}.service; then
    info "=========================================================="
    info "安装成功！${APP_NAME} 已在后台全速运行 🚀"
    info "查看运行状态: systemctl status ${APP_NAME}"
    info "查看实时日志: journalctl -u ${APP_NAME} -f"
    info "配置文件路径: ${CONF_DIR}/Corefile"
    info "=========================================================="
else
    error "服务启动失败，请使用 'journalctl -u ${APP_NAME} -n 50' 查看错误日志。"
fi

# 清理编译目录
rm -rf "$WORK_DIR"