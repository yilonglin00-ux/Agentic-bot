#!/usr/bin/env bash
# bauen.sh – baut Noki.app aus dem vorhandenen Tauri-Projekt.
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
# Ergebnis: desktop/Noki.app
#
# Ist die Tauri-CLI spaeter doch vorhanden, ist `cargo tauri build` der
# offizielle Weg (Signierung, DMG, Updater). Dieses Skript ersetzt sie nicht.
set -eu

_here="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
_projekt="$(cd "$_here/.." && pwd)"
cd "$_here/src-tauri"

# Vorhandene projektlokale Buildwerkzeuge wiederverwenden (keine Installation/Downloads).
if [ -d "$_projekt/.local/cargo/registry" ]; then export CARGO_HOME="$_projekt/.local/cargo"; fi
if [ -x "$_projekt/.local/cmake/cmake/data/bin/cmake" ]; then export CMAKE="$_projekt/.local/cmake/cmake/data/bin/cmake"; fi
mkdir -p "$_projekt/.local/tmp"
export TMPDIR="$_projekt/.local/tmp"

profil="debug"
cargoargs=()
schnell=0
case "${1:-}" in
  --release) profil="release"; cargoargs+=(--release) ;;
  # --fast: derselbe kanonische Build, nur ohne die teuren Wiederholungen.
  # Kein clean und kein Neuaufbau des Bundles; Cargo bleibt inkrementell.
  --fast) schnell=1 ;;
esac

# -j 2 bewusst: ein voller paralleler Cargo-Lauf hat auf diesem Rechner
# schon zu Speicherproblemen gefuehrt.
echo "==> cargo build ($profil)"
cargo build --locked -j 2 "${cargoargs[@]}"

bin="target/$profil/app"
[ -x "$bin" ] || { echo "Binaerdatei fehlt: $bin" >&2; exit 1; }

app="$_here/Noki.app"
echo "==> Programmpaket: $app"
if [ "$schnell" = "1" ] && [ -d "$app/Contents/MacOS" ]; then
  echo "==> Fast: bestehendes Bundle wird aktualisiert"
else
  rm -rf "$app"
fi
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

# Erst in eine Nebendatei schreiben, dann atomar tauschen: die laufende App
# bleibt bis zum letzten Moment benutzbar.
cp "$bin" "$app/Contents/MacOS/Noki.neu" && mv -f "$app/Contents/MacOS/Noki.neu" "$app/Contents/MacOS/Noki"
chmod +x "$app/Contents/MacOS/Noki"
# Dieselbe CLI wird sowohl von `noki code` als auch vom Desktop-Code-Tab benutzt.
# Sie bleibt ein normaler Terminalprozess; die Desktop-App rendert keinen zweiten Code-Chat.
cli="target/$profil/noki"
[ -x "$cli" ] || { echo "Code-CLI fehlt: $cli" >&2; exit 1; }
mkdir -p "$app/Contents/Helpers"
cp "$cli" "$app/Contents/Helpers/noki.neu" && mv -f "$app/Contents/Helpers/noki.neu" "$app/Contents/Helpers/noki"
chmod +x "$app/Contents/Helpers/noki"
# `noki code` im Shell-PATH: nur ein Symlink auf den Repo-Wrapper, nie eine Kopie.
if [ "${NOKI_SKIP_CLI_LINK:-0}" != "1" ] && [ -d "$HOME/.local/bin" ]; then
  link="$HOME/.local/bin/noki"
  if [ ! -e "$link" ] || [ -L "$link" ]; then ln -sfn "$_projekt/noki" "$link"; fi
fi
# Lokale Spracheingabe (On-Device, Speech.framework): eigener kleiner App-Helper.
# TCC akzeptiert Speech-/Mikrofon-Usage-Descriptions nur aus einem echten Bundle;
# ein alleinstehendes Mach-O mit eingebettetem __info_plist wird auf aktuellem macOS beendet.
stt_src="$_here/stt/noki-stt.swift"
stt_app="$_projekt/.local/stt/NokiSpeech.app"
stt_bin="$stt_app/Contents/MacOS/noki-stt"
if [ -f "$stt_src" ] && command -v swiftc >/dev/null 2>&1; then
  mkdir -p "$stt_app/Contents/MacOS"
  # Ein vorhandener aktueller Legacy-Build kann ohne Neu-Kompilierung in das
  # korrekte Bundle migriert werden; die Swift-Quelle bleibt die Autorität.
  stt_legacy="$_projekt/.local/stt/noki-stt"
  if [ ! -x "$stt_bin" ] && [ -x "$stt_legacy" ] && [ ! "$stt_src" -nt "$stt_legacy" ]; then
    cp "$stt_legacy" "$stt_bin"
  fi
  if [ ! -x "$stt_bin" ] || [ "$stt_src" -nt "$stt_bin" ]; then
    echo "==> Spracheingabe (swiftc)"
    swiftc -O -swift-version 5 -o "$stt_bin" "$stt_src" -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$_here/stt/Info.plist"
  fi
  cp "$_here/stt/Info.plist" "$stt_app/Contents/Info.plist"
  codesign --force -s - "$stt_app" >/dev/null 2>&1 || true
  mkdir -p "$app/Contents/Helpers"
  rm -rf "$app/Contents/Helpers/NokiSpeech.app"
  ditto "$stt_app" "$app/Contents/Helpers/NokiSpeech.app"
fi

# Photos-Adapter (PhotoKit): eigener kleiner App-Helper fuer macOS Fotos-Mediathek
photos_src="$_here/photos/noki-photos.swift"
photos_app="$_projekt/.local/photos/NokiPhotos.app"
photos_bin="$photos_app/Contents/MacOS/noki-photos"
if [ -f "$photos_src" ] && command -v swiftc >/dev/null 2>&1; then
  mkdir -p "$photos_app/Contents/MacOS"
  if [ ! -x "$photos_bin" ] || [ "$photos_src" -nt "$photos_bin" ] || [ "$_here/photos/Info.plist" -nt "$photos_bin" ] || [ "$_here/photos/entitlements.plist" -nt "$photos_bin" ]; then
    echo "==> Fotos-Adapter (swiftc)"
    swiftc -O -swift-version 5 -o "$photos_bin" "$photos_src" -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$_here/photos/Info.plist"
  fi
  cp "$_here/photos/Info.plist" "$photos_app/Contents/Info.plist"
  codesign --force --entitlements "$_here/photos/entitlements.plist" -s - "$photos_app" >/dev/null 2>&1 || true
  mkdir -p "$app/Contents/Helpers"
  rm -rf "$app/Contents/Helpers/NokiPhotos.app"
  ditto "$photos_app" "$app/Contents/Helpers/NokiPhotos.app"
  mkdir -p "$_here/target/photos-bin"
  rm -rf "$_here/target/photos-bin/NokiPhotos.app"
  ditto "$photos_app" "$_here/target/photos-bin/NokiPhotos.app"
fi

# Arbeitsplatz-Vorschau (ScreenCaptureKit): eigener kleiner App-Helper.
# Wie bei Sprache und Fotos verlangt TCC ein echtes Bundle - die Freigabe
# "Bildschirmaufnahme" haengt an einer Bundle-Identitaet, nicht an einer
# nackten Mach-O-Datei.
schirm_src="$_here/schirm/main.swift"
schirm_app="$_projekt/.local/schirm/NokiSchirm.app"
schirm_bin="$schirm_app/Contents/MacOS/nokischirm"
if [ -f "$schirm_src" ] && command -v swiftc >/dev/null 2>&1; then
  mkdir -p "$schirm_app/Contents/MacOS"
  if [ ! -x "$schirm_bin" ] || [ "$schirm_src" -nt "$schirm_bin" ] || [ "$_here/schirm/Info.plist" -nt "$schirm_bin" ]; then
    echo "==> Arbeitsplatz-Vorschau (swiftc)"
    # CLT 26.6 can temporarily ship a newer Swift compiler than its 26.x
    # SDK swiftinterfaces.  The installed 15.4 SDK contains every API used
    # by this helper and remains compiler-compatible, so prefer it when
    # present; NOKI_SWIFT_SDK can override this explicitly.
    swift_sdk="${NOKI_SWIFT_SDK:-/Library/Developer/CommandLineTools/SDKs/MacOSX15.4.sdk}"
    swift_sdk_args=()
    [ -d "$swift_sdk" ] && swift_sdk_args=(-sdk "$swift_sdk")
    CLANG_MODULE_CACHE_PATH="$_projekt/.local/tmp/swift-module-cache" \
    swiftc -O "${swift_sdk_args[@]}" -o "$schirm_bin" "$schirm_src" -framework ScreenCaptureKit -framework CoreImage -framework AppKit \
      -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$_here/schirm/Info.plist"
  fi
  cp "$_here/schirm/Info.plist" "$schirm_app/Contents/Info.plist"
  codesign --force -s - "$schirm_app" >/dev/null 2>&1 || true
  mkdir -p "$app/Contents/Helpers"
  rm -rf "$app/Contents/Helpers/NokiSchirm.app"
  ditto "$schirm_app" "$app/Contents/Helpers/NokiSchirm.app"
fi

# Virtueller Arbeitsplatz: private CoreGraphics API is isolated in one
# runtime-checked helper.  Its process lifetime owns the virtual display.
virtual_src="$_here/virtual-display/main.m"
virtual_bin="$_here/virtual-display/noki-virtual-display"
if [ -f "$virtual_src" ] && command -v clang >/dev/null 2>&1; then
  if [ ! -x "$virtual_bin" ] || [ "$virtual_src" -nt "$virtual_bin" ]; then
    echo "==> Virtueller Arbeitsplatz (clang)"
    clang -fobjc-arc -O "$virtual_src" -o "$virtual_bin" \
      -framework Foundation -framework CoreGraphics -framework AppKit -framework IOKit
  fi
  cp "$virtual_bin" "$app/Contents/Helpers/noki-virtual-display.neu"
  mv -f "$app/Contents/Helpers/noki-virtual-display.neu" "$app/Contents/Helpers/noki-virtual-display"
  chmod +x "$app/Contents/Helpers/noki-virtual-display"
fi

# Chat und Code laufen über den lokalen llama.cpp-Router; Ollama bleibt als Rollback.
echo "==> Modelle: llama.cpp-Router lokal (GGUF-Gewichte nicht ins App-Bundle kopiert)"

cp "src-tauri/icons/icon.icns" "$app/Contents/Resources/icon.icns" 2>/dev/null \
  || cp "icons/icon.icns" "$app/Contents/Resources/icon.icns"

# Normale macOS-App: Dock und Programm-Menue bleiben sichtbar. Das passt
# zur ActivationPolicy::Regular und macht Close/Reopen systemtypisch.
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>              <string>Noki</string>
  <key>CFBundleDisplayName</key>       <string>Noki</string>
  <key>CFBundleExecutable</key>        <string>Noki</string>
  <key>CFBundleIdentifier</key>        <string>com.noki.desktop</string>
  <key>CFBundleIconFile</key>          <string>icon</string>
  <key>CFBundlePackageType</key>       <string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key>           <string>__NOKI_BUILD__</string>
  <key>LSMinimumSystemVersion</key>    <string>10.15</string>
  <key>NSHighResolutionCapable</key>   <true/>
  <key>LSUIElement</key>               <false/>
  <key>NSMicrophoneUsageDescription</key><string>Noki nutzt das Mikrofon nur, während du in Ask Noki die Spracheingabe aktivierst. Audio bleibt auf diesem Mac.</string>
  <key>NSSpeechRecognitionUsageDescription</key><string>Noki wandelt deine Sprache ausschließlich auf diesem Mac in Text um (On-Device, keine Cloud).</string>
  <key>NSAppleEventsUsageDescription</key><string>Noki öffnet für Arbeitsplatz-Fokus ein neues Fenster der gewählten App auf deinem aktuellen Schreibtisch, ohne zu einem anderen Schreibtisch zu springen.</string>
  <key>NSPhotoLibraryUsageDescription</key><string>Noki zeigt Fotos aus deiner Mediathek in der Galerie an, damit du sie auswählen und an Gespräche anhängen kannst.</string>
</dict>
</plist>
PLIST
# Eindeutige Build-Kennung (sichtbar in Finder "Informationen" und im Log).
build_stempel="$(date +%Y%m%d.%H%M%S)"
sed -i '' "s/__NOKI_BUILD__/0.1.0.$build_stempel/" "$app/Contents/Info.plist"

# Ad-hoc-Signatur: ohne sie verweigert macOS einer frisch kopierten App
# regelmaessig den Start. Das ist keine Entwicklerzertifikat-Signatur und
# ersetzt keine Notarisierung — es macht das Paket lokal startbar.
# Feste Designated Requirement (Bundle-ID statt CDHash): sonst passt die
# Bildschirmaufnahme-Freigabe (TCC) nach JEDEM Neubau nicht mehr, obwohl
# Noki in den Systemeinstellungen weiter als erlaubt erscheint.
ident=$(sed -n 's/.*"identifier": *"\([^"]*\)".*/\1/p' "$_here/src-tauri/tauri.conf.json" | head -1)
anf=(); [ -n "$ident" ] && anf=(-r="designated => identifier \"$ident\"")
ent_opt=()
[ -f "$_here/entitlements.plist" ] && ent_opt=(--entitlements "$_here/entitlements.plist")
if command -v codesign >/dev/null 2>&1; then
  codesign --force --sign - "${ent_opt[@]}" "${anf[@]}" "$app" >/dev/null 2>&1 \
    && echo "==> ad-hoc signiert" \
    || echo "==> codesign fehlgeschlagen (App startet ggf. trotzdem)"
fi

# ---------------------------------------------------------------------
# Kanonisches Update: GENAU dieses Bundle ist Noki. Danach laeuft genau eine
# Instanz, und zwar aus diesem Pfad. Nutzerdaten (Chats, Projekte, .noki,
# Einstellungen, Timelines) liegen ausserhalb des Bundles und bleiben
# unberuehrt. Ueberspringen: NOKI_KEIN_NEUSTART=1
# ---------------------------------------------------------------------
lsreg=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
# Ein alter Kopie-Stand in /Applications wuerde von Spotlight/Launchpad
# gestartet: reversibel beiseitelegen (nie loeschen) und auf das kanonische
# Bundle verweisen.
if [ -d /Applications/Noki.app ] && [ ! -L /Applications/Noki.app ]; then
  ablage="$_projekt/.local/alte-bundles"; mkdir -p "$ablage"
  ziel="$ablage/Noki-$(stat -f %Sm -t %Y%m%d-%H%M%S /Applications/Noki.app/Contents/MacOS/Noki 2>/dev/null || echo alt).app"
  if mv /Applications/Noki.app "$ziel" 2>/dev/null; then
    "$lsreg" -u "$ziel" >/dev/null 2>&1 || true
    ln -s "$app" /Applications/Noki.app && echo "==> /Applications/Noki.app -> $app (alter Stand: $ziel)"
  else
    echo "==> WARNUNG: /Applications/Noki.app (alter Stand) liess sich nicht verschieben"
  fi
fi
# Registrierte, nicht mehr existierende Noki-Bundles abmelden.
"$lsreg" -dump 2>/dev/null | sed -n 's/^path: *\(.*Noki\.app\) (0x.*/\1/p' | sort -u | while read -r p; do
  [ -d "$p" ] || "$lsreg" -u "$p" >/dev/null 2>&1 || true
done
"$lsreg" -f "$app" >/dev/null 2>&1 || true

exe="$app/Contents/MacOS/Noki"
sha=$(shasum -a 256 "$exe" | cut -c1-16)
version=$(/usr/libexec/PlistBuddy -c "Print CFBundleVersion" "$app/Contents/Info.plist")
printf '{"path":"%s","version":"%s","sha256":"%s","built":"%s"}\n' "$app" "$version" "$sha" "$(date '+%Y-%m-%d %H:%M:%S')" > "$_projekt/.local/noki-build.json"
echo "==> Build $version sha256=$sha"

if [ "${NOKI_KEIN_NEUSTART:-0}" != "1" ]; then
  laufend=$(pgrep -f "/Noki.app/Contents/MacOS/Noki" || true)
  if [ -n "$laufend" ]; then
    echo "==> laufendes Noki beenden ($laufend)"
    osascript -e 'tell application id "com.noki.desktop" to quit' >/dev/null 2>&1 || true
    for _ in $(seq 1 40); do pgrep -f "/Noki.app/Contents/MacOS/Noki" >/dev/null || break; sleep 0.25; done
    pkill -f "/Noki.app/Contents/MacOS/Noki" 2>/dev/null || true
    sleep 0.5
  fi
  open "$app"
  for _ in $(seq 1 40); do pgrep -f "^$exe" >/dev/null && break; sleep 0.25; done
  laeuft=$(pgrep -f "/Noki.app/Contents/MacOS/Noki" || true)
  echt=$(pgrep -f "^$exe" || true)
  if [ -n "$echt" ] && [ "$laeuft" = "$echt" ]; then
    echo "==> laeuft: PID $echt aus $exe (Build $version)"
  else
    echo "==> WARNUNG: laufende Noki-Prozesse ($laeuft) passen nicht zu $exe" >&2
  fi
fi

echo "==> fertig: $app"
