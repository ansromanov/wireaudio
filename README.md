# AudioWireCraft

**CoreAudio-style low-latency ALSA studio engine, multi-client shared audio bus, and desktop control center for Linux.**

`AudioWireCraft` brings the plug-and-play simplicity of **macOS Audio MIDI Setup** and **Focusrite Control** to Linux ALSA — allowing professional DAWs (**REAPER**, **[SoundCraft](https://github.com/storytold/soundcraft)**, **TONE3000** / neural amp modelers) to run at **32-frame buffer sizes (`0.67 ms` @ 48 kHz)** while seamlessly sharing the audio interface with desktop and browser audio.

---

## Key Highlights

- **32-Frame Ultra-Low Latency**: Configures USB and PCI studio interfaces (such as the **Focusrite Scarlett 2i2 4th Gen**) with `S32_LE` (24/32-bit), `125 us` USB high-speed framing, and `SCHED_FIFO` real-time threads.
- **CoreAudio-Style Shared Bus**: No more choosing between your DAW and your browser/voice chat. Multiple clients (DAWs, standalone guitar DSPs, and desktop audio) share the interface simultaneously without exclusive lockouts.
- **Zero-Disruption ALSA Registration**: Registers `audiowirecraft` and `audiowirecraft_direct` PCM devices in `~/.asoundrc` (with `snd_device_name_hint` descriptions) and builds `libasound_module_pcm_audiowirecraft.so` so REAPER and `cpal`-based apps like SoundCraft detect the engine out of the box without uninstalling system packages.
- **macOS Audio MIDI Setup UI**: Built with `egui`/`eframe` for instant inspection of all connected interfaces, input/output channel strips, hardware mixer controls (`snd_ctl`), sample rate (`44.1–192 kHz`), and buffer size (`16–512 frames`).

---

## Workspace Architecture

| Crate | Description |
| :--- | :--- |
| [`crates/awc-core`](crates/awc-core) | Core types, lock-free shared-memory multi-client audio bus (`/dev/shm/audiowirecraft_bus_v1`), `S32_LE` ↔ `f32` sample conversion, and latency math. |
| [`crates/awc-alsa`](crates/awc-alsa) | Direct ALSA hardware discovery (`/proc/asound` + `libasound`), `SCHED_FIFO` real-time audio loop, hardware mixer control inspector (`snd_ctl`), and non-destructive `~/.asoundrc` management. |
| [`crates/awc-pcm-plugin`](crates/awc-pcm-plugin) | Native ALSA External PCM plugin (`libasound_module_pcm_audiowirecraft.so`) for zero-copy client connectivity. |
| [`crates/awc-ui`](crates/awc-ui) | Native desktop GUI modeled after macOS **Audio MIDI Setup**. |

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the complete system design, dataflow diagrams, and real-time scheduling specification.

---

## Quick Start

```bash
# Build all workspace crates and the native UI
cargo build --workspace --release

# Run tests
cargo test --workspace

# Launch the AudioWireCraft Studio Setup UI
cargo run --release -p awc-ui
```

## License

Licensed under either of [Apache License, Version 2.0](http://www.apache.org/licenses/LICENSE-2.0) or [MIT license](http://opensource.org/licenses/MIT) at your option.
