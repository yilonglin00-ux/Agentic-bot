#!/bin/bash
# Noki Companion for VS Code - local install / uninstall.
#   ./installieren.sh            build the VSIX locally and install it
#   ./installieren.sh entfernen  uninstall it again (nothing else is touched)
# The extension only opens a user-only Unix socket for the local Noki app.
set -euo pipefail
hier="$(cd "$(dirname "$0")" && pwd)"
code_cli="${CODE_CLI:-}"
for k in "$code_cli" "$(command -v code || true)" "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"; do
  if [ -n "$k" ] && [ -x "$k" ]; then code_cli="$k"; break; fi
done
[ -x "$code_cli" ] || { echo "VS Code CLI nicht gefunden" >&2; exit 1; }
if [ "${1:-}" = "entfernen" ]; then
  "$code_cli" --uninstall-extension noki.noki-companion || true
  rm -rf "$HOME/Library/Application Support/com.noki.desktop/vsc"
  echo "Noki Companion entfernt."
  exit 0
fi
version=$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' "$hier/package.json" | head -1)
bau="$(mktemp -d)"; trap 'rm -rf "$bau"' EXIT
mkdir -p "$bau/extension"
cp "$hier/package.json" "$hier/extension.js" "$bau/extension/"
cat > "$bau/[Content_Types].xml" <<'XML'
<?xml version="1.0" encoding="utf-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension=".json" ContentType="application/json"/><Default Extension=".js" ContentType="application/javascript"/><Default Extension=".vsixmanifest" ContentType="text/xml"/></Types>
XML
cat > "$bau/extension.vsixmanifest" <<XML
<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011" xmlns:d="http://schemas.microsoft.com/developer/vsx-schema-design/2011">
  <Metadata>
    <Identity Language="en-US" Id="noki-companion" Version="$version" Publisher="noki"/>
    <DisplayName>Noki Companion</DisplayName>
    <Description xml:space="preserve">Local-only bridge for the Noki app.</Description>
    <Properties><Property Id="Microsoft.VisualStudio.Code.Engine" Value="^1.80.0"/></Properties>
  </Metadata>
  <Installation><InstallationTarget Id="Microsoft.VisualStudio.Code"/></Installation>
  <Dependencies/>
  <Assets><Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true"/></Assets>
</PackageManifest>
XML
vsix="$bau/noki-companion-$version.vsix"
(cd "$bau" && zip -qr "$vsix" "[Content_Types].xml" extension.vsixmanifest extension)
"$code_cli" --install-extension "$vsix" --force
echo "Noki Companion $version installiert. Entfernen: $0 entfernen"
