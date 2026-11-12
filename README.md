# Rustino

Rustino is a Tauri 2 desktop Arduino workbench. The dark editor workspace includes a file explorer, C++ syntax colors, static Arduino completions, board and serial-port selection, streamed compiler output, library search/install, and a serial monitor. Rust handles Arduino CLI, file access, settings, and serial process control. The Linux app bundle includes the Ollama runtime and Qwen2.5-Coder 1.5B model for private, on-device AI chat; users do not need to install Ollama or download a model separately.

## Run

Install Rust, Node.js, GTK 3, WebKitGTK 4.1, and Zenity. Then run from the repository root:

```sh
cargo run --release
```

## Windows release

The package name is `rustino`, so the Windows application executable is `rustino.exe`. The Windows Tauri profile brands the NSIS installer as **Rustino by Sabarees**, with publisher **Sabarees · godem.in**. It bundles the Windows Arduino CLI, CPU Ollama runtime, Qwen model, and offline WebView2 installer. ESP32 cores are installed from Rustino's Board Manager for the selected board.

Build the installer on a Windows x64 machine with Rust's MSVC toolchain, the Tauri CLI, and NSIS installed:

```powershell
./scripts/build-windows.ps1
```

The setup executable and `rustino.exe` are written under `target/release/bundle/nsis/` and `target/release/` respectively. Windows installers must be built and smoke-tested on Windows; this Linux workspace cannot produce or run the NSIS artifact.

Tauri serves the checked-in `ui/` frontend directly; no npm install or network access is needed. Rustino uses an installed Arduino CLI, or the Arduino CLI bundled with the official Arduino IDE when found. Install the ESP32 core from Board settings or with:

```sh
arduino-cli core update-index
arduino-cli core install esp32:esp32
```

Open or create an `.ino` sketch, select its board profile and serial port, then Verify or Upload. Upload releases Serial Monitor before invoking Arduino CLI. Settings are stored in `~/.config/rustino/settings.json` or `$XDG_CONFIG_HOME/rustino/settings.json`.

## Scope

This is an actively developing IDE, not full Arduino IDE 2 parity. Completions are static; clangd/LSP, board pin validation, serial plotting, and debugging are not integrated yet. Builds and uploads use the local Arduino CLI, installed board cores, and device permissions. The bundled AI runtime and model increase the Linux installer size by about 1 GB and currently target x86_64 Linux.
