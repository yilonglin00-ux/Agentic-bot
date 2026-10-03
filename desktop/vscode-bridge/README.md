# Noki Companion (VS Code)

Lets the local Noki app type into a VS Code window that sits on another
Desktop (Miniatur remote control). macOS/Electron discards key events posted
to an inactive VS Code, and activating VS Code would move you to its Desktop;
the extension applies the keys through the VS Code API instead.

- **Editor:** text, Enter, Backspace, Delete, Tab, arrows, Home/End,
  Cmd+A/C/X/V/S/Z, Cmd+Shift+Z - at the real cursor/selection of the window's
  active editor.
- **Integrated terminal:** text, Enter, Backspace, arrows, Tab, Ctrl+letter,
  Cmd+V/C - sent to the window's active terminal (`Terminal.sendText`).

## Security
- One Unix domain socket per VS Code window in
  `~/Library/Application Support/com.noki.desktop/vsc` (directory 0700,
  socket 0600). No TCP, no network, no telemetry.
- Every request must carry a random per-session token (published only in a
  0600 file next to the socket, regenerated on every start, never logged).
- Answers contain no document content except, for Noki's own verification,
  the text of the cursor line.

## Install / uninstall
```
./installieren.sh            # builds the VSIX locally and installs it
./installieren.sh entfernen  # uninstalls it and removes the socket directory
```
No VS Code setting is changed and no other extension is touched. A running
VS Code picks the extension up without a restart.

## Live picture (optional)
An occluded VS Code window paints nothing (Chromium occlusion throttling), so
the Miniatur shows its last frame until you look at it. To keep it painting,
quit VS Code and start it with `./vscode-live-starten.sh`
(`--disable-backgrounding-occluded-windows --disable-renderer-backgrounding`).
