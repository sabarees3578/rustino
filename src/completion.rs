#[derive(Clone, Copy)]
pub struct Completion {
    pub label: &'static str,
    pub insert: &'static str,
    pub detail: &'static str,
}
pub const ITEMS: &[Completion] = &[
    Completion {
        label: "setup",
        insert: "void setup() {\n  \n}",
        detail: "Arduino entry point, runs once",
    },
    Completion {
        label: "loop",
        insert: "void loop() {\n  \n}",
        detail: "Arduino entry point, repeats",
    },
    Completion {
        label: "pinMode",
        insert: "pinMode(pin, OUTPUT);",
        detail: "Configure a GPIO pin",
    },
    Completion {
        label: "digitalWrite",
        insert: "digitalWrite(pin, HIGH);",
        detail: "Set a digital output",
    },
    Completion {
        label: "digitalRead",
        insert: "digitalRead(pin)",
        detail: "Read a digital input",
    },
    Completion {
        label: "analogRead",
        insert: "analogRead(pin)",
        detail: "Read an analog input",
    },
    Completion {
        label: "analogWrite",
        insert: "analogWrite(pin, value);",
        detail: "Write PWM output",
    },
    Completion {
        label: "Serial.begin",
        insert: "Serial.begin(115200);",
        detail: "Start the serial port",
    },
    Completion {
        label: "Serial.println",
        insert: "Serial.println(value);",
        detail: "Print a line to Serial",
    },
    Completion {
        label: "millis",
        insert: "millis()",
        detail: "Milliseconds since startup",
    },
    Completion {
        label: "attachInterrupt",
        insert: "attachInterrupt(digitalPinToInterrupt(pin), handler, RISING);",
        detail: "Attach a GPIO interrupt",
    },
    Completion {
        label: "Wire.begin",
        insert: "Wire.begin();",
        detail: "Start the I2C bus",
    },
    Completion {
        label: "SPI.begin",
        insert: "SPI.begin();",
        detail: "Start the SPI bus",
    },
    Completion {
        label: "HIGH",
        insert: "HIGH",
        detail: "Logic high",
    },
    Completion {
        label: "LOW",
        insert: "LOW",
        detail: "Logic low",
    },
    Completion {
        label: "INPUT_PULLUP",
        insert: "INPUT_PULLUP",
        detail: "Input with internal pull-up",
    },
    Completion {
        label: "LED_BUILTIN",
        insert: "LED_BUILTIN",
        detail: "Board built-in LED pin",
    },
    Completion {
        label: "uint8_t",
        insert: "uint8_t",
        detail: "8-bit unsigned integer",
    },
    Completion {
        label: "size_t",
        insert: "size_t",
        detail: "Object size type",
    },
];

pub fn filter(prefix: &str) -> impl Iterator<Item = &'static Completion> {
    ITEMS.iter().filter(move |item| {
        item.label
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
    })
}

pub fn apply_at_end(source: &mut String, replacement: &str) {
    let start = source
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_ascii_alphanumeric() || *c == '_')
        .last()
        .map(|(i, _)| i)
        .unwrap_or(source.len());
    source.replace_range(start.., replacement);
}

pub fn apply_at_char(source: &mut String, cursor: usize, replacement: &str) {
    let cursor = cursor.min(source.chars().count());
    let cursor_byte = source
        .char_indices()
        .nth(cursor)
        .map(|(i, _)| i)
        .unwrap_or(source.len());
    let start = source[..cursor_byte]
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_ascii_alphanumeric() || *c == '_')
        .last()
        .map(|(i, _)| i)
        .unwrap_or(cursor_byte);
    source.replace_range(start..cursor_byte, replacement);
}
