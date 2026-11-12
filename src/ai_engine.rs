use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AiContext {
    pub fqbn: String,
    pub architecture: String,
    pub headers: Vec<String>,
    pub libraries: Vec<String>,
    pub compiler_errors: String,
    pub crash_log: String,
}
impl AiContext {
    pub fn from_sketch(fqbn: &str, source: &str, compiler_errors: &str, crash_log: &str) -> Self {
        let headers = source
            .lines()
            .filter_map(|line| {
                line.trim()
                    .strip_prefix("#include <")
                    .and_then(|s| s.split_once('>'))
                    .map(|(h, _)| h.to_owned())
            })
            .collect::<Vec<_>>();
        let mut libraries = headers.clone();
        libraries.sort();
        libraries.dedup();
        let architecture = if fqbn.starts_with("esp32:") {
            "ESP32 / 3.3V logic; check ADC2 and Wi-Fi conflicts"
        } else if fqbn.starts_with("esp8266:") {
            "ESP8266 / 3.3V logic; limited GPIO and ADC"
        } else if fqbn.starts_with("arduino:avr:") {
            "AVR Arduino; check SRAM and voltage variant"
        } else {
            "Use board-specific voltage, pin, and memory limits for the selected FQBN"
        };
        Self {
            fqbn: fqbn.to_owned(),
            architecture: architecture.into(),
            headers,
            libraries,
            compiler_errors: compiler_errors.to_owned(),
            crash_log: crash_log.to_owned(),
        }
    }
    pub fn system_prompt(&self) -> String {
        format!(
            "You are an embedded Arduino C++ assistant. Target FQBN: {}. Hardware notes: {}. Included headers: {}. Installed or used libraries: {}. Respect voltage limits, valid pins, ADC/PWM/I2C/SPI capabilities, and avoid blocking code where possible.",
            self.fqbn,
            self.architecture,
            self.headers.join(", "),
            self.libraries.join(", ")
        )
    }
    pub fn error_prompt(&self, sketch: &str) -> String {
        format!(
            "{}\nFix the issue and return a unified diff only.\nCompiler errors:\n{}\nCrash log:\n{}\nSketch:\n{}",
            self.system_prompt(),
            self.compiler_errors,
            self.crash_log,
            sketch
        )
    }
}
