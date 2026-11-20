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

# A .command file is opened through LaunchServices, so no Automation permission
# dialog (osascript "tell application Terminal") is needed.
cat > "$OUT/Contents/Resources/radar30-menu.command" <<'MENU'
#!/bin/bash
# Interactive launcher menu, executed by Terminal.app when the app is opened.
set -u
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="$HERE/../MacOS/radar30"

while true; do
  printf '\n========== radar30 ==========\n'
  printf '1) Pesquisa rapida (30 dias)\n'
  printf '2) Company Intel (empresa)\n'
  printf '3) Doctor / status\n'
  printf '4) Sair\n'
  printf '%s\n' '-----------------------------'
  read -rp 'Escolha [1-4]: ' option || exit 0
  case "$option" in
    1)
      read -rp 'Tema ou pergunta: ' topic
      [[ -z "$topic" ]] && continue
      echo
      "$BIN" research "$topic" --depth quick --emit compact
      ;;
    2)
      read -rp 'Empresa: ' company
      [[ -z "$company" ]] && continue
      echo
      "$BIN" company "$company" --emit compact
      ;;
    3)
      echo
      "$BIN" doctor
      ;;
    4|q|Q|'')
      exit 0
      ;;
    *)
      echo 'Opcao invalida.'
      ;;
  esac
  printf '\n'
  read -rp 'Enter para continuar... ' _ || exit 0
done
MENU
chmod +x "$OUT/Contents/Resources/radar30-menu.command"

cat > "$OUT/Contents/MacOS/radar30-launcher" <<'LAUNCHER'
#!/bin/bash
# Hands off to a .command file; Terminal is opened by LaunchServices, not AppleEvents.
set -u
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MENU="$HERE/../Resources/radar30-menu.command"
LOG="$HOME/Library/Logs/radar30-launcher.log"

mkdir -p "$(dirname "$LOG")"
printf '[%s] launcher started (bundle=%s)\n' "$(date -u +%FT%TZ)" "$HERE" >>"$LOG" 2>&1

if [[ "${RADAR30_APP_TEST:-}" == "1" ]]; then
  printf 'CMD: open %s\n' "$MENU"
  exit 0
fi

if ! open "$MENU" >>"$LOG" 2>&1; then
  printf '[%s] open failed\n' "$(date -u +%FT%TZ)" >>"$LOG" 2>&1
  exit 1
fi
LAUNCHER
chmod +x "$OUT/Contents/MacOS/radar30-launcher"
echo "App bundle: $OUT"
