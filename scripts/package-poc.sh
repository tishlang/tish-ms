#!/usr/bin/env bash
# Pack the benchmark build for bench.yml: dist/launcher-poc.zip with launcher-poc.exe and the
# Windows App SDK bootstrapper the WinUI host loads. Then attach it to a release:
#   gh release create poc-<n> dist/launcher-poc.zip --prerelease --title "Host benchmark build"
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
bash "$ROOT/scripts/build-example.sh" launcher-poc
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
curl -fsSL -o "$WORK/f.zip" https://api.nuget.org/v3-flatcontainer/microsoft.windowsappsdk.foundation/2.3.12/microsoft.windowsappsdk.foundation.2.3.12.nupkg
unzip -qo "$WORK/f.zip" 'runtimes/win-x64/native/Microsoft.WindowsAppRuntime.Bootstrap.dll' -d "$WORK"
mkdir -p "$WORK/pkg"
cp "$ROOT/dist/launcher-poc.exe" "$WORK/runtimes/win-x64/native/Microsoft.WindowsAppRuntime.Bootstrap.dll" "$WORK/pkg/"
(cd "$WORK/pkg" && zip -q "$ROOT/dist/launcher-poc.zip" ./*)
unzip -l "$ROOT/dist/launcher-poc.zip"
