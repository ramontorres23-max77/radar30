#!/bin/bash
# Build a double-clickable macOS app bundle around the radar30 CLI.
# Usage: scripts/build-macos-app.sh [path-to-binary] [output.app]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${1:-$ROOT/target/release/radar30}"
OUT="${2:-$ROOT/dist/Radar30.app}"

if [[ ! -x "$BIN" ]]; then
  echo "building release binary..."
  cargo build --release --manifest-path "$ROOT/Cargo.toml"
fi
if [[ ! -x "$BIN" ]]; then
  echo "error: binary not found at $BIN" >&2
  exit 1
fi

APP_NAME="Radar30"
VERSION="$(grep -m1 '^version = ' "$ROOT/Cargo.toml" | cut -d'"' -f2)"

rm -rf "$OUT"
mkdir -p "$OUT/Contents/MacOS" "$OUT/Contents/Resources"
cp "$BIN" "$OUT/Contents/MacOS/radar30"
cp "$ROOT/skills/radar30/SKILL.md" "$OUT/Contents/Resources/SKILL.md"
cp "$ROOT/README.md" "$OUT/Contents/Resources/README.md"

cat > "$OUT/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>radar30</string>
  <key>CFBundleIdentifier</key><string>com.radar30.app</string>
  <key>CFBundleExecutable</key><string>radar30-launcher</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
</dict>
</plist>
PLIST

cat > "$OUT/Contents/MacOS/radar30-launcher" <<'LAUNCHER'
#!/bin/bash
# Thin macOS launcher: asks what to do, then runs the bundled radar30 CLI in Terminal.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="$HERE/radar30"

escape_apple() {
  local s="$1"
  s="${s//\\/\\\\}"
  s="${s//\"/\\\"}"
  printf '%s' "$s"
}

run_in_terminal() {
  local cmd
  cmd="$(escape_apple "$1")"
  osascript -e 'tell application "Terminal" to activate' \
            -e "tell application \"Terminal\" to do script \"$cmd\""
}

# Test hook: no GUI, prints the command that would run.
if [[ "${RADAR30_APP_TEST:-}" == "1" ]]; then
  choice="${RADAR30_APP_CHOICE:-1}"
  topic="${RADAR30_APP_TOPIC:-Rust}"
  case "$choice" in
    1) printf 'CMD: "%s" research "%s" --depth quick --emit compact\n' "$BIN" "$topic" ;;
    2) printf 'CMD: "%s" company "%s" --emit compact\n' "$BIN" "$topic" ;;
    3) printf 'CMD: "%s" doctor\n' "$BIN" ;;
    *) printf 'CMD: open -a Terminal\n' ;;
  esac
  exit 0
fi

MENU=$'Pesquisa rápida (30 dias)\nCompany Intel\nDoctor / status\nAbrir Terminal'
selected="$(osascript \
  -e "choose from list {\"Pesquisa rápida (30 dias)\", \"Company Intel\", \"Doctor / status\", \"Abrir Terminal\"}" \
  -e "with prompt \"radar30 — o que quer fazer?\" default items {\"Pesquisa rápida (30 dias)\"}")" || exit 0
[[ "$selected" == "false" ]] && exit 0

ask() {
  osascript -e "display dialog \"$1\" default answer \"$2\" buttons {\"Cancelar\", \"OK\"} default button \"OK\"" \
            -e 'text returned of result' 2>/dev/null || true
}

case "$selected" in
  "Pesquisa rápida (30 dias)")
    topic="$(ask 'Tema ou pergunta:' 'Rust')"
    [[ -z "$topic" ]] && exit 0
    topic="${topic//\"/\'}"
    run_in_terminal "\"$BIN\" research \"$topic\" --depth quick --emit compact"
    ;;
  "Company Intel")
    company="$(ask 'Empresa:' 'Apple')"
    [[ -z "$company" ]] && exit 0
    company="${company//\"/\'}"
    run_in_terminal "\"$BIN\" company \"$company\" --emit compact"
    ;;
  "Doctor / status")
    run_in_terminal "\"$BIN\" doctor"
    ;;
  "Abrir Terminal")
    osascript -e 'tell application "Terminal" to activate' -e 'tell application "Terminal" to do script ""'
    ;;
esac
LAUNCHER

chmod +x "$OUT/Contents/MacOS/radar30-launcher"
echo "App bundle: $OUT"
