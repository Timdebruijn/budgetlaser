#!/bin/sh
# Install or update BudgetLaser as a cage kiosk service on Raspberry Pi OS Lite (trixie).
# Run as root on the Pi:  sudo deploy/install.sh ./budgetlaser [my-config.toml]
# Idempotent. An existing /etc/budgetlaser/budgetlaser.toml is only replaced when a
# config file is passed. Enabling the service stops the login prompt on tty1.
set -eu

BIN=${1:?usage: install.sh path/to/budgetlaser [config.toml]}
CONF=${2:-}
HERE=$(cd "$(dirname "$0")" && pwd)

apt-get update
apt-get install -y --no-install-recommends cage mesa-vulkan-drivers

# Normal (non-system) user: logind gives it a seat session with GPU access.
# dialout is for the DMX USB Pro serial port.
if ! id budgetlaser >/dev/null 2>&1; then
    useradd --create-home --shell /usr/sbin/nologin budgetlaser
fi
usermod -aG dialout budgetlaser

install -Dm755 "$BIN" /usr/local/bin/budgetlaser
if [ -n "$CONF" ]; then
    install -Dm644 "$CONF" /etc/budgetlaser/budgetlaser.toml
elif [ ! -f /etc/budgetlaser/budgetlaser.toml ]; then
    install -Dm644 "$HERE/../budgetlaser.toml" /etc/budgetlaser/budgetlaser.toml
fi
install -Dm644 "$HERE/pam.d/budgetlaser" /etc/pam.d/budgetlaser
install -Dm644 "$HERE/budgetlaser.service" /etc/systemd/system/budgetlaser.service

systemctl daemon-reload
systemctl enable budgetlaser.service
systemctl restart budgetlaser.service
echo "BudgetLaser running. Logs: journalctl -u budgetlaser -f"
