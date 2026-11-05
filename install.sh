#!/usr/bin/env bash
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="${HOME}/.local/bin"
SYSTEMD_USER_DIR="${HOME}/.config/systemd/user"
APPS_DIR="${HOME}/.local/share/applications"
WP_CONF_DIR="${HOME}/.config/wireplumber/wireplumber.conf.d"

PERIOD="${1:-128}"
RATE="${2:-48000}"
CARD="${3:-Gen}"

echo "=== WireAudio Installer ==="
echo "1. Building release binaries..."
cargo build --manifest-path "${REPO_DIR}/Cargo.toml" --workspace --release

echo "2. Installing binaries to ${BIN_DIR}..."
mkdir -p "${BIN_DIR}"
install -Dm755 "${REPO_DIR}/target/release/wireaudio" "${BIN_DIR}/wireaudio"
install -Dm755 "${REPO_DIR}/target/release/wireaudio-engine" "${BIN_DIR}/wireaudio-engine"
install -Dm755 "${REPO_DIR}/target/release/libasound_module_pcm_wireaudio.so" "${BIN_DIR}/libasound_module_pcm_wireaudio.so"

echo "3. Installing desktop launcher to ${APPS_DIR}..."
mkdir -p "${APPS_DIR}"
sed "s|^Exec=.*|Exec=${BIN_DIR}/wireaudio|" "${REPO_DIR}/packaging/wireaudio.desktop" > "${APPS_DIR}/wireaudio.desktop"
chmod 644 "${APPS_DIR}/wireaudio.desktop"
if command -v kbuildsycoca6 >/dev/null 2>&1; then
    kbuildsycoca6 >/dev/null 2>&1 || true
fi

echo "4. Disabling PipeWire / PulseAudio audio daemons (preserving pure ALSA)..."
mkdir -p "${WP_CONF_DIR}"
cat > "${WP_CONF_DIR}/99-wireaudio-alsa-only.conf" << 'EOF'
# Managed by WireAudio Installer: prevent WirePlumber/PipeWire from opening ALSA soundcards
wireplumber.profiles = {
  main = {
    monitor.alsa = disabled
    monitor.alsa-midi = disabled
  }
}
EOF

AUDIO_UNITS="pipewire-pulse.socket pipewire-pulse.service wireplumber.service pipewire.socket pipewire.service"
systemctl --user stop ${AUDIO_UNITS} 2>/dev/null || true
systemctl --user mask ${AUDIO_UNITS} 2>/dev/null || true

echo "5. Registering WireAudio ALSA profile (period=${PERIOD}, rate=${RATE}, card=${CARD})..."
"${BIN_DIR}/wireaudio-engine" register --period "${PERIOD}" --rate "${RATE}" --card "${CARD}"

echo "6. Installing and enabling systemd user daemon (wireaudio.service)..."
mkdir -p "${SYSTEMD_USER_DIR}"
install -Dm644 "${REPO_DIR}/packaging/wireaudio.service" "${SYSTEMD_USER_DIR}/wireaudio.service"
systemctl --user daemon-reload
systemctl --user enable --now wireaudio.service

echo ""
echo "=== WireAudio Installed Successfully! ==="
echo "• GUI App:       launch 'WireAudio Studio Setup' from KDE menu or run 'wireaudio'"
echo "• Daemon Status: systemctl --user status wireaudio.service"
echo "• ALSA Config:   ~/.asoundrc (default + wireaudio + wireaudio_in1 + wireaudio_in2 + wireaudio_direct)"
echo "• Rollback:      run 'just uninstall' or './uninstall.sh' anytime to restore PipeWire"
