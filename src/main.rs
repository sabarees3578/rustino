use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

#[path = "turbo_builder.rs"]
mod turbo_builder;

const STARTER: &str = "// Rustino sketch\n// Select your ESP32 board, then Verify or Upload.\n\n#ifndef LED_BUILTIN\n#define LED_BUILTIN 2  // Common built-in LED pin on ESP32 Dev Module boards.\n#endif\n\nvoid setup() {\n  pinMode(LED_BUILTIN, OUTPUT);\n}\n\nvoid loop() {\n  digitalWrite(LED_BUILTIN, HIGH);\n  delay(500);\n  digitalWrite(LED_BUILTIN, LOW);\n  delay(500);\n}\n";
const ESP32_CORE: &str = "esp32:esp32";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Preferences {
    cli_path: String,
    fqbn: String,
    port: String,
    sketch_path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppSnapshot {
    preferences: Preferences,
    content: String,
    files: Vec<String>,
    startup_message: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileDocument {
    path: String,
    content: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CliResult {
    success: bool,
    summary: String,
    output: String,
    duration_seconds: f32,
    detected_fqbn: Option<String>,
    selected_port: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryItem {
    name: String,
    version: String,
    sentence: String,
}

#[derive(Clone, Default)]
struct AppState {
    serial: Arc<Mutex<Option<SerialSession>>>,
}

static BUILD_MUTEX: Mutex<()> = Mutex::new(());

struct SerialSession {
    sender: Sender<SerialMessage>,
    done: Receiver<()>,
}

enum SerialMessage {
    Write(String),
    Stop,
}

struct SerialConfig {
    cli_path: String,
    port: String,
    fqbn: String,
    baud: u32,
}

struct SerialWorkerChannels {
    receiver: Receiver<SerialMessage>,
    done: Sender<()>,
    ready: Sender<Result<(), String>>,
    state: Arc<Mutex<Option<SerialSession>>>,
}

#[tauri::command]
fn get_app_state(app: AppHandle) -> AppSnapshot {
    let mut preferences = load_preferences();
    if preferences.cli_path.trim().is_empty() {
        preferences.cli_path = discover_cli(&app);
    }
    if preferences.fqbn.trim().is_empty() {
        preferences.fqbn = "esp32:esp32:esp32".to_owned();
    }
    let mut startup_message = None;
    #[cfg(windows)]
    if preferences.sketch_path.trim().is_empty() {
        match create_first_run_sketch(&app) {
            Ok(path) => {
                preferences.sketch_path = path.to_string_lossy().into_owned();
                startup_message = Some(format!(
                    "Created a starter sketch at {}.",
                    preferences.sketch_path
                ));
                if let Err(error) = save_preferences(preferences.clone()) {
                    startup_message = Some(format!(
                        "Starter sketch is open, but settings could not be saved: {error}"
                    ));
                }
            }
            Err(error) => {
                startup_message = Some(format!(
                    "Could not create the starter sketch automatically: {error}. Use File → New Sketch."
                ));
            }
        }
    }
    let content = if preferences.sketch_path.is_empty() {
        startup_message
            .get_or_insert_with(|| "Open an Arduino sketch to begin editing.".to_owned());
        STARTER.to_owned()
    } else {
        match fs::read_to_string(&preferences.sketch_path) {
            Ok(content) => content,
            Err(error) => {
                startup_message = Some(format!(
                    "Could not restore {}: {error}. Open a sketch to continue.",
                    preferences.sketch_path
                ));
                STARTER.to_owned()
            }
        }
    };
    let files = list_project_files_inner(&preferences.sketch_path);
    AppSnapshot {
        preferences,
        content,
        files,
        startup_message,
    }
}

#[tauri::command]
fn save_preferences(preferences: Preferences) -> Result<(), String> {
    let path = settings_path().ok_or("Could not determine the Rustino settings folder")?;
    write_json_atomic(&path, &preferences)
}

#[tauri::command]
fn list_project_files(sketch_path: String) -> Vec<String> {
    list_project_files_inner(&sketch_path)
}

#[tauri::command]
async fn git_status(sketch_path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let directory = Path::new(&sketch_path)
            .parent()
            .ok_or("Sketch has no parent directory")?;
        let output = hidden_command("git")
            .args([
                "-C",
                &directory.to_string_lossy(),
                "status",
                "--short",
                "--branch",
            ])
            .output()
            .map_err(|error| format!("Could not run Git: {error}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
        }
        let status = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Ok(if status.is_empty() {
            "Git reports a clean working tree.".to_owned()
        } else {
            status
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn open_file(path: String) -> Result<FileDocument, String> {
    let content =
        fs::read_to_string(&path).map_err(|error| format!("Could not open {path}: {error}"))?;
    Ok(FileDocument { path, content })
}

#[tauri::command]
fn choose_sketch() -> Result<Option<FileDocument>, String> {
    let Some(path) = choose_path(false)? else {
        return Ok(None);
    };
    if !Path::new(&path)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ino"))
    {
        return Err("Choose an Arduino .ino sketch".to_owned());
    }
    open_file(path).map(Some)
}

#[tauri::command]
fn choose_save_path() -> Result<Option<String>, String> {
    choose_path(true)
}

#[tauri::command]
fn save_file(path: String, content: String) -> Result<(), String> {
    let file = Path::new(&path);
    if !file
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            ["ino", "h", "hpp", "cpp", "c"]
                .iter()
                .any(|allowed| extension.eq_ignore_ascii_case(allowed))
        })
    {
        return Err("Files must end in .ino, .h, .hpp, .cpp, or .c".to_owned());
    }
    let parent = file.parent().ok_or("File path has no parent folder")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    write_file_atomic(file, content.as_bytes())
        .map_err(|error| format!("Could not save {path}: {error}"))
}

fn write_file_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::fs::OpenOptions;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().unwrap_or_default();
    let (temporary, mut output) = loop {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = name.to_os_string();
        temporary_name.push(format!(".rustino-{}-{id}.tmp", std::process::id()));
        let temporary = parent.join(temporary_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };

    let result = (|| {
        output.write_all(bytes)?;
        output.sync_all()?;
        drop(output);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[tauri::command]
fn list_ports() -> Vec<String> {
    available_ports()
}

#[tauri::command]
fn scan_boards(cli_path: String) -> Result<serde_json::Value, String> {
    let output = hidden_command(resolve_cli(&cli_path))
        .args(["board", "list", "--format", "json"])
        .output()
        .map_err(|error| format!("Could not start Arduino CLI: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return Err(if detail.is_empty() {
            "Arduino board scan failed".to_owned()
        } else {
            detail.to_owned()
        });
    }
    serde_json::from_str(&stdout)
        .map_err(|error| format!("Could not parse Arduino CLI board discovery: {error}"))
}

#[tauri::command]
fn open_sketch_folder(sketch_path: String) -> Result<(), String> {
    let folder = Path::new(&sketch_path)
        .parent()
        .ok_or("Sketch has no parent folder")?;
    #[cfg(windows)]
    let mut command = Command::new("explorer.exe");
    #[cfg(not(windows))]
    let mut command = Command::new("xdg-open");
    command
        .arg(folder)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not open sketch folder {}: {error}", folder.display()))
}

#[tauri::command]
async fn compile_sketch(
    app: AppHandle,
    cli_path: String,
    sketch_path: String,
    fqbn: String,
) -> Result<CliResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        run_build(
            &app,
            cli_path,
            sketch_path,
            fqbn,
            String::new(),
            false,
            false,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn upload_sketch(
    app: AppHandle,
    state: State<'_, AppState>,
    cli_path: String,
    sketch_path: String,
    fqbn: String,
    port: String,
) -> Result<CliResult, String> {
    stop_serial_monitor(state.inner())?;
    tauri::async_runtime::spawn_blocking(move || {
        run_build(&app, cli_path, sketch_path, fqbn, port, true, false)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn export_binary(
    app: AppHandle,
    cli_path: String,
    sketch_path: String,
    fqbn: String,
) -> Result<CliResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        run_build(
            &app,
            cli_path,
            sketch_path,
            fqbn,
            String::new(),
            false,
            true,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn install_esp32_core(app: AppHandle, cli_path: String) -> Result<CliResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        run_simple_cli(
            &app,
            &cli_path,
            &["core", "install", ESP32_CORE],
            "Install ESP32 core",
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn search_libraries(cli_path: String, query: String) -> Result<Vec<LibraryItem>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let output = hidden_command(resolve_cli(&cli_path))
            .args(["lib", "search", "--json", "--omit-releases-details", &query])
            .output()
            .map_err(|error| format!("Could not run library search: {error}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
        }
        parse_library_results(&String::from_utf8_lossy(&output.stdout))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn install_library(
    app: AppHandle,
    cli_path: String,
    name: String,
    install_dependencies: bool,
) -> Result<CliResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut args = vec!["lib", "install"];
        if !install_dependencies {
            args.push("--no-deps");
        }
        args.push(&name);
        run_simple_cli(&app, &cli_path, &args, "Install library")
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn local_ai_chat(
    app: AppHandle,
    prompt: String,
    model: String,
    request_id: u64,
) -> Result<String, String> {
    let model = if model.trim().is_empty() {
        "qwen2.5-coder:1.5b".to_owned()
    } else {
        model.trim().to_owned()
    };
    if model != "qwen2.5-coder:1.5b" {
        return Err(
            "Rustino includes Qwen2.5-Coder 1.5B. Other model downloads are disabled.".to_owned(),
        );
    }
    if let Some(response) = quick_chat_response(&prompt) {
        let _ = app.emit(
            "ai-chunk",
            serde_json::json!({"requestId": request_id, "text": response}),
        );
        return Ok(response);
    }
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = resolve_ollama(&app)?;
        ensure_ollama_server(&runtime)?;
        let mut command = hidden_command(&runtime.binary);
        command.env("OLLAMA_MODELS", &runtime.models);
        command.env("OLLAMA_NOPRUNE", "1");
        command.env("OLLAMA_NUM_PARALLEL", "1");
        command.env("OLLAMA_NO_CLOUD", "1");
        command.env("OLLAMA_LIBRARY_PATH", &runtime.libraries);
        let mut child = command
            .args(["run", model.as_str(), prompt.as_str()])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Could not start the local Ollama model: {error}"))?;
        let stdout = child.stdout.take().ok_or("Could not capture AI response")?;
        let stderr = child.stderr.take().ok_or("Could not capture AI errors")?;
        let stderr_reader = thread::spawn(move || {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::BufReader::new(stderr), &mut text);
            text
        });
        let response = collect_text_chunks(stdout, |chunk| {
            let _ = app.emit(
                "ai-chunk",
                serde_json::json!({"requestId": request_id, "text": chunk}),
            );
        })
        .map_err(|error| format!("Could not read local AI response: {error}"))?;
        let status = child.wait().map_err(|error| error.to_string())?;
        let stderr = stderr_reader.join().unwrap_or_default();
        if !status.success() {
            return Err(if stderr.trim().is_empty() {
                "Ollama could not run qwen2.5-coder:1.5b. Check the local model installation."
                    .to_owned()
            } else {
                stderr.trim().to_owned()
            });
        }
        Ok(response.trim().to_owned())
    })
    .await
    .map_err(|error| error.to_string())?
}

fn quick_chat_response(prompt: &str) -> Option<String> {
    let question = prompt
        .rsplit_once("Question:")
        .map(|(_, question)| question.trim())
        .unwrap_or(prompt)
        .to_lowercase();
    let normalized = question
        .chars()
        .map(|ch| if ch.is_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    if matches!(
        normalized.as_str(),
        "hi" | "hii"
            | "hiii"
            | "hello"
            | "hey"
            | "heya"
            | "good morning"
            | "good afternoon"
            | "good evening"
            | "how are you"
    ) {
        return Some(
            "Hi! What would you like help with in Rustino? I can help with your sketch, board setup, compiler errors, or serial output.".to_owned(),
        );
    }

    if ((normalized.contains("did not ask")
        || normalized.contains("didnt ask")
        || normalized.contains("not ask"))
        && normalized.contains("code"))
        || normalized.contains("dont want code")
        || normalized.contains("do not want code")
        || normalized.contains("why did you give code")
    {
        return Some(
            "You're right—sorry. I shouldn't have sent code when you didn't ask for it. I'll answer your question directly and only generate or change code when you ask.".to_owned(),
        );
    }

    if matches!(normalized.as_str(), "thanks" | "thank you" | "thx") {
        return Some("You're welcome. What would you like help with?".to_owned());
    }
    None
}

struct OllamaRuntime {
    binary: PathBuf,
    libraries: PathBuf,
    models: PathBuf,
}

fn resolve_ollama(app: &AppHandle) -> Result<OllamaRuntime, String> {
    #[cfg(windows)]
    let development_root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/ollama-windows");
    #[cfg(not(windows))]
    let development_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/ollama");
    #[cfg(windows)]
    let development_models =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/ollama/models");
    let bundled_roots = app
        .path()
        .resource_dir()
        .ok()
        .map(|path| path.join("ollama"))
        .into_iter()
        .chain(std::iter::once(development_root));
    for root in bundled_roots {
        #[cfg(windows)]
        let runtime = OllamaRuntime {
            binary: root.join("ollama.exe"),
            libraries: root.join("lib/ollama"),
            models: if root.join("models").is_dir() {
                root.join("models")
            } else {
                development_models.clone()
            },
        };
        #[cfg(not(windows))]
        let runtime = OllamaRuntime {
            binary: root.join("usr/bin/ollama"),
            libraries: root.join("usr/lib/ollama"),
            models: root.join("models"),
        };
        let bundled_model = runtime
            .models
            .join("manifests/registry.ollama.ai/library/qwen2.5-coder/1.5b");
        if runtime.binary.is_file()
            && runtime.libraries.is_dir()
            && runtime.models.is_dir()
            && bundled_model.is_file()
        {
            return Ok(runtime);
        }
    }

    Err(
        "Rustino's bundled AI runtime or Qwen model is missing. Reinstall the complete app bundle."
            .to_owned(),
    )
}

fn ensure_ollama_server(runtime: &OllamaRuntime) -> Result<(), String> {
    let address: SocketAddr = "127.0.0.1:11434".parse().expect("static Ollama address");
    let connected = || TcpStream::connect_timeout(&address, Duration::from_millis(150)).is_ok();
    if connected() {
        return Ok(());
    }
    let mut server = hidden_command(&runtime.binary);
    server
        .arg("serve")
        .env("OLLAMA_MODELS", &runtime.models)
        .env("OLLAMA_NOPRUNE", "1")
        .env("OLLAMA_NUM_PARALLEL", "1")
        .env("OLLAMA_NO_CLOUD", "1")
        .env("OLLAMA_LIBRARY_PATH", &runtime.libraries)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = server.spawn().map_err(|error| {
        format!(
            "Could not start Rustino's bundled AI engine at {}: {error}",
            runtime.binary.display()
        )
    })?;
    for _ in 0..150 {
        if connected() {
            return Ok(());
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("Could not check Ollama server: {error}"))?
        {
            return Err(format!("Ollama server exited during startup ({status})."));
        }
        thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    Err("Ollama did not start on 127.0.0.1:11434.".to_owned())
}

#[cfg(test)]
fn builtin_embedded_response(prompt: &str) -> String {
    let question = prompt
        .rsplit_once("Question:")
        .map(|(_, question)| question.trim())
        .unwrap_or(prompt)
        .to_owned();
    let lower = question.to_lowercase();
    let board = prompt
        .lines()
        .find_map(|line| {
            line.split_once("Target board FQBN:")
                .map(|(_, board)| board)
        })
        .map(str::trim)
        .and_then(|value| value.split_whitespace().next())
        .map(|value| value.trim_end_matches('.'))
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown board");

    if lower.contains("blink") || lower.contains("flash the led") {
        let delay = lower
            .split_whitespace()
            .find_map(|word| {
                let digits = word.trim_matches(|ch: char| !ch.is_ascii_digit());
                digits.parse::<u32>().ok().filter(|_| {
                    lower.contains("ms")
                        || lower.contains("millisecond")
                        || lower.contains("second")
                })
            })
            .map(|value| {
                if lower.contains("second") && !lower.contains("ms") {
                    value.saturating_mul(1000)
                } else {
                    value
                }
            })
            .unwrap_or(500);
        if lower.contains("non-blocking")
            || lower.contains("without delay")
            || lower.contains("millis")
        {
            return format!(
                "Non-blocking blink sketch for {board}:\n\n```cpp\nconstexpr uint32_t BLINK_INTERVAL_MS = {delay};\nuint32_t lastToggle = 0;\nbool ledOn = false;\n\nvoid setup() {{\n  pinMode(LED_BUILTIN, OUTPUT);\n}}\n\nvoid loop() {{\n  const uint32_t now = millis();\n  if (now - lastToggle >= BLINK_INTERVAL_MS) {{\n    lastToggle = now;\n    ledOn = !ledOn;\n    digitalWrite(LED_BUILTIN, ledOn ? HIGH : LOW);\n  }}\n}}\n```"
            );
        }
        return format!(
            "Blink sketch for {board}:\n\n```cpp\nvoid setup() {{\n  pinMode(LED_BUILTIN, OUTPUT);\n}}\n\nvoid loop() {{\n  digitalWrite(LED_BUILTIN, HIGH);\n  delay({delay});\n  digitalWrite(LED_BUILTIN, LOW);\n  delay({delay});\n}}\n```"
        );
    }

    if lower.contains("button") {
        return format!(
            "This example reads a button on GPIO 4 and controls the built-in LED for {board}. Wire the button between GPIO 4 and GND.\n\n```cpp\nconst uint8_t BUTTON_PIN = 4;\n\nvoid setup() {{\n  pinMode(BUTTON_PIN, INPUT_PULLUP);\n  pinMode(LED_BUILTIN, OUTPUT);\n}}\n\nvoid loop() {{\n  const bool pressed = digitalRead(BUTTON_PIN) == LOW;\n  digitalWrite(LED_BUILTIN, pressed ? HIGH : LOW);\n}}\n```"
        );
    }

    if lower.contains("sensor") && lower.contains("led") {
        let (pin, voltage_note) = if board.contains("arduino:avr:uno") {
            ("A0", "The Uno analog input is 5V tolerant.")
        } else if board.contains("esp32c3") {
            (
                "0",
                "Use a 3.3V sensor signal; do not connect 5V to an ESP32 GPIO.",
            )
        } else {
            (
                "34",
                "GPIO 34 is an ADC1 input on classic ESP32; use a 3.3V sensor signal.",
            )
        };
        return format!(
            "For {board}, this turns the built-in LED on when the analog sensor reading exceeds 2000. Connect sensor OUT to GPIO {pin} and share GND. {voltage_note}\n\n```cpp\nconstexpr uint8_t SENSOR_PIN = {pin};\nconstexpr int SENSOR_THRESHOLD = 2000;\n\nvoid setup() {{\n  pinMode(LED_BUILTIN, OUTPUT);\n  Serial.begin(115200);\n}}\n\nvoid loop() {{\n  const int reading = analogRead(SENSOR_PIN);\n  digitalWrite(LED_BUILTIN, reading > SENSOR_THRESHOLD ? HIGH : LOW);\n  Serial.println(reading);\n  delay(25);\n}}\n```"
        );
    }

    if lower.contains("serial") || lower.contains("print") {
        return format!(
            "Here is a serial output sketch for {board}. Open Serial Monitor at 115200 baud.\n\n```cpp\nvoid setup() {{\n  Serial.begin(115200);\n}}\n\nvoid loop() {{\n  Serial.println(\"Rustino is running\");\n  delay(1000);\n}}\n```"
        );
    }

    if lower.contains("analog") || lower.contains("sensor") {
        let pin = if board.contains("arduino:avr:uno") {
            "A0"
        } else if board.contains("esp32c3") {
            "0"
        } else {
            "34"
        };
        return format!(
            "Analog sensor example for {board}; connect the sensor output to GPIO {pin} (Arduino analog pin A0 where applicable).\n\n```cpp\nconstexpr uint8_t SENSOR_PIN = {pin};\n\nvoid setup() {{\n  Serial.begin(115200);\n}}\n\nvoid loop() {{\n  const int reading = analogRead(SENSOR_PIN);\n  Serial.println(reading);\n  delay(250);\n}}\n```"
        );
    }

    if lower.contains("explain") {
        let current = prompt
            .split_once("Current sketch (")
            .and_then(|(_, rest)| rest.split_once("```cpp\n"))
            .map(|(_, code)| code.split("\n```").next().unwrap_or(code))
            .unwrap_or("");
        let has_setup = current.contains("setup(");
        let has_loop = current.contains("loop(");
        return format!(
            "Your sketch is targeting {board}. It {} a setup() function{}.",
            if has_setup && has_loop {
                "defines"
            } else {
                "does not clearly define"
            },
            if has_setup && has_loop {
                " and a loop() function"
            } else {
                " and loop()"
            }
        );
    }

    if lower.contains("error") || lower.contains("failed") || lower.contains("guru meditation") {
        let diagnostics = prompt
            .lines()
            .filter(|line| {
                let line = line.to_lowercase();
                line.contains("error:")
                    || line.contains("fatal error")
                    || line.contains("backtrace")
            })
            .collect::<Vec<_>>();
        return if diagnostics.is_empty() {
            "Paste the exact compiler error or serial crash text and I can look for common Arduino diagnostics.".to_owned()
        } else {
            format!(
                "Diagnostic lines for {board}:\n\n{}\n\nCheck that the selected board and installed libraries match these errors.",
                diagnostics.join("\n")
            )
        };
    }

    format!(
        "I’m Rustino’s built-in offline Arduino assistant for {board}. I can generate LED blink, button-to-LED, and Serial examples without another program. Ask for one of those, or include the exact compiler error you want help with."
    )
}

#[tauri::command]
async fn format_cpp(source: String, path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut child = hidden_command("clang-format")
            .arg(format!("--assume-filename={path}"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Could not start clang-format: {error}"))?;
        child
            .stdin
            .take()
            .ok_or("Could not open clang-format input")?
            .write_all(source.as_bytes())
            .map_err(|error| error.to_string())?;
        let output = child
            .wait_with_output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
        }
        String::from_utf8(output.stdout).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn serial_start(
    app: AppHandle,
    state: State<'_, AppState>,
    cli_path: String,
    port: String,
    fqbn: String,
    baud: u32,
) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    let (done_sender, done) = mpsc::channel();
    let (ready_sender, ready) = mpsc::channel();
    {
        let mut active = state
            .serial
            .lock()
            .map_err(|_| "Serial state lock failed")?;
        if active.is_some() {
            return Err("Serial Monitor is already connected".to_owned());
        }
        *active = Some(SerialSession { sender, done });
    }
    let serial_state = state.serial.clone();
    thread::spawn(move || {
        serial_worker(
            app,
            SerialConfig {
                cli_path,
                port,
                fqbn,
                baud,
            },
            SerialWorkerChannels {
                receiver,
                done: done_sender,
                ready: ready_sender,
                state: serial_state,
            },
        )
    });
    match ready.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error),
        Err(error) => {
            let _ = stop_serial_monitor(state.inner());
            Err(format!("Serial Monitor did not start: {error}"))
        }
    }
}

#[tauri::command]
fn serial_send(state: State<'_, AppState>, line: String) -> Result<(), String> {
    let active = state
        .serial
        .lock()
        .map_err(|_| "Serial state lock failed")?;
    let session = active.as_ref().ok_or("Serial Monitor is not connected")?;
    session
        .sender
        .send(SerialMessage::Write(line))
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn serial_stop(state: State<'_, AppState>) -> Result<(), String> {
    stop_serial_monitor(state.inner()).map(|_| ())
}

fn run_build(
    app: &AppHandle,
    cli_path: String,
    sketch_path: String,
    fqbn: String,
    port: String,
    upload: bool,
    export: bool,
) -> Result<CliResult, String> {
    let _build_guard = BUILD_MUTEX
        .lock()
        .map_err(|_| "Another build is active and the build lock failed".to_owned())?;
    let started = Instant::now();
    let port = if upload {
        let was_automatic = port.trim().is_empty();
        let resolved = resolve_upload_port(&cli_path, &fqbn, &port)?;
        if was_automatic {
            emit_cli(
                app,
                format!("Automatically selected upload port {resolved} for {fqbn}."),
                "system",
            );
        }
        resolved
    } else {
        port
    };
    let sketch_dir = prepare_cli_sketch(&sketch_path)?;
    let make_command = |target: &str| {
        let mut command = hidden_command(resolve_cli(&cli_path));
        command.args(["compile", "--fqbn", target]);
        if export {
            command.arg("--export-binaries");
        }
        let plan = turbo_builder::build_plan(Path::new(&sketch_path), target);
        turbo_builder::add_compile_options(&mut command, &plan);
        if upload {
            command.arg("--upload");
            if !port.trim().is_empty() {
                command.args(["--port", port.trim()]);
            }
        }
        command.arg(&sketch_dir);
        command
    };
    let (mut success, mut output) = run_streamed(app, &mut make_command(&fqbn))?;
    let mut detected_fqbn = None;
    let mut effective_fqbn = fqbn.clone();
    let mut retried = false;
    if upload
        && !success
        && let Some(target) = parse_esptool_chip_fqbn(&output).filter(|target| target != &fqbn)
    {
        retried = true;
        effective_fqbn = target.clone();
        detected_fqbn = Some(target.clone());
        emit_cli(
            app,
            format!("Detected chip mismatch. Retrying with {target}…"),
            "system",
        );
        (success, output) = run_streamed(app, &mut make_command(&target))?;
    }
    if success && export {
        copy_exported_binaries(
            &sketch_dir,
            Path::new(&sketch_path).parent().unwrap_or(&sketch_dir),
        )?;
    }
    let elapsed = started.elapsed().as_secs_f32();
    let summary = if success {
        if upload {
            format!("Upload completed successfully in {elapsed:.2}s for {effective_fqbn}.")
        } else if export {
            format!(
                "Compiled binary exported to the sketch folder in {elapsed:.2}s for {effective_fqbn}."
            )
        } else {
            format!("Build completed successfully in {elapsed:.2}s for {effective_fqbn}.")
        }
    } else if is_compile_failure(&output) {
        compile_failure_summary(&output, &effective_fqbn, elapsed)
    } else if upload {
        upload_failure_summary(&output, &port, elapsed, detected_fqbn.as_deref(), retried)
    } else {
        format!(
            "Build failed after {elapsed:.2}s for {effective_fqbn}. Check the live output for compiler diagnostics."
        )
    };
    if retried {
        emit_cli(
            app,
            format!("Selected detected board: {effective_fqbn}"),
            "system",
        );
    }
    Ok(CliResult {
        success,
        summary,
        output,
        duration_seconds: elapsed,
        detected_fqbn,
        selected_port: upload.then_some(port),
    })
}

fn run_simple_cli(
    app: &AppHandle,
    cli_path: &str,
    args: &[&str],
    title: &str,
) -> Result<CliResult, String> {
    let started = Instant::now();
    let mut command = hidden_command(resolve_cli(cli_path));
    command.args(args);
    let (success, output) = run_streamed(app, &mut command)?;
    let elapsed = started.elapsed().as_secs_f32();
    let summary = if success {
        format!("{title} completed in {elapsed:.2}s.")
    } else {
        format!("{title} failed after {elapsed:.2}s. See the output panel.")
    };
    Ok(CliResult {
        success,
        summary,
        output,
        duration_seconds: elapsed,
        detected_fqbn: None,
        selected_port: None,
    })
}

fn resolve_upload_port(cli_path: &str, fqbn: &str, requested: &str) -> Result<String, String> {
    let requested = requested.trim();
    let scan = hidden_command(resolve_cli(cli_path))
        .args(["board", "list", "--format", "json"])
        .output();
    let detected = if let Ok(scan) = scan
        && scan.status.success()
        && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&scan.stdout)
    {
        value
            .get("detected_ports")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    if !requested.is_empty() {
        let cli_detected = detected.iter().any(|entry| {
            entry
                .pointer("/port/address")
                .and_then(serde_json::Value::as_str)
                == Some(requested)
        });
        if cli_detected || available_ports().iter().any(|port| port == requested) {
            return Ok(requested.to_owned());
        }
        return Err(format!(
            "Selected upload port {requested} is not connected. Reconnect the board or select a currently detected port."
        ));
    }

    if !detected.is_empty() {
        let mut matching = detected
            .iter()
            .filter(|entry| {
                entry
                    .get("matching_boards")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|boards| {
                        boards.iter().any(|board| {
                            board.get("fqbn").and_then(serde_json::Value::as_str) == Some(fqbn)
                        })
                    })
            })
            .filter_map(|entry| {
                entry
                    .pointer("/port/address")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>();
        matching.sort();
        matching.dedup();
        if matching.len() == 1 {
            return Ok(matching.remove(0));
        }
        if matching.len() > 1 {
            return Err(format!(
                "More than one connected board matches {fqbn} ({}). Select the upload port in Board settings.",
                matching.join(", ")
            ));
        }
    }

    let ports = available_ports();
    match ports.as_slice() {
        [port] => Ok(port.clone()),
        [] => Err("No upload port is selected and no serial port was detected. Connect the board, scan ports, and select its port in Board settings.".to_owned()),
        _ => Err(format!(
            "No upload port is selected and multiple serial ports are connected ({}). Select the ESP32 port in Board settings.",
            ports.join(", ")
        )),
    }
}

fn copy_exported_binaries(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    let source = fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
    let destination = fs::canonicalize(destination).unwrap_or_else(|_| destination.to_path_buf());
    if source == destination {
        return Ok(());
    }
    for entry in fs::read_dir(&source).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if !path.is_file()
            || !matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("bin" | "hex" | "elf")
            )
        {
            continue;
        }
        let name = path.file_name().ok_or("Exported binary has no file name")?;
        fs::copy(&path, destination.join(name)).map_err(|error| {
            format!("Could not copy exported binary {}: {error}", path.display())
        })?;
    }
    Ok(())
}

fn run_streamed(app: &AppHandle, command: &mut Command) -> Result<(bool, String), String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start Arduino CLI: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Could not capture Arduino CLI output")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("Could not capture Arduino CLI errors")?;
    let stdout_app = app.clone();
    let stderr_app = app.clone();
    let stdout_reader = thread::spawn(move || collect_and_emit(stdout, stdout_app, "stdout"));
    let stderr_reader = thread::spawn(move || collect_and_emit(stderr, stderr_app, "stderr"));
    let status = child
        .wait()
        .map_err(|error| format!("Arduino CLI could not finish: {error}"))?;
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    let output = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Ok((status.success(), output))
}

fn collect_and_emit<R: Read>(reader: R, app: AppHandle, stream: &'static str) -> String {
    collect_text_chunks(reader, |text| emit_cli(&app, text.to_owned(), stream)).unwrap_or_default()
}

/// Forward process and serial data as soon as `read` returns bytes. A line
/// reader can block forever when a process writes carriage-return progress or
/// a device prints without a newline.
fn collect_text_chunks<R: Read>(
    mut reader: R,
    mut on_chunk: impl FnMut(&str),
) -> std::io::Result<String> {
    let mut collected = String::new();
    let mut pending = Vec::<u8>::new();
    let mut buffer = [0_u8; 4096];

    loop {
        let count = match reader.read(&mut buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        let eof = count == 0;
        pending.extend_from_slice(&buffer[..count]);

        loop {
            match std::str::from_utf8(&pending) {
                Ok(valid) if !valid.is_empty() => {
                    collected.push_str(valid);
                    on_chunk(valid);
                    pending.clear();
                }
                Ok(_) => break,
                Err(error) => {
                    let valid_len = error.valid_up_to();
                    if valid_len > 0 {
                        let valid = std::str::from_utf8(&pending[..valid_len])
                            .expect("valid_up_to points to valid UTF-8");
                        collected.push_str(valid);
                        on_chunk(valid);
                        pending.drain(..valid_len);
                        continue;
                    }
                    if let Some(invalid_len) = error.error_len() {
                        let invalid = String::from_utf8_lossy(&pending[..invalid_len]);
                        collected.push_str(&invalid);
                        on_chunk(&invalid);
                        pending.drain(..invalid_len);
                        continue;
                    }
                    break;
                }
            }
        }

        if eof {
            if !pending.is_empty() {
                let tail = String::from_utf8_lossy(&pending);
                collected.push_str(&tail);
                on_chunk(&tail);
            }
            return Ok(collected);
        }
    }
}

fn emit_cli(app: &AppHandle, line: String, stream: &str) {
    let _ = app.emit(
        "cli-output",
        serde_json::json!({"line": line, "stream": stream}),
    );
}

fn serial_worker(app: AppHandle, config: SerialConfig, channels: SerialWorkerChannels) {
    let SerialConfig {
        cli_path,
        port,
        fqbn,
        baud,
    } = config;
    let SerialWorkerChannels {
        receiver,
        done,
        ready,
        state: serial_state,
    } = channels;
    let baud_config = format!("baudrate={baud}");
    let spawn = hidden_command(resolve_cli(&cli_path))
        .args([
            "monitor",
            "--port",
            &port,
            "--fqbn",
            &fqbn,
            "--config",
            &baud_config,
            "--raw",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawn {
        Ok(child) => child,
        Err(error) => {
            let _ = app.emit("cli-output", serde_json::json!({"line": format!("Could not start Serial Monitor: {error}"), "stream": "stderr"}));
            let _ = app.emit("serial-ended", ());
            let _ = ready.send(Err(format!(
                "Could not open serial monitor on {port}: {error}"
            )));
            if let Ok(mut state) = serial_state.lock() {
                state.take();
            }
            let _ = done.send(());
            return;
        }
    };
    let stdout = child.stdout.take().expect("piped monitor stdout");
    let stderr = child.stderr.take().expect("piped monitor stderr");
    let stdout_app = app.clone();
    let stdout_reader = thread::spawn(move || stream_serial(stdout, stdout_app));
    let stderr_app = app.clone();
    let stderr_reader = thread::spawn(move || collect_and_emit(stderr, stderr_app, "stderr"));
    let mut input = child.stdin.take().expect("piped monitor stdin");
    // Give Arduino CLI a moment to validate the selected port and options.
    // Otherwise an invalid command exits just after the UI reports connected.
    thread::sleep(Duration::from_millis(60));
    if child.try_wait().ok().flatten().is_some() {
        let _ = child.wait();
        let _ = stdout_reader.join();
        let error = stderr_reader.join().unwrap_or_default();
        let message = if error.trim().is_empty() {
            format!("Arduino CLI Serial Monitor exited while opening {port}.")
        } else {
            error.trim().to_owned()
        };
        let _ = ready.send(Err(message));
        let _ = app.emit("serial-ended", ());
        if let Ok(mut state) = serial_state.lock() {
            state.take();
        }
        let _ = done.send(());
        return;
    }
    let _ = ready.send(Ok(()));
    loop {
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        match receiver.recv_timeout(Duration::from_millis(30)) {
            Ok(SerialMessage::Write(text)) => {
                if input.write_all(text.as_bytes()).is_err()
                    || input.write_all(b"\n").is_err()
                    || input.flush().is_err()
                {
                    break;
                }
            }
            Ok(SerialMessage::Stop) | Err(RecvTimeoutError::Disconnected) => {
                let _ = child.kill();
                break;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
    let _ = child.wait();
    let _ = stdout_reader.join();
    let _ = stderr_reader.join();
    let _ = app.emit("serial-ended", ());
    if let Ok(mut state) = serial_state.lock() {
        state.take();
    }
    let _ = done.send(());
}

fn stream_serial<R: Read>(reader: R, app: AppHandle) -> String {
    collect_text_chunks(reader, |text| {
        let _ = app.emit("serial-data", text);
    })
    .unwrap_or_default()
}

fn stop_serial_monitor(state: &AppState) -> Result<bool, String> {
    let session = state
        .serial
        .lock()
        .map_err(|_| "Serial state lock failed")?
        .take();
    let Some(session) = session else {
        return Ok(false);
    };
    let _ = session.sender.send(SerialMessage::Stop);
    match session.done.recv_timeout(Duration::from_secs(5)) {
        Ok(()) => Ok(true),
        Err(_) => Err("Serial Monitor did not release the port within 5 seconds. Disconnect it and retry upload.".to_owned()),
    }
}

fn prepare_cli_sketch(sketch_file: &str) -> Result<PathBuf, String> {
    let source = Path::new(sketch_file);
    if !source
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ino"))
    {
        return Err(format!(
            "{} is not an Arduino .ino sketch",
            source.display()
        ));
    }
    let parent = source.parent().ok_or("Sketch has no parent directory")?;
    let stem = source
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or("Sketch name is invalid")?;
    let cli_stem = turbo_builder::cli_sketch_name(stem);
    if parent
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|folder| {
            if cli_stem != stem {
                return false;
            }
            if cfg!(windows) {
                folder.eq_ignore_ascii_case(stem)
            } else {
                folder == stem
            }
        })
    {
        return Ok(parent.to_path_buf());
    }
    let staged = turbo_builder::staged_sketch_path(source);
    if staged.exists() {
        fs::remove_dir_all(&staged).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&staged).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(parent).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            continue;
        }
        let path = entry.path();
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        if !["ino", "h", "hpp", "cpp", "c"]
            .iter()
            .any(|allowed| extension.eq_ignore_ascii_case(allowed))
        {
            continue;
        }
        let destination = if path == source {
            staged.join(format!("{cli_stem}.ino"))
        } else {
            staged.join(entry.file_name())
        };
        fs::copy(path, destination).map_err(|error| error.to_string())?;
    }
    let main = staged.join(format!("{cli_stem}.ino"));
    if !main.is_file() {
        return Err(format!("Could not stage the sketch at {}", main.display()));
    }
    Ok(staged)
}

fn parse_esptool_chip_fqbn(output: &str) -> Option<String> {
    let lower = output.to_ascii_lowercase();
    let marker = "this chip is ";
    let start = lower.find(marker)? + marker.len();
    let chip = lower[start..].split([',', '.', '\n', '\r']).next()?.trim();
    let fqbn = if chip.starts_with("esp32-c3") {
        "esp32:esp32:esp32c3"
    } else if chip.starts_with("esp32-c6") {
        "esp32:esp32:esp32c6"
    } else if chip.starts_with("esp32-s3") {
        "esp32:esp32:esp32s3"
    } else if chip.starts_with("esp32-s2") {
        "esp32:esp32:esp32s2"
    } else if chip.starts_with("esp32-c2") {
        "esp32:esp32:esp32c2"
    } else if chip == "esp32" || chip.starts_with("esp32-d0wd") {
        "esp32:esp32:esp32"
    } else {
        return None;
    };
    Some(fqbn.to_owned())
}

fn is_compile_failure(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    lower.contains("error during build")
        || lower.contains("fatal error:")
        || lower.contains("compilation terminated")
}

fn compile_failure_summary(output: &str, fqbn: &str, elapsed: f32) -> String {
    let lower = output.to_ascii_lowercase();
    let board_hint = if fqbn.to_ascii_lowercase().contains("esp32c3")
        && lower.contains("gpio_num_33")
    {
        "This build targets ESP32-C3 but the sketch refers to GPIO 33 and classic ESP32 EXT0 wakeup APIs. Check the selected board profile and the sketch's hardware-specific pin code."
    } else {
        "The sketch failed to compile for the selected board. Check the compiler diagnostics in Output and confirm the selected board profile."
    };
    format!("Build failed before upload after {elapsed:.2}s for {fqbn}. {board_hint}")
}

fn upload_failure_summary(
    output: &str,
    port: &str,
    elapsed: f32,
    detected: Option<&str>,
    retried: bool,
) -> String {
    let lower = output.to_ascii_lowercase();
    let advice = if lower.contains("no serial data received")
        || lower.contains("failed to connect to esp32")
        || lower.contains("wrong boot mode")
        || lower.contains("timed out waiting for packet header")
    {
        "The chip did not enter download mode. Hold BOOT, tap EN/RESET, and release BOOT when flashing begins."
    } else if lower.contains("permission denied") {
        "The serial port denied access. Check serial-port permissions and close other serial programs."
    } else if lower.contains("resource busy") || lower.contains("port is busy") {
        "The serial port is busy. Rustino stopped its monitor before upload; close any other program using this port."
    } else {
        "Upload failed. Review the live Arduino CLI output for the specific tool diagnostic."
    };
    let target = detected
        .map(|fqbn| {
            format!(
                " Detected target: {fqbn}{}.",
                if retried { " (retry also failed)" } else { "" }
            )
        })
        .unwrap_or_default();
    format!(
        "Upload failed after {elapsed:.2}s on {}. {advice}{target}",
        if port.is_empty() {
            "automatic port selection"
        } else {
            port
        }
    )
}

#[cfg(test)]
fn format_board_scan(output: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(output) else {
        return output.trim().to_owned();
    };
    let Some(ports) = value
        .get("detected_ports")
        .and_then(serde_json::Value::as_array)
    else {
        return "Arduino CLI returned no port list.".to_owned();
    };
    if ports.is_empty() {
        return "No board matches found. Select a serial port manually.".to_owned();
    }
    let mut lines = Vec::new();
    for entry in ports {
        let address = entry
            .pointer("/port/address")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown port");
        let label = entry
            .pointer("/port/label")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        lines.push(if label.is_empty() {
            address.to_owned()
        } else {
            format!("{address} — {label}")
        });
        if let Some(boards) = entry
            .get("matching_boards")
            .and_then(serde_json::Value::as_array)
        {
            for board in boards {
                let name = board
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Unknown board");
                let fqbn = board
                    .get("fqbn")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                lines.push(if fqbn.is_empty() {
                    format!("  {name}")
                } else {
                    format!("  {name} ({fqbn})")
                });
            }
        }
    }
    lines.join("\n")
}

fn parse_library_results(output: &str) -> Result<Vec<LibraryItem>, String> {
    let value: serde_json::Value = serde_json::from_str(output)
        .map_err(|error| format!("Could not parse Arduino library results: {error}"))?;
    if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
        return Err(error.to_owned());
    }
    let entries = value
        .get("libraries")
        .or_else(|| value.get("searched_libraries"))
        .and_then(serde_json::Value::as_array)
        .ok_or("Arduino CLI returned no library result list")?;
    Ok(entries
        .iter()
        .filter_map(|entry| {
            let name = entry.get("name")?.as_str()?.to_owned();
            let release = entry.get("latest").unwrap_or(entry);
            Some(LibraryItem {
                name,
                version: release
                    .get("version")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("latest")
                    .to_owned(),
                sentence: release
                    .get("sentence")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Arduino library")
                    .to_owned(),
            })
        })
        .collect())
}

fn choose_path(save: bool) -> Result<Option<String>, String> {
    #[cfg(windows)]
    {
        let script = if save {
            "$ErrorActionPreference = 'Stop'; Add-Type -AssemblyName System.Windows.Forms -ErrorAction Stop; $d = New-Object System.Windows.Forms.SaveFileDialog; $d.Filter = 'Arduino sketch (*.ino)|*.ino|C++ source (*.cpp)|*.cpp|All files (*.*)|*.*'; $d.DefaultExt = 'ino'; $d.AddExtension = $true; $d.FileName = 'Sketch.ino'; if ($d.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { [Console]::Write($d.FileName) }"
        } else {
            "$ErrorActionPreference = 'Stop'; Add-Type -AssemblyName System.Windows.Forms -ErrorAction Stop; $d = New-Object System.Windows.Forms.OpenFileDialog; $d.Filter = 'Arduino sketch (*.ino)|*.ino|C++ source (*.cpp)|*.cpp|All files (*.*)|*.*'; $d.Multiselect = $false; if ($d.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { [Console]::Write($d.FileName) }"
        };
        let output = hidden_command("powershell.exe")
            .args(["-NoProfile", "-STA", "-Command", script])
            .output()
            .map_err(|error| format!("Could not open the Windows file chooser: {error}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
        }
        let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        return Ok((!path.is_empty()).then_some(path));
    }
    #[cfg(not(windows))]
    {
        let mut dialog = Command::new("zenity");
        dialog.args(["--file-selection", "--title=Rustino — Select sketch"]);
        if save {
            dialog.args(["--save", "--confirm-overwrite"]);
        } else {
            dialog.arg("--file-filter=Arduino sketches | *.ino");
        }
        let output = dialog
            .output()
            .map_err(|error| format!("Could not open file chooser (install Zenity): {error}"))?;
        if !output.status.success() {
            return Ok(None);
        }
        let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Ok((!path.is_empty()).then_some(path))
    }
}

fn list_project_files_inner(sketch_path: &str) -> Vec<String> {
    let Some(parent) = Path::new(sketch_path).parent() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut files = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| {
                    ["ino", "h", "hpp", "cpp", "c"]
                        .iter()
                        .any(|allowed| extension.eq_ignore_ascii_case(allowed))
                })
                .then(|| path.to_string_lossy().into_owned())
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn available_ports() -> Vec<String> {
    #[cfg(windows)]
    {
        let mut ports = serialport::available_ports()
            .unwrap_or_default()
            .into_iter()
            .map(|port| port.port_name)
            .collect::<Vec<_>>();
        ports.sort();
        ports.dedup();
        return ports;
    }
    #[cfg(not(windows))]
    {
        let mut ports = Vec::new();
        if let Ok(entries) = fs::read_dir("/dev") {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("ttyACM") || name.starts_with("ttyUSB") {
                    ports.push(format!("/dev/{name}"));
                }
            }
        }
        if let Ok(entries) = fs::read_dir("/dev/serial/by-id") {
            for entry in entries.flatten() {
                if let Ok(path) = fs::canonicalize(entry.path()) {
                    ports.push(path.to_string_lossy().into_owned());
                }
            }
        }
        ports.sort();
        ports.dedup();
        ports
    }
}

fn resolve_cli(path: &str) -> String {
    if !path.trim().is_empty() {
        return path.to_owned();
    }
    #[cfg(windows)]
    return "arduino-cli.exe".to_owned();
    #[cfg(not(windows))]
    "arduino-cli".to_owned()
}

fn hidden_command(program: impl AsRef<OsStr>) -> Command {
    let command = Command::new(program);
    #[cfg(windows)]
    let command = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let mut command = command;
        command.creation_flags(CREATE_NO_WINDOW);
        command
    };
    command
}

#[cfg(windows)]
fn create_first_run_sketch(app: &AppHandle) -> Result<PathBuf, String> {
    let documents = app
        .path()
        .document_dir()
        .map_err(|error| format!("Could not locate the Documents folder: {error}"))?;
    let path = documents
        .join("Rustino")
        .join("Sketches")
        .join("Blink")
        .join("Blink.ino");
    let parent = path
        .parent()
        .ok_or("Starter sketch has no parent directory")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    if !path.exists() {
        write_file_atomic(&path, STARTER.as_bytes()).map_err(|error| error.to_string())?;
    }
    Ok(path)
}

fn discover_cli(app: &AppHandle) -> String {
    #[cfg(windows)]
    let bundled_name = "arduino-cli.exe";
    #[cfg(not(windows))]
    let bundled_name = "arduino-cli";
    let bundled = app
        .path()
        .resource_dir()
        .ok()
        .map(|directory| directory.join("arduino-cli").join(bundled_name))
        .filter(|path| path.is_file());
    if let Some(path) = bundled {
        return path.to_string_lossy().into_owned();
    }
    #[cfg(windows)]
    let development_cli = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources/arduino-cli-windows/arduino-cli.exe");
    #[cfg(not(windows))]
    let development_cli = PathBuf::new();
    if development_cli.is_file() {
        return development_cli.to_string_lossy().into_owned();
    }
    [
        "/opt/arduino-ide/resources/app/lib/backend/resources/arduino-cli",
        "/usr/bin/arduino-cli",
        "/usr/local/bin/arduino-cli",
    ]
    .iter()
    .find(|path| Path::new(path).is_file())
    .map(|path| (*path).to_owned())
    .unwrap_or_else(|| "arduino-cli".to_owned())
}

fn settings_path() -> Option<PathBuf> {
    #[cfg(windows)]
    if let Some(roaming) = std::env::var_os("APPDATA") {
        return Some(PathBuf::from(roaming).join("Rustino/settings.json"));
    }
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".config"))
        })
        .map(|base| base.join("rustino/settings.json"))
}

fn load_preferences() -> Preferences {
    settings_path()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let directory = path.parent().ok_or("Settings path has no parent folder")?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())
}

fn main() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_app_state,
            save_preferences,
            list_project_files,
            git_status,
            open_file,
            choose_sketch,
            choose_save_path,
            save_file,
            list_ports,
            scan_boards,
            open_sketch_folder,
            compile_sketch,
            upload_sketch,
            export_binary,
            install_esp32_core,
            search_libraries,
            install_library,
            local_ai_chat,
            format_cpp,
            serial_start,
            serial_send,
            serial_stop,
        ])
        .run(tauri::generate_context!())
        .expect("Rustino failed to start");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    struct ChunkReader {
        data: Vec<u8>,
        offset: usize,
        max_chunk: usize,
    }

    impl Read for ChunkReader {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if self.offset == self.data.len() {
                return Ok(0);
            }
            let count = output
                .len()
                .min(self.max_chunk)
                .min(self.data.len() - self.offset);
            output[..count].copy_from_slice(&self.data[self.offset..self.offset + count]);
            self.offset += count;
            Ok(count)
        }
    }

    #[test]
    fn chip_mismatch_maps_c3_and_classic_profiles() {
        assert_eq!(
            parse_esptool_chip_fqbn("This chip is ESP32-C3, not ESP32."),
            Some("esp32:esp32:esp32c3".to_owned())
        );
        assert_eq!(
            parse_esptool_chip_fqbn("This chip is ESP32, not ESP32-C3."),
            Some("esp32:esp32:esp32".to_owned())
        );
    }

    #[test]
    fn built_in_assistant_uses_selected_board_and_generates_blink_code() {
        let answer = builtin_embedded_response(
            "Target board FQBN: esp32:esp32:esp32c3\nQuestion: give the LED blink code",
        );
        assert!(answer.contains("esp32:esp32:esp32c3"));
        assert!(answer.contains("pinMode(LED_BUILTIN, OUTPUT)"));
        assert!(answer.contains("digitalWrite(LED_BUILTIN, HIGH)"));
    }

    #[test]
    fn built_in_assistant_generates_non_blocking_blink_when_requested() {
        let answer = builtin_embedded_response(
            "Target board FQBN: arduino:avr:uno\nQuestion: make a non-blocking LED blink with millis",
        );
        assert!(answer.contains("millis()"));
        assert!(!answer.contains("delay("));
    }

    #[test]
    fn built_in_assistant_uses_esp32_context_for_sensor_led_sketch() {
        let answer = builtin_embedded_response(
            "You are Rustino's assistant. Target board FQBN: esp32:esp32:esp32\nQuestion: ESP32 LED by sensor",
        );
        assert!(answer.contains("esp32:esp32:esp32"));
        assert!(answer.contains("SENSOR_PIN = 34"));
        assert!(answer.contains("3.3V sensor signal"));
    }

    #[test]
    fn c3_compile_failure_mentions_board_specific_diagnostic() {
        let summary = compile_failure_summary(
            "error: 'GPIO_NUM_33' was not declared in this scope",
            "esp32:esp32:esp32c3",
            1.0,
        );
        assert!(summary.contains("ESP32-C3"));
        assert!(summary.contains("before upload"));
    }

    #[test]
    fn library_results_parse_current_cli_json_shape() {
        let input = r#"{"libraries":[{"name":"Adafruit GFX Library","latest":{"version":"1.12.1","sentence":"Graphics core library"}}]}"#;
        let parsed = parse_library_results(input).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "Adafruit GFX Library");
        assert_eq!(parsed[0].version, "1.12.1");
    }

    #[test]
    fn scan_output_displays_detected_serial_port_and_board() {
        let input = r#"{"detected_ports":[{"port":{"address":"/dev/ttyUSB0","label":"CP2102"},"matching_boards":[{"name":"ESP32C3 Dev Module","fqbn":"esp32:esp32:esp32c3"}]}]}"#;
        let output = format_board_scan(input);
        assert!(output.contains("/dev/ttyUSB0"));
        assert!(output.contains("esp32:esp32:esp32c3"));
    }

    #[test]
    fn loose_ino_file_is_staged_with_cli_required_main_filename() {
        let root =
            std::env::temp_dir().join(format!("rustino-loose-sketch-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let source = root.join("Board Demo (1).ino");
        fs::write(&source, "void setup() {}\nvoid loop() {}\n").unwrap();
        fs::write(root.join("pins.h"), "#pragma once\n").unwrap();

        let staged = prepare_cli_sketch(source.to_str().unwrap()).unwrap();
        assert_eq!(
            staged.file_name().and_then(|name| name.to_str()),
            Some("Board_Demo__1_")
        );
        assert!(staged.join("Board_Demo__1_.ino").is_file());
        assert!(staged.join("pins.h").is_file());

        let stage_root = staged.parent().unwrap().to_path_buf();
        fs::remove_dir_all(stage_root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stream_reader_emits_unterminated_text_and_handles_split_utf8() {
        let reader = ChunkReader {
            data: "serial 😀 data without newline".as_bytes().to_vec(),
            offset: 0,
            max_chunk: 1,
        };
        let mut emitted = String::new();
        let collected = collect_text_chunks(reader, |chunk| emitted.push_str(chunk)).unwrap();
        assert_eq!(collected, "serial 😀 data without newline");
        assert_eq!(emitted, collected);
    }

    #[test]
    fn stream_reader_emits_carriage_return_progress_as_received() {
        let reader = ChunkReader {
            data: b"Step 1\rStep 2\rDone".to_vec(),
            offset: 0,
            max_chunk: 2,
        };
        let mut emitted = String::new();
        let collected = collect_text_chunks(reader, |chunk| emitted.push_str(chunk)).unwrap();
        assert_eq!(collected, "Step 1\rStep 2\rDone");
        assert_eq!(emitted, collected);
    }
}
