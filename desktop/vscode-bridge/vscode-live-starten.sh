#!/bin/bash
# Optional: start VS Code so that its windows keep painting while they sit on
# a Desktop you are not looking at - then the Noki Miniatur shows them live.
# Without these Chromium switches an occluded VS Code window paints nothing
# (macOS/Chromium occlusion throttling); typing via Noki Companion still
# works, the Miniatur just shows the last painted frame.
# Quit VS Code first (Cmd+Q); this changes no setting and installs nothing.
exec open -a "Visual Studio Code" --args --disable-backgrounding-occluded-windows --disable-renderer-backgrounding "$@"
