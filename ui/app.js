const { invoke } = window.__TAURI__.core;

const $ = (selector) => document.querySelector(selector);
const editor = $("#editor");
const outputPanel = $("#panel-output");
const serialPanel = $("#panel-serial");
const lineNumbers = $("#line-numbers");
const syntaxLayer = $("#syntax-layer");

const STARTER = `// Rustino sketch\n// Select your ESP32 board, then Verify or Upload.\n\n#ifndef LED_BUILTIN\n#define LED_BUILTIN 2  // Common built-in LED pin on ESP32 Dev Module boards.\n#endif\n\nvoid setup() {\n  pinMode(LED_BUILTIN, OUTPUT);\n}\n\nvoid loop() {\n  digitalWrite(LED_BUILTIN, HIGH);\n  delay(500);\n  digitalWrite(LED_BUILTIN, LOW);\n  delay(500);\n}\n`;
const COMPLETIONS = [
  ["setup", "void setup() {\n  \n}", "runs once"],
  ["loop", "void loop() {\n  \n}", "runs repeatedly"],
  ["pinMode", "pinMode(pin, OUTPUT);", "configure a GPIO"],
  ["digitalWrite", "digitalWrite(pin, HIGH);", "set a digital output"],
  ["digitalRead", "digitalRead(pin)", "read a digital input"],
  ["analogRead", "analogRead(pin)", "read an analog input"],
  ["analogWrite", "analogWrite(pin, value);", "write PWM output"],
  ["Serial.begin", "Serial.begin(115200);", "start serial"],
  ["Serial.println", "Serial.println(value);", "print a line"],
  ["millis", "millis()", "milliseconds since startup"],
  ["attachInterrupt", "attachInterrupt(digitalPinToInterrupt(pin), handler, RISING);", "attach GPIO interrupt"],
  ["Wire.begin", "Wire.begin();", "start I2C"],
  ["SPI.begin", "SPI.begin();", "start SPI"],
  ["HIGH", "HIGH", "logic high"], ["LOW", "LOW", "logic low"],
  ["INPUT_PULLUP", "INPUT_PULLUP", "input with pull-up"],
  ["LED_BUILTIN", "LED_BUILTIN", "built-in LED pin"],
  ["uint8_t", "uint8_t", "8-bit unsigned integer"], ["size_t", "size_t", "object size type"],
];

let prefs = { cli_path: "", fqbn: "esp32:esp32:esp32", port: "", sketch_path: "" };
let files = [];
let currentPath = "";
let currentName = "Blink.ino";
let dirty = false;
let panelVisible = true;
let activePanel = "output";
let serialActive = false;
let completionItems = [];
let completionIndex = 0;
let mode = localStorage.getItem("rustino-mode") || "normal";
let flowNodes = [];
let flowSequence = 0;
let aiHistory = [];
let latestAiCode = "";
let plotSamples = [];
let plotTextBuffer = "";
let aiRequestSequence = 0;
let activeAiRequest = 0;
let pendingAiBody = null;
let activeAiText = "";

const MODE_LABELS = { normal: "Normal", hardcore: "Hardcore", fun: "Fun", ai: "AI" };

function escapeHtml(value) {
  return value.replace(/[&<>"']/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]);
}

function setStatus(message, state = "system") {
  $("#status-state").textContent = message;
  if (state === "working") $("#status-state").dataset.state = "working";
  else delete $("#status-state").dataset.state;
}

function logLine(text, kind = "system") {
  const line = document.createElement("div");
  line.className = `panel-line ${kind}`;
  line.textContent = text;
  outputPanel.append(line);
  outputPanel.scrollTop = outputPanel.scrollHeight;
}

function appendStreamText(panel, text, kind = "") {
  const normalized = text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
  const parts = normalized.split(/(\n)/);
  for (const part of parts) {
    if (part === "\n") {
      panel._streamLine = null;
      continue;
    }
    if (!part) continue;
    if (!panel._streamLine || !panel._streamLine.isConnected) {
      panel._streamLine = document.createElement("div");
      panel._streamLine.className = `panel-line ${kind}`;
      panel.append(panel._streamLine);
    }
    panel._streamLine.textContent += part;
  }
  panel.scrollTop = panel.scrollHeight;
}

function displayCliResult(result, action = "Arduino CLI") {
  // Repaint with the complete captured stream after the live stream finishes.
  // This keeps the final diagnostics visible even if WebView events arrived late.
  outputPanel.replaceChildren();
  outputPanel._streamLine = null;
  logLine(`${action} result`, "system");
  if (result.output) appendStreamText(outputPanel, result.output);
  else logLine("Arduino CLI returned no output text.", "system");
  logLine(result.summary, result.success ? "success" : "error");
}

function toast(message, kind = "success") {
  document.querySelector(".toast")?.remove();
  const element = document.createElement("div");
  element.className = `toast ${kind}`;
  element.textContent = message;
  document.body.append(element);
  setTimeout(() => element.remove(), 4200);
}

window.addEventListener("error", (event) => {
  const message = `Interface error: ${event.message || "unknown JavaScript error"}`;
  if ($("#status-state")) setStatus("Interface error");
  if (outputPanel) logLine(message, "error");
});
window.addEventListener("unhandledrejection", (event) => {
  const message = `Action failed: ${String(event.reason)}`;
  if (outputPanel) logLine(message, "error");
  toast(message, "error");
});

function basename(path) { return path.split(/[\\/]/).filter(Boolean).at(-1) || path; }

function setDocument(path, content) {
  currentPath = path;
  currentName = path ? (basename(path) || "Untitled.ino") : "No file open";
  editor.value = content;
  dirty = false;
  renderEditor();
  $("#active-file-title").textContent = currentName;
  $("#tab-file-name").textContent = currentName;
  $("#breadcrumb-file").textContent = currentName;
  $("#breadcrumb-root").textContent = basename(path.replace(/[\\/][^\\/]+$/, "")) || "sketch";
  $("#workspace-name").textContent = path ? (basename(path.replace(/[\\/][^\\/]+$/, "")) || "Sketch") : "No workspace";
  $("#status-workspace").textContent = currentName;
  $("#close-file-tab").disabled = !path;
  updateDirty();
}

function setMode(nextMode) {
  if (!MODE_LABELS[nextMode]) return;
  mode = nextMode;
  localStorage.setItem("rustino-mode", mode);
  $("#mode-select").value = mode;
  $("#status-mode").textContent = `${MODE_LABELS[mode]} Mode`;
  document.body.dataset.mode = mode;
  $("#fun-canvas").classList.toggle("hidden", mode !== "fun");
  $("#ai-canvas").classList.toggle("hidden", mode !== "ai");
  $(".editor-wrap").classList.toggle("hidden", mode === "fun" || mode === "ai");
  $(".editor-footer").classList.toggle("hidden", mode === "fun" || mode === "ai");
  $("#complete-action").classList.toggle("hidden", mode === "hardcore");
  $("#editor-menu").classList.toggle("hidden", mode === "fun" || mode === "ai");
  if (mode === "fun") renderFlow();
  if (mode === "ai") updateLiveCode();
}

function addFlowNode(type) {
  const defaults = {
    input: { title: "Digital Input", pin: "4" },
    output: { title: "Digital Output", pin: "5" },
    delay: { title: "Delay", value: "250" },
    serial: { title: "Serial Print", value: "Sensor reading" },
    led: { title: "Built-in LED", value: "250" },
  };
  const config = defaults[type];
  if (!config) return;
  flowNodes.push({ id: ++flowSequence, type, ...config });
  renderFlow();
}

function renderFlow() {
  const canvas = $("#flow-canvas");
  if (!canvas) return;
  canvas.replaceChildren();
  if (!flowNodes.length) {
    const empty = document.createElement("div"); empty.className = "flow-empty"; empty.textContent = "Add a node above to start your flow."; canvas.append(empty); return;
  }
  flowNodes.forEach((node, index) => {
    const card = document.createElement("article"); card.className = `flow-node node-${node.type}`;
    const pinControl = node.type === "input" || node.type === "output"
      ? `<label>GPIO <input data-field="pin" type="number" min="0" max="48" value="${escapeHtml(node.pin)}"></label>`
      : "";
    const valueLabel = node.type === "delay" || node.type === "led" ? "Milliseconds" : "Text";
    const valueControl = node.type === "input" || node.type === "output" ? "" : `<label>${valueLabel}<input data-field="value" value="${escapeHtml(node.value)}"></label>`;
    card.innerHTML = `<div class="flow-port in"></div><header><span class="node-index">${String(index + 1).padStart(2, "0")}</span><strong>${escapeHtml(node.title)}</strong><button class="node-remove" title="Remove node">×</button></header><div class="node-fields">${pinControl}${valueControl}</div><div class="flow-port out"></div>`;
    card.querySelector("[data-field='pin']")?.addEventListener("input", (event) => { node.pin = event.target.value; });
    card.querySelector("[data-field='value']")?.addEventListener("input", (event) => { node.value = event.target.value; });
    card.querySelector(".node-remove").onclick = () => { flowNodes = flowNodes.filter((item) => item.id !== node.id); renderFlow(); };
    canvas.append(card);
    if (index < flowNodes.length - 1) { const connector = document.createElement("div"); connector.className = "flow-connector"; canvas.append(connector); }
  });
}

function generateFlowCode() {
  const declarations = [];
  const setup = ["  Serial.begin(115200);"];
  const loop = [];
  let inputNumber = 0;
  for (const node of flowNodes) {
    if (node.type === "input") {
      const pin = Math.max(0, Number.parseInt(node.pin, 10) || 0);
      const variable = `flowInput${inputNumber++}`;
      setup.push(`  pinMode(${pin}, INPUT_PULLUP);`);
      loop.push(`  const int ${variable} = digitalRead(${pin});`);
      loop.push(`  Serial.println(${variable});`);
    } else if (node.type === "output") {
      const pin = Math.max(0, Number.parseInt(node.pin, 10) || 0);
      setup.push(`  pinMode(${pin}, OUTPUT);`);
      loop.push(`  digitalWrite(${pin}, HIGH);`);
    } else if (node.type === "delay") {
      const ms = Math.max(0, Number.parseInt(node.value, 10) || 0);
      loop.push(`  delay(${ms});`);
    } else if (node.type === "serial") {
      const message = String(node.value).replaceAll("\\", "\\\\").replaceAll('"', '\\"');
      loop.push(`  Serial.println("${message}");`);
    } else if (node.type === "led") {
      setup.push("  pinMode(LED_BUILTIN, OUTPUT);");
      loop.push("  digitalWrite(LED_BUILTIN, HIGH);");
      loop.push(`  delay(${Math.max(0, Number.parseInt(node.value, 10) || 0)});`);
      loop.push("  digitalWrite(LED_BUILTIN, LOW);");
    }
  }
  if (!loop.length) loop.push("  // Add an operation node to this flow.");
  return `// Generated by Rustino Visual Firmware mode.\n${declarations.join("\n")}\nvoid setup() {\n${setup.join("\n")}\n}\n\nvoid loop() {\n${loop.join("\n")}\n}\n`;
}

function applyFlowToEditor() {
  editor.value = generateFlowCode();
  dirty = true;
  renderEditor();
  updateDirty();
  setMode("normal");
  logLine("Visual flow generated as Arduino C++ in the editor.", "success");
}

function addChatMessage(role, text) {
  const history = $("#chat-history");
  history.querySelector(".chat-empty")?.remove();
  const message = document.createElement("article");
  message.className = `chat-message ${role}`;
  const label = document.createElement("div"); label.className = "chat-role"; label.textContent = role === "user" ? "YOU" : "RUSTINO AI";
  const body = document.createElement("div"); body.className = "chat-message-text"; body.textContent = text;
  message.append(label, body); history.append(message); history.scrollTop = history.scrollHeight;
  return body;
}

function extractCode(response) {
  const fenced = response.match(/```(?:cpp|c\+\+|ino|c)?\s*\n([\s\S]*?)(?:```|$)/i);
  return fenced?.[1]?.trim() || "";
}

function updateLiveCode() {
  const replies = [...document.querySelectorAll(".chat-message.assistant .chat-message-text")];
  latestAiCode = extractCode(replies.at(-1)?.innerText || "") || editor.value;
  $("#live-code-preview").textContent = latestAiCode;
}

function isUploadRequest(question) {
  const normalized = question.toLowerCase()
    .replace(/\buplod(?:e|ing|ed)?\b/g, "upload")
    .replace(/\buplaod\b/g, "upload")
    .replace(/\buploade\b/g, "upload");
  return /\b(?:upload(?:ing|ed)?|flash(?:ing)?|program(?:ming)?)(?:\s+(?:the\s+)?(?:code|sketch|board|esp32))*\b/i.test(normalized);
}

async function sendLocalChat() {
  const input = $("#chat-input");
  const question = input.value.trim();
  if (!question) return;
  input.value = "";
  addChatMessage("user", question);
  aiHistory.push({ role: "user", content: question });
  const wantsUpload = isUploadRequest(question);
  const requestsNewCode = /\b(?:write|create|generate|make|draft|produce)\b.{0,60}\b(?:code|sketch|program|example)\b/i.test(question);
  if (wantsUpload && !requestsNewCode) {
    const send = $("#send-chat"); send.disabled = true; send.textContent = "Uploading…";
    const reply = addChatMessage("assistant", "Uploading the current sketch to the selected board…");
    try {
      const result = await build(true);
      reply.textContent = result?.success
        ? `Upload succeeded in ${result.durationSeconds.toFixed(2)} seconds.`
        : result?.summary || "Upload did not start. Check the Output panel for the board, port, or Arduino CLI error.";
      aiHistory.push({ role: "assistant", content: reply.textContent });
    } catch (error) {
      reply.textContent = `Upload failed: ${String(error)}`;
      aiHistory.push({ role: "assistant", content: reply.textContent });
    } finally {
      send.disabled = false; send.textContent = "Send ↗";
    }
    return;
  }
  const history = aiHistory.slice(-5, -1).map((item) => {
    const compact = item.content.replace(/```[\s\S]*?```/g, "[code omitted; use the editor contents if needed]").slice(0, 320);
    return `${item.role.toUpperCase()}: ${compact}`;
  }).join("\n");
  const needsSketch = /\b(?:this sketch|current sketch|my sketch|this code|my code|fix|debug|explain|modify|edit|refactor|compile|verify|upload|not working|doesn.t work)\b/i.test(question);
  const needsOutput = /\b(?:error|failed|failure|compile|verify|build|upload|diagnostic|crash|backtrace)\b/i.test(question);
  const sketchText = needsSketch
    ? `${editor.value.slice(0, 6000)}${editor.value.length > 6000 ? "\n// ... sketch context truncated ..." : ""}`
    : "Not included; the user did not ask about the current sketch.";
  const recentOutput = needsOutput ? outputPanel.innerText.slice(-2000) : "Not relevant to this request.";
  const model = $("#ai-model").value.trim() || "qwen2.5-coder:1.5b";
  const prompt = `You are Rustino, a concise and helpful assistant inside an Arduino IDE. Follow the latest user request exactly. Do not assume that the user wants code. For greetings or ordinary conversation, reply naturally and briefly without code. Only generate code when the user explicitly asks for code, an example, or a sketch. Never repeat or replace the current sketch unless asked. If asked to explain, answer in plain language. For code tasks, respect the selected board. Never say verify or upload succeeded unless the IDE reports success.\nTarget board FQBN: ${prefs.fqbn}\n\nRecent conversation (older context; the latest request takes priority):\n${history || "None"}\n\nRelevant sketch context (${currentPath || "unsaved"}):\n\`\`\`cpp\n${sketchText}\n\`\`\`\n\nRelevant IDE output:\n${recentOutput}\n\nLatest user request: ${question}\n\nQuestion: ${question}`;
  const send = $("#send-chat"); send.disabled = true; send.textContent = "Thinking…";
  const requestId = ++aiRequestSequence;
  activeAiRequest = requestId;
  pendingAiBody = addChatMessage("assistant", "");
  activeAiText = "";
  try {
    const response = await invoke("local_ai_chat", { prompt, model, requestId });
    if (!activeAiText) pendingAiBody.textContent = response;
    const finalResponse = activeAiText || response;
    aiHistory.push({ role: "assistant", content: finalResponse });
    const code = extractCode(finalResponse);
    latestAiCode = code || editor.value;
    $("#live-code-preview").textContent = latestAiCode || editor.value;
    const wantsVerify = /\b(verif\w*|compile|build|test)\b/i.test(question);
    const wantsApply = /\b(apply|use this|put it in the sketch|replace my sketch)\b/i.test(question);
    if (code && (wantsUpload || wantsVerify || wantsApply)) {
      if (!prefs.sketch_path || currentPath !== prefs.sketch_path) {
        pendingAiBody.textContent = `${finalResponse}\n\nOpen or save the main .ino sketch before I can apply and build it.`;
      } else {
        editor.value = code;
        dirty = true; renderEditor(); updateDirty();
        pendingAiBody.textContent = `${finalResponse}\n\nApplied the generated sketch. ${wantsUpload ? "Uploading now…" : wantsVerify ? "Verifying now…" : "It is ready in the editor."}`;
        if (wantsUpload || wantsVerify) {
          const result = await build(wantsUpload);
          pendingAiBody.textContent += result?.success
            ? `\n\n${wantsUpload ? "Upload" : "Verification"} succeeded.`
            : `\n\n${wantsUpload ? "Upload" : "Verification"} failed. See the Output panel.`;
        }
      }
    }
  } catch (error) {
    pendingAiBody.textContent = `Assistant action error: ${String(error)}`;
  } finally { if (activeAiRequest === requestId) { pendingAiBody = null; send.disabled = false; send.textContent = "Send ↗"; } }
}

function openPanel(panel) {
  activePanel = panel; panelVisible = true; showPanel();
}

async function formatCurrentCode() {
  try {
    editor.value = await invoke("format_cpp", { source: editor.value, path: currentPath || "sketch.ino" });
    dirty = true; renderEditor(); updateDirty(); toast("Formatted with clang-format.");
  } catch (error) { toast(String(error), "error"); }
}

function toggleComment() {
  const start = editor.selectionStart;
  const end = editor.selectionEnd;
  const lineStart = editor.value.lastIndexOf("\n", Math.max(0, start - 1)) + 1;
  let lineEnd = editor.value.indexOf("\n", end);
  if (lineEnd < 0) lineEnd = editor.value.length;
  const selected = editor.value.slice(lineStart, lineEnd);
  const lines = selected.split("\n");
  const remove = lines.every((line) => !line.trim() || line.trimStart().startsWith("//"));
  const updated = lines.map((line) => {
    if (!line.trim()) return line;
    if (remove) return line.replace(/^(\s*)\/\/ ?/, "$1");
    return line.replace(/^(\s*)/, "$1// ");
  }).join("\n");
  editor.setRangeText(updated, lineStart, lineEnd, "select");
  dirty = true; renderEditor(); updateDirty(); editor.focus();
}

function searchPlotData(text) {
  plotTextBuffer += text;
  const lines = plotTextBuffer.split(/\r\n|\r|\n/);
  plotTextBuffer = lines.pop() || "";
  for (const line of lines) {
    let values;
    try {
      const parsed = JSON.parse(line);
      values = Array.isArray(parsed) ? parsed.map(Number) : Object.values(parsed).map(Number);
    } catch (_) { values = line.split(",").map((part) => Number(part.trim())); }
    if (values.length && values.every(Number.isFinite)) {
      plotSamples.push(values.slice(0, 8));
      if (plotSamples.length > 300) plotSamples.shift();
    }
  }
  drawPlotter();
}

function drawPlotter() {
  const canvas = $("#plotter-canvas");
  if (!canvas || canvas.clientWidth < 1) return;
  const ratio = window.devicePixelRatio || 1;
  canvas.width = canvas.clientWidth * ratio; canvas.height = canvas.clientHeight * ratio;
  const context = canvas.getContext("2d"); context.scale(ratio, ratio);
  const width = canvas.clientWidth, height = canvas.clientHeight;
  context.fillStyle = "#11151b"; context.fillRect(0, 0, width, height);
  context.strokeStyle = "#29313d"; context.lineWidth = 1;
  for (let row = 1; row < 5; row++) { const y = row * height / 5; context.beginPath(); context.moveTo(0, y); context.lineTo(width, y); context.stroke(); }
  const channels = Math.max(0, ...plotSamples.map((sample) => sample.length));
  const colors = ["#58b8ff", "#59d69b", "#f0b45f", "#e681a4", "#a78bfa", "#ff7f50", "#62d5d5", "#d7dc73"];
  for (let channel = 0; channel < channels; channel++) {
    const data = plotSamples.map((sample) => sample[channel]).filter(Number.isFinite);
    if (data.length < 2) continue;
    let min = Math.min(...data), max = Math.max(...data); if (min === max) { min -= 1; max += 1; }
    context.strokeStyle = colors[channel % colors.length]; context.lineWidth = 1.7; context.beginPath();
    data.forEach((value, index) => { const x = index * width / (data.length - 1); const y = height - 10 - (value - min) / (max - min) * (height - 20); index ? context.lineTo(x, y) : context.moveTo(x, y); }); context.stroke();
  }
  const legend = $("#plotter-legend");
  legend.textContent = channels ? `${channels} channel${channels === 1 ? "" : "s"} · ${plotSamples.length} samples` : "Connect to a board and print comma-separated numbers or JSON arrays to plot values.";
}

function updateDirty() {
  $("#dirty-mark").classList.toggle("visible", dirty);
  $("#title-dirty").classList.toggle("visible", dirty);
  $("#editor-message").textContent = dirty ? "Unsaved changes" : (currentPath ? "Arduino C++ · UTF-8" : "Open a sketch to begin");
  $("#close-file-tab").disabled = !currentPath && !dirty;
  $("#close-file-action").disabled = !currentPath && !dirty;
}

function highlight(source) {
  const token = /(\/\/[^\n]*|\/\*[\s\S]*?\*\/|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|#\s*[a-zA-Z_]+|\b(?:void|bool|true|false|int|long|float|double|char|byte|unsigned|const|static|volatile|return|if|else|for|while|switch|case|break|continue|class|struct|public|private|using|namespace|sizeof|new|delete)\b|\b(?:HIGH|LOW|INPUT|OUTPUT|INPUT_PULLUP|LED_BUILTIN|uint8_t|uint16_t|uint32_t|size_t)\b|\b\d+(?:\.\d+)?\b|\b[a-zA-Z_]\w*\b)/g;
  let html = "";
  let last = 0;
  for (const match of source.matchAll(token)) {
    const value = match[0];
    const index = match.index;
    html += escapeHtml(source.slice(last, index));
    let className = "tok-key";
    if (value.startsWith("//") || value.startsWith("/*")) className = "tok-comment";
    else if (value.startsWith('"') || value.startsWith("'")) className = "tok-string";
    else if (value.startsWith("#")) className = "tok-pre";
    else if (/^\d/.test(value)) className = "tok-number";
    else if (/^(?:uint|size_t|bool|int|long|float|double|char|byte)/.test(value)) className = "tok-type";
    else if (source.slice(index + value.length).trimStart().startsWith("(")) className = "tok-func";
    html += `<span class="${className}">${escapeHtml(value)}</span>`;
    last = index + value.length;
  }
  html += escapeHtml(source.slice(last));
  syntaxLayer.innerHTML = `${html}\n`;
  lineNumbers.textContent = Array.from({ length: Math.max(1, source.split("\n").length) }, (_, i) => i + 1).join("\n");
}

function renderEditor() { highlight(editor.value); }

function updateBoardStatus() {
  $("#status-board").textContent = prefs.fqbn || "Select board";
  $("#status-port").textContent = prefs.port || "No port";
  const topBoard = $("#top-board");
  if (topBoard) {
    if (![...topBoard.options].some((option) => option.value === prefs.fqbn)) {
      const custom = new Option(`Custom: ${prefs.fqbn}`, prefs.fqbn);
      topBoard.add(custom);
    }
    topBoard.value = prefs.fqbn;
  }
  const topPort = $("#top-port");
  if (topPort) topPort.value = prefs.port || "";
}

async function persistPreferences() {
  await invoke("save_preferences", { preferences: prefs });
  updateBoardStatus();
}

async function refreshFiles() {
  if (!prefs.sketch_path) return;
  files = await invoke("list_project_files", { sketchPath: prefs.sketch_path });
  renderSidebar();
}

let projectSearchTimer;
function searchProject() {
  clearTimeout(projectSearchTimer);
  projectSearchTimer = setTimeout(async () => {
    const query = $("#project-search")?.value.trim().toLowerCase();
    const results = $("#search-results");
    if (!results) return;
    if (!query) { results.textContent = "Search the files in the current sketch."; return; }
    results.textContent = "Searching…";
    const matches = [];
    for (const path of files) {
      try {
        const document = await invoke("open_file", { path });
        document.content.split("\n").forEach((line, index) => {
          if (line.toLowerCase().includes(query)) matches.push({ path, index, line: line.trim() });
        });
      } catch (_) { /* A file may have been removed since the explorer refresh. */ }
    }
    results.replaceChildren();
    if (!matches.length) { results.textContent = "No matches."; return; }
    for (const match of matches.slice(0, 100)) {
      const button = document.createElement("button");
      button.className = "search-result";
      button.innerHTML = `<strong>${escapeHtml(basename(match.path))}:${match.index + 1}</strong><span>${escapeHtml(match.line)}</span>`;
      button.onclick = () => openFile(match.path);
      results.append(button);
    }
  }, 160);
}

async function refreshGitStatus() {
  const output = $("#git-status");
  if (!output || !prefs.sketch_path) return;
  try { output.textContent = await invoke("git_status", { sketchPath: prefs.sketch_path }); }
  catch (error) { output.textContent = String(error); }
}

async function renderSidebar(view = document.querySelector(".activity.active")?.dataset.view || "explorer") {
  const target = $("#side-content");
  const heading = $("#sidebar-title");
  target.replaceChildren();
  heading.textContent = view.toUpperCase();

  if (view === "explorer") {
    const workspace = document.createElement("div");
    workspace.className = "section-label";
    workspace.innerHTML = `<span>${escapeHtml(basename(prefs.sketch_path?.replace(/[\\/][^\\/]+$/, "") || "RUSTINO SKETCH").toUpperCase())}</span><span>⌄</span>`;
    target.append(workspace);
    const root = document.createElement("div");
    for (const file of files) {
      const button = document.createElement("button");
      button.className = `side-file ${file === currentPath ? "active" : ""}`;
      const glyph = file.endsWith(".ino") ? "◇" : file.endsWith(".h") ? "#" : "C";
      button.innerHTML = `<span class="file-glyph">${glyph}</span><span>${escapeHtml(basename(file))}</span>`;
      button.onclick = () => openFile(file);
      root.append(button);
    }
    target.append(root);
    if (!files.length) {
      const empty = document.createElement("p"); empty.className = "side-card-note"; empty.textContent = "Open a sketch to see its files."; target.append(empty);
    }
    const note = document.createElement("div"); note.className = "side-card-note explorer-note"; note.textContent = "Arduino C++ project"; target.append(note);
  } else if (view === "boards") {
    target.innerHTML = `<div class="side-card"><div class="side-card-title">BOARD TARGET</div><label class="side-label" for="board-select">Selected board</label><select id="board-select" class="side-select"></select><label class="side-label" for="fqbn-custom">Board FQBN</label><input id="fqbn-custom" class="side-input" value="${escapeHtml(prefs.fqbn)}"/><label class="side-label" for="port-select">Serial port</label><select id="port-select" class="side-select"><option value="">Select a port…</option></select><button class="side-button row" id="scan-ports">⟳ &nbsp; Scan ports</button><button class="side-button primary" id="install-core">Install / update ESP32 core</button><p class="side-card-note">Choose the profile matching your physical ESP32. If several serial devices are connected, select the board’s port explicitly.</p></div><div class="side-card"><div class="side-card-title">DETECTED BOARDS</div><div id="board-discovery" class="side-card-note">Scan to inspect ports.</div></div>`;
    const selector = $("#board-select");
    const boards = [["ESP32 Dev Module", "esp32:esp32:esp32"], ["ESP32-C3 Dev Module", "esp32:esp32:esp32c3"], ["ESP32-S3 Dev Module", "esp32:esp32:esp32s3"], ["ESP32-C6 Dev Module", "esp32:esp32:esp32c6"], ["ESP32-S2 Dev Module", "esp32:esp32:esp32s2"], ["ESP32-P4 Function EV Board", "esp32:esp32:esp32p4"]];
    const knownBoard = boards.some(([, fqbn]) => fqbn === prefs.fqbn);
    selector.innerHTML = `${knownBoard ? "" : `<option value="${escapeHtml(prefs.fqbn)}">Custom FQBN</option>`}${boards.map(([label, fqbn]) => `<option value="${fqbn}">${label}</option>`).join("")}`;
    selector.value = prefs.fqbn || boards[0][1];
    selector.onchange = async () => { prefs.fqbn = selector.value; $("#fqbn-custom").value = prefs.fqbn; await persistPreferences(); };
    $("#fqbn-custom").onchange = async (event) => { prefs.fqbn = event.target.value.trim(); await persistPreferences(); };
    await fillPorts();
    $("#scan-ports").onclick = scanBoards;
    $("#install-core").onclick = installCore;
  } else if (view === "search") {
    target.innerHTML = `<div class="side-card"><div class="side-card-title">SEARCH IN SKETCH</div><input id="project-search" class="side-input" placeholder="Find in project…"/><div id="search-results" class="side-card-note">Search the files in the current sketch.</div></div>`;
    $("#project-search").oninput = searchProject;
  } else if (view === "git") {
    target.innerHTML = `<div class="side-card"><div class="side-card-title">SOURCE CONTROL</div><p class="side-card-note">Git status for this sketch folder.</p><button class="side-button" id="refresh-git">⟳ &nbsp; Refresh</button></div><pre class="git-status" id="git-status">Checking Git…</pre>`;
    $("#refresh-git").onclick = refreshGitStatus;
    refreshGitStatus();
  } else if (view === "libraries") {
    target.innerHTML = `<div class="side-card"><div class="side-card-title">LIBRARY MANAGER</div><label class="side-label" for="library-query">Search Arduino libraries</label><input id="library-query" class="side-input" placeholder="e.g. Adafruit GFX"/><button id="library-search" class="side-button primary">Search libraries</button><label class="side-label"><input type="checkbox" id="library-deps" checked/> Install dependencies</label></div><div id="library-results"></div>`;
    $("#library-search").onclick = searchLibraries;
    $("#library-query").onkeydown = (event) => { if (event.key === "Enter") searchLibraries(); };
  } else {
    target.innerHTML = `<div class="side-card"><div class="side-card-title">TOOLCHAIN</div><label class="side-label" for="cli-path">Arduino CLI executable</label><input id="cli-path" class="side-input" value="${escapeHtml(prefs.cli_path)}"/><button id="save-settings" class="side-button">Save settings</button><p class="side-card-note">Rustino uses Arduino CLI and your installed board cores for compilation and upload.</p></div><div class="side-card"><div class="side-card-title">EDITOR</div><div class="side-card-note">Ctrl+S Save<br>Ctrl+R Verify<br>Ctrl+U Upload<br>Ctrl+Space Suggestions</div></div>`;
    $("#save-settings").onclick = async () => { prefs.cli_path = $("#cli-path").value.trim(); await persistPreferences(); toast("Toolchain settings saved."); };
  }
}

async function fillPorts() {
  const ports = await invoke("list_ports");
  const select = $("#port-select");
  if (select) select.innerHTML = `<option value="">Select a port…</option>${ports.map((port) => `<option value="${escapeHtml(port)}">${escapeHtml(port)}</option>`).join("")}`;
  const topSelect = $("#top-port");
  if (topSelect) topSelect.innerHTML = `<option value="">No port</option>${ports.map((port) => `<option value="${escapeHtml(port)}">${escapeHtml(port)}</option>`).join("")}`;
  const previous = prefs.port;
  if (!ports.includes(prefs.port) && ports.length > 0) prefs.port = mode !== "hardcore" && ports.length === 1 ? ports[0] : "";
  if (prefs.port) addPortOption(prefs.port);
  if (select) select.value = prefs.port;
  if (topSelect) topSelect.value = prefs.port;
  if (previous !== prefs.port) await persistPreferences();
  if (select) select.onchange = async () => { prefs.port = select.value; await persistPreferences(); };
}

function addPortOption(address) {
  if (!address) return;
  for (const selector of [$("#top-port"), $("#port-select")]) {
    if (selector && ![...selector.options].some((option) => option.value === address)) {
      selector.add(new Option(address, address));
    }
  }
}

function choosePort(address) {
  prefs.port = address;
  addPortOption(address);
  for (const selector of [$("#top-port"), $("#port-select")]) {
    if (selector) selector.value = address;
  }
}

async function scanBoards() {
  try {
    setStatus("Scanning ports…", "working");
    await fillPorts();
    const result = await invoke("scan_boards", { cliPath: prefs.cli_path });
    const place = $("#board-discovery");
    const detectedPorts = result.detected_ports || [];
    const detectedAddresses = detectedPorts.map((entry) => entry.port?.address).filter(Boolean);
    for (const address of detectedAddresses) addPortOption(address);
    const boardMatches = detectedPorts.filter((entry) => (entry.matching_boards || []).some((board) => board.fqbn === prefs.fqbn));
    const automaticPort = boardMatches.length === 1 ? boardMatches[0] : detectedPorts.length === 1 ? detectedPorts[0] : null;
    if (mode !== "hardcore" && automaticPort?.port?.address && !detectedAddresses.includes(prefs.port)) {
      choosePort(automaticPort.port.address);
      await persistPreferences();
    }
    if (place) {
      place.replaceChildren();
      if (!detectedPorts.length) place.textContent = "No boards detected. Select a serial port manually.";
      for (const entry of detectedPorts) {
        const address = entry.port?.address || "Unknown port";
        const group = document.createElement("div"); group.className = "detected-port";
        const portButton = document.createElement("button"); portButton.className = "side-button";
        portButton.textContent = "Use port " + address;
        portButton.onclick = async () => { choosePort(address); await persistPreferences(); toast("Selected " + address + "."); };
        group.append(portButton);
        for (const board of entry.matching_boards || []) {
          const boardButton = document.createElement("button"); boardButton.className = "side-button";
          boardButton.textContent = "Use " + (board.name || board.fqbn);
          boardButton.onclick = async () => {
            if (board.fqbn) {
              prefs.fqbn = board.fqbn;
              addBoardOption(board.name || board.fqbn, board.fqbn);
              if ($("#board-select")) { addBoardOption(board.name || board.fqbn, board.fqbn, $("#board-select")); $("#board-select").value = board.fqbn; }
              await persistPreferences();
            }
            choosePort(address); await persistPreferences();
            toast("Selected " + (board.name || board.fqbn) + " on " + address + ".");
          };
          group.append(boardButton);
        }
        place.append(group);
      }
    }
    logLine(detectedPorts.length ? "Detected " + detectedPorts.length + " serial port(s). Select a detected board or port in Board settings." : "No matching boards found. You can still select the port manually.", "system");
    setStatus("Ready");
  } catch (error) { setStatus("Scan failed"); logLine(String(error), "error"); }
}

function addBoardOption(label, fqbn, select = $("#top-board")) {
  if (!select || !fqbn) return;
  if (![...select.options].some((option) => option.value === fqbn)) select.add(new Option(label, fqbn));
}

async function openFile(path) {
  try {
    const doc = await invoke("open_file", { path });
    if (doc.path === currentPath) return;
    if (!(await confirmDocumentSwitch())) return;
    setDocument(doc.path, doc.content);
    if (/\.ino$/i.test(doc.path)) prefs.sketch_path = doc.path;
    await persistPreferences();
    await refreshFiles();
  } catch (error) { toast(String(error), "error"); }
}

async function openSketch() {
  try {
    const doc = await invoke("choose_sketch");
    if (!doc) return;
    if (doc.path === currentPath) return;
    if (!(await confirmDocumentSwitch())) return;
    setDocument(doc.path, doc.content);
    prefs.sketch_path = doc.path;
    await persistPreferences();
    await refreshFiles();
    logLine(`Opened ${doc.path}`);
  } catch (error) { toast(String(error), "error"); }
}

async function saveCurrent() {
  let destination = currentPath;
  if (!currentPath) {
    try {
      const path = await invoke("choose_save_path");
      if (!path) return false;
      destination = /\.(ino|cpp|c|h|hpp)$/i.test(path) ? path : path + ".ino";
    } catch (error) { toast(String(error), "error"); return false; }
  }
  try {
    await invoke("save_file", { path: destination, content: editor.value });
    currentPath = destination;
    currentName = basename(destination);
    $("#active-file-title").textContent = currentName;
    $("#tab-file-name").textContent = currentName;
    $("#breadcrumb-file").textContent = currentName;
    $("#status-workspace").textContent = currentName;
    if (/\.ino$/i.test(currentPath)) prefs.sketch_path = currentPath;
    dirty = false; updateDirty(); await persistPreferences(); await refreshFiles();
    logLine(`Saved ${currentPath}`, "success");
    return true;
  } catch (error) { toast(String(error), "error"); return false; }
}

async function newSketch() {
  try {
    const path = await invoke("choose_save_path");
    if (!path) return;
    const sketch = /\.ino$/i.test(path) ? path : `${path}.ino`;
    if (!(await confirmDocumentSwitch())) return;
    await invoke("save_file", { path: sketch, content: STARTER });
    setDocument(sketch, STARTER);
    prefs.sketch_path = sketch;
    await persistPreferences(); await refreshFiles();
    logLine(`Created ${sketch}`, "success");
  } catch (error) { toast(String(error), "error"); }
}

async function confirmDocumentSwitch() {
  if (!dirty) return true;
  const dialog = $("#unsaved-dialog");
  if (!dialog?.showModal) return confirm("Discard unsaved changes and continue?");
  return new Promise((resolve) => {
    const finish = (choice) => { dialog.close(); resolve(choice); };
    $("#unsaved-save").onclick = () => finish("save");
    $("#unsaved-discard").onclick = () => finish("discard");
    $("#unsaved-cancel").onclick = () => finish("cancel");
    dialog.oncancel = (event) => { event.preventDefault(); finish("cancel"); };
    dialog.showModal();
  }).then(async (choice) => {
    if (choice === "save") return await saveCurrent();
    return choice === "discard";
  });
}

async function closeCurrentFile() {
  if (!(await confirmDocumentSwitch())) return;
  const previousSketchPath = prefs.sketch_path;
  prefs.sketch_path = "";
  try {
    await persistPreferences();
  } catch (error) {
    prefs.sketch_path = previousSketchPath;
    toast(`Could not close the active file: ${String(error)}`, "error");
    return;
  }
  files = [];
  setDocument("", "");
  renderSidebar("explorer");
  editor.focus();
  toast("File closed. The file remains on disk.");
}

async function saveBeforeBuild() {
  if (!currentPath || currentPath !== prefs.sketch_path) {
    throw new Error("Open the main .ino sketch before building.");
  }
  await invoke("save_file", { path: currentPath, content: editor.value });
  dirty = false; updateDirty(); await persistPreferences();
}

async function build(upload) {
  activePanel = "output"; panelVisible = true; showPanel();
  outputPanel.replaceChildren(); outputPanel._streamLine = null;
  const action = upload ? "Upload" : "Verify";
  setStatus(upload ? "Uploading…" : "Verifying…", "working");
  logLine(`${action} started · Board ${prefs.fqbn || "not selected"}${upload ? ` · Port ${prefs.port || "automatic detection"}` : ""}`, "system");
  if (!prefs.cli_path) {
    const message = "Arduino CLI is not configured. Set its path in Settings.";
    setStatus(`${action} failed`); logLine(message, "error"); toast(message, "error"); return null;
  }
  if (!prefs.sketch_path) {
    const message = "No sketch is open. Open or save an .ino sketch first.";
    setStatus(`${action} failed`); logLine(message, "error"); toast(message, "error"); return null;
  }
  try {
    if (mode === "fun") {
      editor.value = generateFlowCode();
      dirty = true;
      renderEditor();
      updateDirty();
    }
    await saveBeforeBuild();
    const reconnectSerial = upload && serialActive;
    if (reconnectSerial) {
      serialActive = false;
      $("#serial-connect").textContent = "Connecting after upload…";
    }
    $("#verify").disabled = true; $("#upload").disabled = true;
    const result = upload
      ? await invoke("upload_sketch", { cliPath: prefs.cli_path, sketchPath: prefs.sketch_path, fqbn: prefs.fqbn, port: prefs.port })
      : await invoke("compile_sketch", { cliPath: prefs.cli_path, sketchPath: prefs.sketch_path, fqbn: prefs.fqbn });
    if (result.selectedPort && prefs.port !== result.selectedPort) {
      prefs.port = result.selectedPort;
      await persistPreferences();
      const selector = $("#top-port");
      if (selector) {
        if (![...selector.options].some((option) => option.value === prefs.port)) {
          selector.add(new Option(prefs.port, prefs.port));
        }
        selector.value = prefs.port;
      }
      const boardPort = $("#port-select");
      if (boardPort) {
        if (![...boardPort.options].some((option) => option.value === prefs.port)) {
          boardPort.add(new Option(prefs.port, prefs.port));
        }
        boardPort.value = prefs.port;
      }
    }
    if (result.detectedFqbn) { prefs.fqbn = result.detectedFqbn; await persistPreferences(); renderSidebar("boards"); }
    setStatus(result.success ? (upload ? "Upload complete" : "Build complete") : (upload ? "Upload failed" : "Build failed"));
    displayCliResult(result, action);
    toast(result.summary, result.success ? "success" : "error");
    return result;
  } catch (error) {
    activePanel = "output"; panelVisible = true; showPanel();
    setStatus(`${action} failed`); logLine(`${action} stopped: ${String(error)}`, "error"); toast(String(error), "error");
    return null;
  } finally {
    $("#verify").disabled = false; $("#upload").disabled = false;
    if (upload && $("#serial-connect").textContent === "Connecting after upload…") {
      try {
        await invoke("serial_start", { cliPath: prefs.cli_path, port: prefs.port, fqbn: prefs.fqbn, baud: Number($("#serial-baud").value) });
        serialActive = true;
        $("#serial-connect").textContent = "Disconnect";
        logLine("Serial Monitor reconnected after upload.", "success");
      } catch (error) {
        serialActive = false;
        $("#serial-connect").textContent = "Connect";
        logLine(`Could not reconnect Serial Monitor: ${String(error)}`, "warn");
      }
    }
  }
}

async function installCore() {
  try { activePanel = "output"; panelVisible = true; showPanel(); setStatus("Installing ESP32 core…", "working"); const result = await invoke("install_esp32_core", { cliPath: prefs.cli_path }); displayCliResult(result); setStatus(result.success ? "Ready" : "Core install failed"); }
  catch (error) { setStatus("Core install failed"); logLine(String(error), "error"); }
}

async function searchLibraries() {
  const query = $("#library-query").value.trim();
  if (!query) return;
  const results = $("#library-results");
  results.innerHTML = `<div class="side-card-note">Searching…</div>`;
  try {
    const libraries = await invoke("search_libraries", { cliPath: prefs.cli_path, query });
    results.replaceChildren();
    if (!libraries.length) { results.innerHTML = `<div class="side-card-note">No matching libraries found.</div>`; return; }
    for (const lib of libraries) {
      const row = document.createElement("div"); row.className = "result-row";
      const title = document.createElement("strong"); title.textContent = `${lib.name} · ${lib.version}`;
      const sentence = document.createElement("p"); sentence.textContent = lib.sentence;
      const install = document.createElement("button"); install.textContent = "Install";
      install.onclick = async () => { install.disabled = true; install.textContent = "Installing…"; activePanel = "output"; panelVisible = true; showPanel(); try { const result = await invoke("install_library", { cliPath: prefs.cli_path, name: lib.name, installDependencies: $("#library-deps").checked }); displayCliResult(result); install.textContent = result.success ? "Installed" : "Retry"; } catch (error) { install.disabled = false; install.textContent = "Install"; toast(String(error), "error"); } };
      row.append(title, sentence, install); results.append(row);
    }
  } catch (error) { results.textContent = String(error); }
}

function showPanel() {
  $(".app-frame").classList.toggle("panel-collapsed", !panelVisible);
  $("#bottom-panel").classList.toggle("hidden", !panelVisible);
  $("#panel-output").classList.toggle("hidden", activePanel !== "output");
  $("#panel-serial").classList.toggle("hidden", activePanel !== "serial");
  $("#panel-plotter").classList.toggle("hidden", activePanel !== "plotter");
  $("#panel-problems").classList.toggle("hidden", activePanel !== "problems");
  $("#serial-controls").hidden = activePanel !== "serial" && activePanel !== "plotter";
  $("#serial-entry").classList.toggle("hidden", activePanel !== "serial" || !serialActive);
  document.querySelectorAll(".panel-tab").forEach((button) => button.classList.toggle("active", button.dataset.panel === activePanel));
}

async function toggleSerial() {
  if (serialActive) {
    await invoke("serial_stop");
    return;
  }
  if (!prefs.port) { toast("Select a serial port in Board settings first.", "error"); return; }
  try {
    await invoke("serial_start", { cliPath: prefs.cli_path, port: prefs.port, fqbn: prefs.fqbn, baud: Number($("#serial-baud").value) });
    serialActive = true; $("#serial-connect").textContent = "Disconnect"; showPanel(); setStatus("Serial connected");
  } catch (error) { toast(String(error), "error"); }
}

function showCompletions() {
  if (mode === "hardcore") return;
  const cursor = editor.selectionStart;
  const before = editor.value.slice(0, cursor);
  const match = before.match(/[A-Za-z_]\w*$/);
  const prefix = match?.[0] || "";
  const start = cursor - prefix.length;
  completionItems = COMPLETIONS.filter(([label]) => label.toLowerCase().startsWith(prefix.toLowerCase())).slice(0, 20);
  completionIndex = 0;
  const menu = $("#completion-menu");
  menu.replaceChildren();
  for (let index = 0; index < completionItems.length; index++) {
    const [label, , detail] = completionItems[index];
    const row = document.createElement("div"); row.className = `completion-item ${index === 0 ? "selected" : ""}`;
    row.innerHTML = `<span>${escapeHtml(label)}</span><span class="completion-detail">${escapeHtml(detail)}</span>`;
    row.onclick = () => applyCompletion(index, start, cursor);
    menu.append(row);
  }
  if (!completionItems.length) { menu.innerHTML = `<div class="completion-item">No matching completions</div>`; }
  menu.classList.remove("hidden");
  menu.dataset.start = String(start); menu.dataset.cursor = String(cursor);
}

function applyCompletion(index, start = Number($("#completion-menu").dataset.start), cursor = Number($("#completion-menu").dataset.cursor)) {
  const choice = completionItems[index];
  if (!choice) return;
  editor.setRangeText(choice[1], start, cursor, "end");
  dirty = true; updateDirty(); renderEditor(); $("#completion-menu").classList.add("hidden"); editor.focus();
}

editor.addEventListener("input", () => { dirty = true; updateDirty(); renderEditor(); });
editor.addEventListener("scroll", () => { syntaxLayer.scrollTop = editor.scrollTop; syntaxLayer.scrollLeft = editor.scrollLeft; lineNumbers.scrollTop = editor.scrollTop; });
editor.addEventListener("keydown", (event) => {
  if ((event.ctrlKey || event.metaKey) && event.code === "Space") { event.preventDefault(); showCompletions(); return; }
  if (event.key === "Escape") $("#completion-menu").classList.add("hidden");
  if (!$("#completion-menu").classList.contains("hidden")) {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") { event.preventDefault(); completionIndex = (completionIndex + (event.key === "ArrowDown" ? 1 : completionItems.length - 1)) % Math.max(completionItems.length, 1); document.querySelectorAll(".completion-item").forEach((row, i) => row.classList.toggle("selected", i === completionIndex)); }
    if (event.key === "Enter" && completionItems.length) { event.preventDefault(); applyCompletion(completionIndex); }
  }
});

$("#verify").onclick = () => build(false);
$("#upload").onclick = () => build(true);
$("#open-file-tab").onclick = openSketch;
$("#close-file-action").onclick = closeCurrentFile;
$("#refresh-files").onclick = refreshFiles;
$("#add-file").onclick = async () => {
  const path = await invoke("choose_save_path");
  if (!path) return;
  const destination = path.match(/\.(ino|cpp|c|h|hpp)$/i) ? path : path + ".h";
  try { await invoke("save_file", { path: destination, content: "" }); await refreshFiles(); await openFile(destination); }
  catch (error) { toast(String(error), "error"); }
};
$("#complete-action").onclick = showCompletions;
$("#serial-connect").onclick = toggleSerial;
$("#serial-clear").onclick = () => { serialPanel.replaceChildren(); serialPanel._streamLine = null; plotSamples = []; plotTextBuffer = ""; drawPlotter(); };
$("#serial-send").onclick = async () => { const input = $("#serial-input"); if (!input.value) return; await invoke("serial_send", { line: input.value }); input.value = ""; };
$("#serial-input").onkeydown = (event) => { if (event.key === "Enter") $("#serial-send").click(); };
$("#toggle-panel").onclick = () => { panelVisible = !panelVisible; showPanel(); };
document.querySelectorAll(".panel-tab").forEach((button) => button.onclick = () => { activePanel = button.dataset.panel; panelVisible = true; showPanel(); });
document.querySelectorAll(".activity").forEach((button) => button.onclick = () => { document.querySelectorAll(".activity").forEach((item) => item.classList.toggle("active", item === button)); renderSidebar(button.dataset.view); });
$("#status-board").onclick = () => { document.querySelector('[data-view="boards"]').click(); };
$("#status-port").onclick = () => { document.querySelector('[data-view="boards"]').click(); };

$("#mode-select").onchange = async (event) => {
  setMode(event.target.value);
  if (mode === "normal") await scanBoards();
  if (mode === "hardcore") logLine("Hardcore mode: auto port selection and editor completions are disabled.", "system");
};
$("#status-mode").onclick = () => { $("#mode-select").focus(); $("#mode-select").click(); };
document.querySelectorAll("[data-node]").forEach((button) => button.onclick = () => addFlowNode(button.dataset.node));
$("#generate-flow").onclick = applyFlowToEditor;
$("#send-chat").onclick = sendLocalChat;
$("#chat-input").onkeydown = (event) => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); sendLocalChat(); } };
$("#toggle-live-code").onclick = () => { $("#live-code").classList.toggle("hidden"); updateLiveCode(); };
$("#close-live-code").onclick = () => $("#live-code").classList.add("hidden");
$("#apply-ai-code").onclick = () => {
  if (!latestAiCode.trim()) { toast("The assistant response did not include a fenced code block.", "error"); return; }
  editor.value = latestAiCode; dirty = true; renderEditor(); updateDirty(); setMode("normal"); toast("AI code applied to the sketch.");
};
$("#verify-ai-code").onclick = async () => { if (latestAiCode.trim()) { editor.value = latestAiCode; dirty = true; updateDirty(); } setMode("normal"); await build(false); };
$("#upload-ai-code").onclick = async () => { if (latestAiCode.trim()) { editor.value = latestAiCode; dirty = true; updateDirty(); } setMode("normal"); await build(true); };
$("#top-board").onchange = async (event) => { prefs.fqbn = event.target.value; await persistPreferences(); };
$("#top-port").onchange = async (event) => { prefs.port = event.target.value; await persistPreferences(); };
$("#quick-serial").onclick = () => openPanel("serial");
$("#quick-plotter").onclick = () => openPanel("plotter");
window.addEventListener("resize", drawPlotter);

const currentWindow = window.__TAURI__.window.getCurrentWindow();
$("#window-minimize").onclick = () => currentWindow.minimize();
$("#window-maximize").onclick = async () => {
  if (await currentWindow.isMaximized()) await currentWindow.unmaximize();
  else await currentWindow.maximize();
};
let allowWindowClose = false;
let closeGuardBusy = false;
currentWindow.onCloseRequested(async (event) => {
  if (allowWindowClose) return;
  event.preventDefault();
  if (closeGuardBusy) return;
  closeGuardBusy = true;
  try {
    if (dirty && !(await confirmDocumentSwitch())) return;
    allowWindowClose = true;
    await currentWindow.close();
  } finally { closeGuardBusy = false; }
});
$("#window-close").onclick = () => currentWindow.close();
$("#close-file-tab").onclick = closeCurrentFile;

async function saveAs() {
  const path = await invoke("choose_save_path");
  if (!path) return;
  const destination = /\.(ino|cpp|c|h|hpp)$/i.test(path) ? path : `${path}.ino`;
  try {
    await invoke("save_file", { path: destination, content: editor.value });
    if (/\.ino$/i.test(destination)) prefs.sketch_path = destination;
    setDocument(destination, editor.value); await persistPreferences(); await refreshFiles();
    logLine(`Saved ${destination}`, "success");
  } catch (error) { toast(String(error), "error"); }
}

function findNext() {
  const query = $("#find-query").value;
  if (!query) return;
  const start = editor.selectionEnd;
  const index = editor.value.toLowerCase().indexOf(query.toLowerCase(), start);
  if (index < 0) { toast("No more matches.", "system"); return; }
  editor.focus(); editor.setSelectionRange(index, index + query.length);
}

async function menuAction(action) {
  document.querySelectorAll(".menu-entry.open").forEach((entry) => entry.classList.remove("open"));
  if (action === "new") return newSketch();
  if (action === "open") return openSketch();
  if (action === "close-file") return closeCurrentFile();
  if (action === "save") return saveCurrent();
  if (action === "save-as") return saveAs();
  if (action === "settings") return document.querySelector('[data-view="settings"]').click();
  if (action === "quit") return currentWindow.close();
  if (action === "verify") return build(false);
  if (action === "upload") return build(true);
  if (action === "serial") return openPanel("serial");
  if (action === "plotter") return openPanel("plotter");
  if (action === "boards") return document.querySelector('[data-view="boards"]').click();
  if (action === "libraries") return document.querySelector('[data-view="libraries"]').click();
  if (action === "add-file") return $("#add-file").click();
  if (action === "format") return formatCurrentCode();
  if (action === "comment") return toggleComment();
  if (action === "complete") { editor.focus(); showCompletions(); return; }
  if (action === "select-all") { editor.focus(); editor.select(); return; }
  if (action === "undo" || action === "redo" || action === "cut" || action === "copy" || action === "paste") {
    editor.focus(); document.execCommand(action); return;
  }
  if (action === "find") { $("#find-widget").classList.remove("hidden"); $("#find-query").focus(); return; }
  if (action === "replace") { $("#find-widget").classList.remove("hidden"); $("#replace-query").focus(); return; }
  if (action === "folder") {
    if (!prefs.sketch_path) { toast("Open a sketch first.", "error"); return; }
    try { await invoke("open_sketch_folder", { sketchPath: prefs.sketch_path }); }
    catch (error) { toast(String(error), "error"); }
    return;
  }
  if (action === "export") {
    if (!prefs.sketch_path) { toast("Open a sketch first.", "error"); return; }
    try {
      await saveBeforeBuild();
      activePanel = "output"; panelVisible = true; showPanel();
      $("#panel-output").replaceChildren(); setStatus("Exporting binary…", "working");
      const result = await invoke("export_binary", { cliPath: prefs.cli_path, sketchPath: prefs.sketch_path, fqbn: prefs.fqbn });
      displayCliResult(result); setStatus(result.success ? "Binary exported" : "Export failed");
      toast(result.summary, result.success ? "success" : "error");
    } catch (error) { setStatus("Export failed"); logLine(String(error), "error"); toast(String(error), "error"); }
    return;
  }
  if (action === "getting-started") { toast("Open a sketch, choose its board and port, then Verify or Upload.", "system"); return; }
  if (action === "reference") { window.open("https://docs.arduino.cc/language-reference/", "_blank", "noopener"); return; }
  if (action === "about") { toast("Rustino 0.3 · Tauri Arduino Workbench", "system"); }
}

document.querySelectorAll(".menu-trigger").forEach((trigger) => trigger.onclick = (event) => {
  event.stopPropagation();
  const entry = trigger.closest(".menu-entry");
  const wasOpen = entry.classList.contains("open");
  document.querySelectorAll(".menu-entry.open").forEach((item) => item.classList.remove("open"));
  entry.classList.toggle("open", !wasOpen);
});
document.addEventListener("click", (event) => { if (!event.target.closest(".menu-entry")) document.querySelectorAll(".menu-entry.open").forEach((item) => item.classList.remove("open")); });
document.querySelectorAll(".menu-popover [data-action]").forEach((button) => button.onclick = () => menuAction(button.dataset.action));
$("#editor-menu").onclick = () => $("#editor-context").classList.toggle("hidden");
document.querySelectorAll("[data-context-action]").forEach((button) => button.onclick = async () => {
  $("#editor-context").classList.add("hidden");
  await menuAction(button.dataset.contextAction);
});
document.addEventListener("click", (event) => {
  if (!event.target.closest("#tabs")) $("#editor-context").classList.add("hidden");
});
$("#find-next").onclick = findNext;
$("#find-query").onkeydown = (event) => { if (event.key === "Enter") findNext(); };
$("#replace-one").onclick = () => {
  const selection = editor.value.slice(editor.selectionStart, editor.selectionEnd);
  if (!selection) return findNext();
  editor.setRangeText($("#replace-query").value, editor.selectionStart, editor.selectionEnd, "end");
  dirty = true; renderEditor(); updateDirty(); findNext();
};
$("#close-find").onclick = () => $("#find-widget").classList.add("hidden");

document.addEventListener("keydown", (event) => {
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "n") { event.preventDefault(); newSketch(); }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "o") { event.preventDefault(); openSketch(); }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "w") { event.preventDefault(); closeCurrentFile(); }
  if ((event.ctrlKey || event.metaKey) && event.key === ",") { event.preventDefault(); document.querySelector('[data-view="settings"]').click(); }
  if ((event.ctrlKey || event.metaKey) && event.key === "/") { event.preventDefault(); toggleComment(); }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") { event.preventDefault(); menuAction("find"); }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "h") { event.preventDefault(); menuAction("replace"); }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "t") { event.preventDefault(); formatCurrentCode(); }
  if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === "i") { event.preventDefault(); document.querySelector('[data-view="libraries"]').click(); }
  if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === "s") { event.preventDefault(); saveAs(); }
  if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === "m") { event.preventDefault(); openPanel("serial"); }
  if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === "l") { event.preventDefault(); openPanel("plotter"); }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") { event.preventDefault(); saveCurrent(); }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "r") { event.preventDefault(); build(false); }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "u") { event.preventDefault(); build(true); }
});

window.__TAURI__.event.listen("cli-output", (event) => {
  const payload = event.payload;
  appendStreamText(outputPanel, payload.line, /error|fatal/i.test(payload.line) ? "error" : /warning/i.test(payload.line) ? "warn" : "");
});
window.__TAURI__.event.listen("ai-chunk", (event) => {
  const payload = event.payload;
  if (payload.requestId !== activeAiRequest || !pendingAiBody) return;
  activeAiText += payload.text;
  pendingAiBody.textContent = activeAiText;
  const history = $("#chat-history"); history.scrollTop = history.scrollHeight;
  const code = extractCode(activeAiText);
  if (code) { latestAiCode = code; $("#live-code-preview").textContent = code; }
});
window.__TAURI__.event.listen("serial-data", (event) => {
  appendStreamText(serialPanel, event.payload);
  searchPlotData(event.payload);
});
window.__TAURI__.event.listen("serial-ended", () => {
  serialActive = false;
  if ($("#serial-connect").textContent !== "Connecting after upload…") $("#serial-connect").textContent = "Connect";
  showPanel();
  if (!$("#status-state").dataset.state) setStatus("Serial disconnected");
});

async function boot() {
  try {
    const snapshot = await invoke("get_app_state");
    prefs = snapshot.preferences;
    files = snapshot.files;
    setDocument(prefs.sketch_path || "", snapshot.content ?? "");
    updateBoardStatus();
    setMode(mode);
    renderSidebar("explorer");
    logLine("Rustino is ready. Open a sketch, select its board and serial port, then Verify or Upload.");
    if (snapshot.startup_message) logLine(snapshot.startup_message, "warn");
    await fillPorts();
    if (mode === "normal") await scanBoards();
  } catch (error) {
    setDocument("", "");
    renderSidebar("explorer");
    logLine(`Startup failed: ${String(error)}`, "error");
  }
}

boot();
