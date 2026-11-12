$ErrorActionPreference = 'Stop'

$required = @(
  'resources/ollama-windows/ollama.exe',
  'resources/ollama/models/manifests/registry.ollama.ai/library/qwen2.5-coder/1.5b',
  'resources/arduino-cli-windows/arduino-cli.exe',
  'icons/rustino.ico'
)
foreach ($path in $required) {
  if (-not (Test-Path $path)) {
    throw "Required bundled Windows asset is missing: $path"
  }
}

cargo tauri build --bundles nsis
if ($LASTEXITCODE -ne 0) {
  throw "Tauri Windows installer build failed with exit code $LASTEXITCODE"
}

$exe = 'target/release/rustino.exe'
$installer = Get-ChildItem 'target/release/bundle/nsis' -Filter '*.exe' |
  Sort-Object LastWriteTime -Descending |
  Select-Object -First 1
if (-not (Test-Path $exe) -or -not $installer) {
  throw 'Build finished without rustino.exe and the NSIS setup executable.'
}
Write-Host "App:       $exe"
Write-Host "Installer: $($installer.FullName)"
