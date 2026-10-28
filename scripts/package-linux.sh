#!/usr/bin/env bash
# 打包 Linux：tar.gz（含二进制、图标、desktop 文件、安装脚本）
#
# 用法：scripts/package-linux.sh <target-triple> <版本>
# 例：  scripts/package-linux.sh x86_64-unknown-linux-gnu 0.2.0

set -euo pipefail
cd "$(dirname "$0")/.."

TARGET="${1:?需要目标三元组}"
VERSION="${2:-$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')}"

case "$TARGET" in
  x86_64-*)   ARCH="x64" ;;
  aarch64-*)  ARCH="arm64" ;;
  *)          ARCH="$TARGET" ;;
esac

BIN="target/$TARGET/release/desksync"
if [ ! -f "$BIN" ]; then
  echo "找不到 $BIN，请先 cargo build --release --features ui --target $TARGET" >&2
  exit 1
fi

NAME="DeskSync-${VERSION}-Linux-${ARCH}"
STAGE="dist"
rm -rf "$STAGE/$NAME"
mkdir -p "$STAGE/$NAME"

cp "$BIN" "$STAGE/$NAME/desksync"
cp assets/icon.png "$STAGE/$NAME/desksync.png"
cp packaging/desksync.desktop "$STAGE/$NAME/desksync.desktop"
cp README.md LICENSE "$STAGE/$NAME/"

cat > "$STAGE/$NAME/install.sh" <<'EOS'
#!/usr/bin/env bash
# 安装 DeskSync 到当前用户（不需要 root 也能跑，给 --system 则装到 /usr/local）
set -euo pipefail
cd "$(dirname "$0")"
PREFIX="$HOME/.local"
BIN_DIR="$PREFIX/bin"
if [ "${1:-}" = "--system" ]; then
  PREFIX="/usr/local"
  BIN_DIR="$PREFIX/bin"
fi

install -Dm755 desksync "$BIN_DIR/desksync"
install -Dm644 desksync.png "$PREFIX/share/icons/hicolor/512x512/apps/desksync.png"
install -Dm644 desksync.desktop "$PREFIX/share/applications/desksync.desktop"
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
fi
echo "已安装到 $BIN_DIR/desksync"
echo "命令行直接敲 desksync 即可启动（菜单栏会出现图标）。"
echo "运行依赖：libx11 libxtst libxdo3 gtk3 libayatana-appindicator3（缺 libxdo 会直接起不来）。"
EOS
chmod +x "$STAGE/$NAME/install.sh"

tar -czf "$STAGE/$NAME.tar.gz" -C "$STAGE" "$NAME"
rm -rf "$STAGE/$NAME"
(cd "$STAGE" && shasum -a 256 "$NAME.tar.gz" | awk '{print $1}' > "$NAME.tar.gz.sha256")

echo "完成：dist/$NAME.tar.gz"
ls -lh "$STAGE/$NAME.tar.gz"
