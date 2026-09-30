#!/bin/sh
set -eu
project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$project_root"
export CARGO_HOME="${CARGO_HOME:-$project_root/target/cargo-home}"
cargo build --release --locked
bundle="$project_root/target/workspace-editor.app"
mkdir -p "$bundle/Contents/MacOS"
cp target/release/workspace-editor "$bundle/Contents/MacOS/workspace-editor"
cat > "$bundle/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>workspace-editor</string>
<key>CFBundleIdentifier</key><string>local.workspace-editor.prototype</string>
<key>CFBundleName</key><string>workspace-editor</string>
<key>CFBundleDisplayName</key><string>workspace-editor P1</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>LSMinimumSystemVersion</key><string>14.0</string>
</dict></plist>
PLIST
printf '%s\n' "$bundle"
