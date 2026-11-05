# Root Justfile for WireAudio (https://github.com/ansromanov/wireaudio)

set dotenv-load := false

#List available recipes
default:
    @just --list

# Install WireAudio app, systemd user daemon, desktop launcher, and disable PipeWire audio daemons
install period="128" rate="48000" card="Gen":
    bash ./install.sh {{period}} {{rate}} {{card}}

# Uninstall WireAudio and restore PipeWire / WirePlumber audio daemons
uninstall:
    bash ./uninstall.sh

# Build the entire workspace in release mode
build:
    cargo build --workspace --release

# Inspect all connected ALSA soundcards and current buffer/latency status
status:
    cargo run -p wireaudio-engine --release -- status

# Register `wireaudio` and `wireaudio_direct` PCM devices in ~/.asoundrc (default: 32 frames @ 48000 Hz)
register period="32" rate="48000" card="Gen":
    cargo run -p wireaudio-engine --release -- register --period {{period}} --rate {{rate}} --card {{card}}

# Register Aggregate / Split I/O (e.g., Input on Scarlett `Gen`, Output on internal `Generic`)
register-split in_card="Gen" out_card="Generic" period="32" rate="48000":
    cargo run -p wireaudio-engine --release -- register --period {{period}} --rate {{rate}} --card {{in_card}} --out-card {{out_card}}

# Remove the managed WireAudio block from ~/.asoundrc without touching the rest of the system
unregister:
    cargo run -p wireaudio-engine --release -- unregister

# Install the native ALSA External PCM plugin (`libasound_module_pcm_wireaudio.so`) into /usr/lib/alsa-lib/
install-plugin:
    cargo build -p wireaudio-pcm-plugin --release
    sudo install -Dm755 target/release/libasound_module_pcm_wireaudio.so /usr/lib/alsa-lib/libasound_module_pcm_wireaudio.so

# Run the real-time ALSA routing engine daemon in the terminal
run-engine period="32" rate="48000" card="Gen":
    cargo run -p wireaudio-engine --release -- run --period {{period}} --rate {{rate}} --card {{card}}

# Launch the WireAudio Desktop Control Center UI (macOS Audio MIDI Setup style)
run-ui:
    cargo run -p wireaudio-ui --release

# Full quickstart: build release, register 32-frame ALSA PCM device, and launch the UI
all: build register run-ui

# Run formatting and Clippy lint checks
lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings

# Run all workspace unit tests
test:
    cargo test --workspace

# Clean build artifacts
clean:
    cargo clean
