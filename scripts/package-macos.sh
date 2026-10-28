#!/usr/bin/env bash
# 打包 macOS：DeskSync.app（通用二进制）→ dmg（拖拽安装）+ pkg（安装器）
#
# 用法：scripts/package-macos.sh [版本]
# 产物写在 dist/ 下。需要先用 cargo build --release --features ui 编出两个架构的二进制。

set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="${1:-$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')}"
APPLE_DIR="target/universal"
APP="DeskSync.app"
STAGE="dist"

if [ ! -f "target/aarch64-apple-darwin/release/desksync" ] || [ ! -f "target/x86_64-apple-darwin/release/desksync" ]; then
  echo "缺少架构二进制，请先运行 cargo build --release --features ui --target <aarch64|x86_64>-apple-darwin" >&2
  exit 1
fi

echo "==> 合成通用二进制"
mkdir -p "$APPLE_DIR"
lipo -create -output "$APPLE_DIR/desksync" \
  target/aarch64-apple-darwin/release/desksync \
  target/x86_64-apple-darwin/release/desksync
lipo -info "$APPLE_DIR/desksync"

echo "==> 组装 $APP"
rm -rf "$APP" "$STAGE"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$STAGE"
cp "$APPLE_DIR/desksync" "$APP/Contents/MacOS/DeskSync"
cp assets/DeskSync.icns "$APP/Contents/Resources/AppIcon.icns"
cp README.md LICENSE "$APP/Contents/Resources/"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>DeskSync</string>
    <key>CFBundleDisplayName</key>
    <string>DeskSync</string>
    <key>CFBundleIdentifier</key>
    <string>com.kiwitk.desksync</string>
    <key>CFBundleExecutable</key>
    <string>DeskSync</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

echo "==> 本地 ad-hoc 签名（没有商业证书时的标准做法）"
codesign --force --deep -s - "$APP"
codesign --verify --verbose=2 "$APP" 2>&1 | head -3

DMG_NAME="DeskSync-${VERSION}-macOS-universal.dmg"
PKG_NAME="DeskSync-${VERSION}-macOS-universal.pkg"

echo "==> 生成 $DMG_NAME"
DMG_ROOT=".dmgroot"
rm -rf "$DMG_ROOT"
mkdir -p "$DMG_ROOT"
cp -R "$APP" "$DMG_ROOT/"
ln -sf /Applications "$DMG_ROOT/Applications"
hdiutil create -volname "DeskSync ${VERSION}" -srcfolder "$DMG_ROOT" -ov -format UDZO "$STAGE/$DMG_NAME" > /dev/null
rm -rf "$DMG_ROOT"

echo "==> 生成 $PKG_NAME"
PKG_ROOT=".pkgroot"
rm -rf "$PKG_ROOT"
mkdir -p "$PKG_ROOT"
cp -R "$APP" "$PKG_ROOT/"
pkgbuild --identifier com.kiwitk.desksync \
  --version "$VERSION" \
  --install-location /Applications \
  --root "$PKG_ROOT" \
  "$STAGE/$PKG_NAME" > /dev/null
rm -rf "$PKG_ROOT"

cd "$STAGE"
shasum -a 256 "$DMG_NAME" | awk '{print $1}' > "$DMG_NAME.sha256"
shasum -a 256 "$PKG_NAME" | awk '{print $1}' > "$PKG_NAME.sha256"
ls -lh
echo "完成：dist/$DMG_NAME dist/$PKG_NAME"
