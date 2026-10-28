#!/usr/bin/env python3
"""从 assets/icon.png 重新生成各平台图标。

打包前必须跑的三件事：

1. 按平台标准重新排版：macOS / Windows 的图标内容只占画布的 80%
   （1024 画布上内容区 824），四周留透明边。AI 直接出的图是满幅的，
   放进 Dock 或桌面会比系统图标明显大一圈。
2. 颜色外扩（bleed）：把透明像素的 RGB 填成最近的不透明像素的颜色。
   空的透明像素如果留着白色或黑色，一旦遇到预乘 alpha、忽略 alpha、
   或者缩放插值，边角就会露出白边／黑边。外扩之后任何渲染路径都只会
   看到邻接的内容色，不会凭空多出一圈白。
3. ICO 小尺寸用 32bpp BMP 条目：Windows 与 Inno Setup 对 BMP 图标的
   支持最稳，256 才用 PNG。

用法：python3 scripts/make-icons.py  （需要 numpy + pillow，依赖 macOS 的 iconutil）
"""

import io
import os
import struct
import subprocess
import sys

import numpy as np
from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ASSETS = os.path.join(ROOT, "assets")
MASTER = os.path.join(ASSETS, "icon.png")

CANVAS = 1024
CONTENT = 824  # 平台标准：内容占画布 80.5%
PAD = (CANVAS - CONTENT) // 2

# 小尺寸用 BMP，256 用 PNG
BMP_SIZES = [16, 24, 32, 48, 64, 128]
PNG_SIZE = 256


def shift(arr, dy, dx):
    """平移指定步数，并把从画布另一侧卷绕过来的边清零。"""
    r = np.roll(arr, (dy, dx), axis=(0, 1))
    if dy > 0:
        r[:dy, ...] = 0
    elif dy < 0:
        r[dy:, ...] = 0
    if dx > 0:
        r[:, :dx, ...] = 0
    elif dx < 0:
        r[:, dx:, ...] = 0
    return r


def bleed(im):
    """把 alpha 为 0 的像素填上最近的不透明像素的颜色。

    步长倍增（1,2,4,...,512）能在十轮内铺满整幅图，比逐像素扩散快得多，
    结果是曼哈顿距离意义下的近似最近邻，边角够用。
    """
    arr = np.array(im)
    alpha = arr[..., 3]
    known = alpha > 0

    out = np.where(known[..., None], arr[..., :3], 0).astype(np.uint8)
    have = known.copy()

    height, width = arr.shape[:2]
    step = 1
    while step < max(height, width):
        for dy, dx in ((step, 0), (-step, 0), (0, step), (0, -step)):
            s_have = shift(have.view(np.int8), dy, dx).view(bool)
            if dy > 0:
                s_have[:dy, ...] = False
            elif dy < 0:
                s_have[dy:, ...] = False
            if dx > 0:
                s_have[:, :dx, ...] = False
            elif dx < 0:
                s_have[:, dx:, ...] = False

            take = (~have) & s_have
            if not take.any():
                continue
            out[take] = shift(out, dy, dx)[take]
            have |= take
        if have.all():
            break
        step *= 2

    arr[..., :3] = out
    return Image.fromarray(arr, "RGBA")


def recanvas(im):
    """把满幅内容缩到标准尺寸并居中，四周补透明。"""
    im = im.convert("RGBA")
    bbox = im.split()[-1].getbbox()
    if not bbox:
        sys.exit("母图整幅都是透明的，没法确定内容范围")
    cropped = im.crop(bbox)
    side = max(cropped.size)
    square = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    square.paste(
        cropped, ((side - cropped.width) // 2, (side - cropped.height) // 2)
    )
    scaled = square.resize((CONTENT, CONTENT), Image.LANCZOS)
    canvas = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    canvas.paste(scaled, (PAD, PAD), scaled)
    return canvas


def save_master(im):
    im.save(MASTER)
    for size in (128, 64):
        im.resize((size, size), Image.LANCZOS).save(
            os.path.join(ASSETS, f"icon-{size}.png")
        )


def make_icns(im):
    """macOS 的 .icns：先摆好 iconset 再交给 iconutil 打包。"""
    iconset = os.path.join(ASSETS, ".icon-work", "DeskSync.iconset")
    os.makedirs(iconset, exist_ok=True)
    for base in (16, 32, 128, 256, 512):
        im.resize((base, base), Image.LANCZOS).save(
            os.path.join(iconset, f"icon_{base}x{base}.png")
        )
        im.resize((base * 2, base * 2), Image.LANCZOS).save(
            os.path.join(iconset, f"icon_{base}x{base}@2x.png")
        )
    target = os.path.join(ASSETS, "DeskSync.icns")
    subprocess.run(
        ["iconutil", "-c", "icns", iconset, "-o", target], check=True
    )


def bmp_entry(im, size):
    """32bpp、自下而上的 DIB，外加全 0 的 AND 掩码（有 alpha 就不用它）。"""
    arr = np.array(im.resize((size, size), Image.LANCZOS))
    xor = b"".join(
        arr[y][:, [2, 1, 0, 3]].tobytes() for y in range(size - 1, -1, -1)
    )
    and_mask = bytes(((size + 31) // 32) * 4 * size)
    header = struct.pack(
        "<IiiHHIIiiII",
        40,          # biSize
        size,        # biWidth
        size * 2,    # biHeight：XOR 与 AND 两份叠在一起
        1,           # biPlanes
        32,          # biBitCount = 32bpp BGRA
        0,           # biCompression = BI_RGB
        len(xor) + len(and_mask),  # biSizeImage
        0, 0, 0, 0,
    )
    return header + xor + and_mask


def make_ico(im):
    """小尺寸用 BMP（兼容性最好），256 用 PNG。"""
    entries = [(s, bmp_entry(im, s)) for s in BMP_SIZES]
    buf = io.BytesIO()
    im.resize((PNG_SIZE, PNG_SIZE), Image.LANCZOS).save(buf, "PNG", optimize=True)
    entries.append((PNG_SIZE, buf.getvalue()))

    offset = 6 + 16 * len(entries)
    directory, blobs = b"", b""
    for size, blob in entries:
        directory += struct.pack(
            "<BBBBHHII",
            size if size < 256 else 0,
            size if size < 256 else 0,
            0, 0, 1, 32, len(blob), offset,
        )
        offset += len(blob)
        blobs += blob

    target = os.path.join(ASSETS, "DeskSync.ico")
    with open(target, "wb") as f:
        f.write(struct.pack("<HHH", 0, 1, len(entries)) + directory + blobs)


def describe(label, im):
    w = im.size[0]
    bbox = im.split()[-1].getbbox()
    span = (bbox[2] - bbox[0]) / w * 100 if bbox else 0
    corners = [im.getpixel(p) for p in ((0, 0), (w - 1, 0), (0, w - 1), (w - 1, w - 1))]
    print(f"{label}: 内容占比 {span:.0f}%, 四角 {corners[0]}")


def main():
    src = Image.open(MASTER).convert("RGBA")
    describe("处理前", src)

    master = bleed(recanvas(src))
    save_master(master)
    describe("处理后", master)

    make_icns(master)
    make_ico(master)
    subprocess.run(
        ["rm", "-rf", os.path.join(ASSETS, ".icon-work")], check=False
    )

    print("产物:")
    for name in ("DeskSync.icns", "DeskSync.ico", "icon-128.png", "icon-64.png"):
        path = os.path.join(ASSETS, name)
        print(f"  {name:<16} {os.path.getsize(path) / 1024:>7.0f} KB")


if __name__ == "__main__":
    main()
