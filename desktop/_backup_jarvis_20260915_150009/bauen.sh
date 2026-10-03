#!/usr/bin/env bash
# bauen.sh – baut JARVIS.app aus dem vorhandenen Tauri-Projekt.
#
# Warum dieses Skript und nicht `cargo tauri build`?
# Die Tauri-CLI ist hier nicht installiert, und sie nachzuinstallieren waere
# eine neue Abhaengigkeit fuer genau einen Schritt: das Programmpaket
# zusammenzustellen. Ein macOS-Programmpaket ist ein Verzeichnis mit einer
# Info.plist, dem Programm und einem Symbol — genau das macht dieses Skript,
# aus derselben Binaerdatei, die cargo ohnehin baut.
#
#   ./bauen.sh            Debug-Paket (schnell)
#   ./bauen.sh --release   Release-Paket (langsamer, kleiner)
#
# Ergebnis: desktop/JARVIS.app
#
# Ist die Tauri-CLI spaeter doch vorhanden, ist `cargo tauri build` der
# offizielle Weg (Signierung, DMG, Updater). Dieses Skript ersetzt sie nicht.
set -eu

_here="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
cd "$_here/src-tauri"

profil="debug"
cargoargs=()
if [ "${1:-}" = "--release" ]; then
  profil="release"
  cargoargs+=(--release)
fi

# -j 2 bewusst: ein voller paralleler Cargo-Lauf hat auf diesem Rechner
# schon zu Speicherproblemen gefuehrt.
echo "==> cargo build ($profil)"
cargo build -j 2 "${cargoargs[@]}"

bin="target/$profil/app"
[ -x "$bin" ] || { echo "Binaerdatei fehlt: $bin" >&2; exit 1; }

app="$_here/JARVIS.app"
echo "==> Programmpaket: $app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

cp "$bin" "$app/Contents/MacOS/JARVIS"
chmod +x "$app/Contents/MacOS/JARVIS"
cp "src-tauri/icons/icon.icns" "$app/Contents/Resources/icon.icns" 2>/dev/null \
  || cp "icons/icon.icns" "$app/Contents/Resources/icon.icns"

# LSUIElement passt zur ActivationPolicy::Accessory in lib.rs: kein
# Dock-Symbol, kein Programm-Menue. Der Weg zur Bedienung ist die
# Menueleiste (Abschnitt 16 der Vorgabe).
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>              <string>JARVIS</string>
  <key>CFBundleDisplayName</key>       <string>JARVIS</string>
  <key>CFBundleExecutable</key>        <string>JARVIS</string>
  <key>CFBundleIdentifier</key>        <string>com.jarvis.desktop</string>
  <key>CFBundleIconFile</key>          <string>icon</string>
  <key>CFBundlePackageType</key>       <string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key>           <string>0.1.0</string>
  <key>LSMinimumSystemVersion</key>    <string>10.15</string>
  <key>NSHighResolutionCapable</key>   <true/>
  <key>LSUIElement</key>               <true/>
</dict>
</plist>
PLIST

# Ad-hoc-Signatur: ohne sie verweigert macOS einer frisch kopierten App
# regelmaessig den Start. Das ist keine Entwicklerzertifikat-Signatur und
# ersetzt keine Notarisierung — es macht das Paket lokal startbar.
# Feste Designated Requirement (Bundle-ID statt CDHash): sonst passt die
# Bildschirmaufnahme-Freigabe (TCC) nach JEDEM Neubau nicht mehr, obwohl
# JARVIS in den Systemeinstellungen weiter als erlaubt erscheint.
ident=$(sed -n 's/.*"identifier": *"\([^"]*\)".*/\1/p' "$_here/src-tauri/tauri.conf.json" | head -1)
anf=(); [ -n "$ident" ] && anf=(-r="designated => identifier \"$ident\"")
if command -v codesign >/dev/null 2>&1; then
  codesign --force --sign - "${anf[@]}" "$app" >/dev/null 2>&1 \
    && echo "==> ad-hoc signiert" \
    || echo "==> codesign fehlgeschlagen (App startet ggf. trotzdem)"
fi

echo "==> fertig: $app"
echo "    starten:  open '$app'"
