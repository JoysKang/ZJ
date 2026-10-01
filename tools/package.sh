#!/bin/sh
set -eu
project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$project_root"
export CARGO_HOME="${CARGO_HOME:-$project_root/target/cargo-home}"
cargo build --profile dist --locked
bundle="$project_root/target/ZJ.app"
mkdir -p "$bundle/Contents/MacOS"
cp target/dist/workspace-editor "$bundle/Contents/MacOS/ZJ.new"
mv -f "$bundle/Contents/MacOS/ZJ.new" "$bundle/Contents/MacOS/ZJ"
mkdir -p "$bundle/Contents/Resources"
cp crates/app/assets/app-icon/bamboo.icns "$bundle/Contents/Resources/bamboo.icns"
cat > "$bundle/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>ZJ</string>
<key>CFBundleIdentifier</key><string>local.zj.editor</string>
<key>CFBundleName</key><string>ZJ</string>
<key>CFBundleDisplayName</key><string>ZJ</string>
<key>CFBundleIconFile</key><string>bamboo.icns</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>LSMinimumSystemVersion</key><string>14.0</string>
</dict></plist>
PLIST
printf '%s\n' "$bundle"
