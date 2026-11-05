#!/usr/bin/env bash
set -euo pipefail

BIN_DIR="${HOME}/.local/bin"
SYSTEMD_USER_DIR="${HOME}/.config/systemd/user"
APPS_DIR="${HOME}/.local/share/applications"
WP_CONF_FILE="${HOME}/.config/wireplumber/wireplumber.conf.d/99-wireaudio-alsa-only.conf"

echo "=== WireAudio Uninstaller & Audio Stack Restore ==="

echo "1. Stopping and disabling wireaudio.service..."
systemctl --user disable --now wireaudio.service 2>/dev/null || true
rm -f "${SYSTEMD_USER_DIR}/wireaudio.service"

echo "2. Removing WireAudio block from ~/.asoundrc..."
if [ -x "${BIN_DIR}/wireaudio-engine" ]; then
    "${BIN_DIR}/wireaudio-engine" unregister || true
fi

echo "3. Restoring PipeWire, WirePlumber, and pipewire-pulse daemons..."
rm -f "${WP_CONF_FILE}"
systemctl --user unmask pipewire-pulse.socket pipewire-pulse.service wireplumber.service pipewire.socket pipewire.service 2>/dev/null || true
systemctl --user daemon-reload
systemctl --user enable --now pipewire.socket pipewire-pulse.socket wireplumber.service 2>/dev/null || true

echo "4. Removing desktop entry and binaries..."
rm -f "${APPS_DIR}/wireaudio.desktop"
rm -f "${BIN_DIR}/wireaudio" "${BIN_DIR}/wireaudio-engine" "${BIN_DIR}/libasound_module_pcm_wireaudio.so"
if command -v kbuildsycoca6 >/dev/null 2>&1; then
    kbuildsycoca6 >/dev/null 2>&1 || true
fi

echo "=== WireAudio uninstalled and default PipeWire stack restored. ==="
