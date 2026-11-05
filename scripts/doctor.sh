#!/usr/bin/env bash
# Health check for an installed WireAudio setup. Exit code = number of failed checks.
set -uo pipefail

BIN_DIR="${HOME}/.local/bin"
CONFIG="${HOME}/.config/wireaudio/config.json"
WP_CONF="${HOME}/.config/wireplumber/wireplumber.conf.d/99-wireaudio-alsa-only.conf"
FAILS=0

ok()   { printf '  \e[32m✔\e[0m %s\n' "$1"; }
warn() { printf '  \e[33m!\e[0m %s\n' "$1"; }
fail() { printf '  \e[31m✘\e[0m %s\n' "$1"; FAILS=$((FAILS + 1)); }
check() { if eval "$2" >/dev/null 2>&1; then ok "$1"; else fail "$1${3:+ — $3}"; fi; }

echo "Install"
check "wireaudio-engine in ${BIN_DIR}" "[ -x '${BIN_DIR}/wireaudio-engine' ]" "run: just install"
check "wireaudio UI in ${BIN_DIR}" "[ -x '${BIN_DIR}/wireaudio' ]" "run: just install"
check "config ${CONFIG}" "[ -f '${CONFIG}' ]"

echo "Service"
check "wireaudio.service enabled" "systemctl --user is-enabled wireaudio.service"
check "wireaudio.service active" "systemctl --user is-active wireaudio.service" "journalctl --user -u wireaudio -e"

echo "PipeWire"
check "pipewire-pulse masked" "[ \"\$(systemctl --user is-enabled pipewire-pulse.socket)\" = masked ]"
check "WirePlumber ALSA monitor disabled" "grep -q 'monitor.alsa = disabled' '${WP_CONF}'"
for unit in pipewire wireplumber; do
    if systemctl --user is-active "${unit}" >/dev/null 2>&1; then
        ok "${unit} active (video / screen sharing)"
    else
        warn "${unit} inactive — Wayland screen sharing will not work"
    fi
done
if command -v wpctl >/dev/null 2>&1 && systemctl --user is-active wireplumber >/dev/null 2>&1; then
    held=$(wpctl status 2>/dev/null | sed -n '/^Audio/,/^Video/p' | grep -cE '^\s*│\s+\*?\s*[0-9]+\.' || true)
    if [ "${held}" -eq 0 ]; then ok "PipeWire holds no audio devices"; else fail "PipeWire holds ${held} audio device(s)"; fi
fi

echo "ALSA"
check "~/.asoundrc has WireAudio block" "grep -q 'WireAudio Managed Block' '${HOME}/.asoundrc'" "run: just register"
check "ALSA default is WireAudio" "aplay -L | grep -A1 '^default$' | grep -q WireAudio"
card=$(sed -n 's/.*"selected_card_id": *"\([^"]*\)".*/\1/p' "${CONFIG}" 2>/dev/null)
if [ -n "${card}" ]; then
    check "configured card '${card}' present" "aplay -l | grep -q 'card [0-9]*: ${card} '" "is the interface plugged in?"
fi
# Plays 1 s of silence: proves default opens (not EBUSY) without making noise.
timeout 1 aplay -q -D default -f S32_LE -r 48000 -c 2 /dev/zero >/dev/null 2>&1
rc=$?
if [ "${rc}" -eq 124 ]; then ok "default PCM opens and plays"; else fail "default PCM failed to play (exit ${rc})"; fi

echo "Realtime"
rt=$(ulimit -r)
if [ "${rt}" = unlimited ] || [ "${rt}" -ge 80 ]; then ok "RT priority limit ${rt}"; else warn "RT priority limit ${rt} (<80) — RT monitor runs without SCHED_FIFO"; fi

echo
if [ "${FAILS}" -eq 0 ]; then echo "All checks passed."; else echo "${FAILS} check(s) failed."; fi
exit "${FAILS}"
