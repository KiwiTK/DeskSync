# DeskSync

一个二进制文件，让局域网里的几台电脑共用同一套键盘鼠标，并同步剪贴板。

鼠标移到屏幕边缘再往外推，光标就出现在旁边那台电脑上，键盘跟着一起过去；沿原路推回来，控制回到本机。复制的文字在任意一台机器上都能直接粘贴。

不需要 KVM 切换器，不需要云端账号，不装驱动，不进内核。

## 特性

- **单文件**：一个可执行程序，1 MB 出头，解压即用。不依赖 Python、.NET、Java 或其他运行时。
- **跨平台**：Windows 10/11（x64）、macOS 13+（Apple Silicon / Intel）、Linux（X11，x64 / ARM64）。
- **mTLS 双向认证**：每台机器首次启动时自签一张 P-256 证书，连接双方互相验证证书指纹。没有 CA，也没有中间服务器。
- **局域网直连**：UDP 广播自动发现同一网段内的其他机器，也可以手动填 IP。数据不出局域网。
- **相对位移转发**：鼠标按移动量转发而不是绝对坐标，两台分辨率不同的机器也能对得上。
- **剪贴板同步**：任意一端复制，另一端立即可以粘贴（文本）。
- **自动收回**：被控端检测到本机真实键鼠活动，立刻把控制权拿回来。

## 下载

从 [Releases](https://github.com/KiwiTK/DeskSync/releases) 下载对应平台的压缩包，解压得到 `desksync`（Windows 为 `desksync.exe`）。

| 电脑 | 文件 |
| --- | --- |
| Windows 10/11 x64 | `DeskSync-<版本>-Windows-x64.zip` |
| Apple Silicon Mac | `DeskSync-<版本>-macOS-arm64.tar.gz` |
| Intel Mac | `DeskSync-<版本>-macOS-x86_64.tar.gz` |
| Linux x64 | `DeskSync-<版本>-Linux-x64.tar.gz` |
| Linux ARM64 | `DeskSync-<版本>-Linux-arm64.tar.gz` |

下载后可核对 `SHA256SUMS.txt`。

Linux 上键鼠捕获依赖 X11（Xorg）会话：

```bash
sudo apt install libx11-6 libxtst6      # Debian / Ubuntu
sudo dnf install libX11 libXtst         # Fedora
```

macOS 首次运行如果提示无法打开，是 Gatekeeper 拦的（没有商业签名）。解除隔离属性即可：

```bash
xattr -dr com.apple.quarantine desksync
```

## 快速开始

两台机器，左边一台叫 `left-pc`，右边一台叫 `right-pc`。

**第 1 步：各自生成配置**

```bash
./desksync init left-pc      # 在左边那台
./desksync init right-pc     # 在右边那台
```

会打印配置文件路径，同时生成证书。

**第 2 步：互相登记**

编辑 `left-pc` 的配置：

```json
{
  "name": "left-pc",
  "port": 24872,
  "edges": { "right-pc": "right" },
  "peers": [{ "name": "right-pc", "addr": "192.168.1.20:24872" }],
  "auto_trust": true,
  "screen": null
}
```

编辑 `right-pc` 的配置：

```json
{
  "name": "right-pc",
  "port": 24872,
  "edges": { "left-pc": "left" },
  "peers": [{ "name": "left-pc", "addr": "192.168.1.10:24872" }],
  "auto_trust": true,
  "screen": null
}
```

`edges` 说的是「从我这边出发，往哪个方向走能到它」——左机的右边通向右机，右机的左边通向左机，两边必须互为反向。`addr` 里的端口写对方的 `port`。

同一网段且都开着广播发现时，`addr` 可以省略，会自动找到对方。

**第 3 步：各自启动**

```bash
./desksync run
```

看到 `已启动` 和证书指纹就说明在跑了。把鼠标推到右边界之外，光标出现在右机上。

## 命令

| 命令 | 作用 |
| --- | --- |
| `desksync init <名字>` | 生成配置文件和本机证书 |
| `desksync status` | 显示名字、端口、证书指纹、已信任的邻居 |
| `desksync run` | 常驻运行 |
| `desksync run --no-input` | 只跑网络，不捕获也不注入键鼠（本地联调用） |

## 配置项

配置文件路径：

| 系统 | 位置 |
| --- | --- |
| macOS | `~/Library/Application Support/DeskSync/config.json` |
| Windows | `%LOCALAPPDATA%\DeskSync\config.json` |
| Linux | `${XDG_CONFIG_HOME:-~/.config}/desksync/config.json` |

| 字段 | 说明 |
| --- | --- |
| `name` | 本机名称，广播和配对时显示 |
| `port` | 监听端口，默认 24872（广播发现用 24871，即端口减一） |
| `edges` | 邻居名 → 通向它的那条边：`left` / `right` / `top` / `bottom` |
| `peers` | 邻居列表，`addr` 形如 `192.168.1.20:24872`；留空则依赖广播发现 |
| `trusted` | 已确认的邻居证书指纹（十六进制 SHA-256），首次连接后自动写入 |
| `auto_trust` | 首次连接自动信任。家用局域网建议 `true`；想严格一点设 `false`，连接后手动把指纹写进 `trusted` |
| `screen` | 手动指定虚拟桌面尺寸 `[宽, 高]`。多显示器探测不准时用它覆盖 |

## 权限

键鼠捕获需要操作系统授权，程序自己弹不出这个框，得手动加：

- **macOS**：系统设置 → 隐私与安全性 → 辅助功能（以及输入监控），把 `desksync` 加进去并打开开关。首次运行没授权的话，日志会提示 `输入捕获失败`。
- **Windows**：不需要额外授权，但以管理员权限运行时，无法控制以更高完整性级别运行的窗口（UAC 弹窗等）。普通用户权限即可正常使用。
- **Linux**：需要 X11 会话（Wayland 会话下键鼠注入不可用，请切换到 Xorg 登录）。

## 快捷键

| 操作 | 快捷键 |
| --- | --- |
| 强制收回控制 | `Ctrl + Alt + Esc` |

## 工作原理

1. 每台机器启动一对 TCP 连接，双方用自签证书做 mTLS 握手，按证书 SHA-256 指纹认人。
2. 本机捕获到的键鼠事件序列化后发给当前被控端；鼠标事件转成相对位移发送，避免分辨率差异。
3. 光标顶到配置好的那条边并继续外推时，交出控制：把光标位置归一化传给对端，对端把光标放到对应边的相同相对位置。
4. 交出控制后，本机光标被拉回距边缘 64 px 处——操作系统在光标贴边后不再上报位移，拉回来才能继续读到移动量，从而判断「往回推」。
5. 被控端收到对端的真实输入事件时立即收回控制，两边不会同时操作。

## 从源码构建

```bash
cargo build --release
```

产物在 `target/release/desksync`。release 配置开了 `opt-level="z"`、fat LTO、单编译单元和符号剥离，这是体积能做到 1 MB 出头的主要原因。

Linux 构建需要 `libx11-dev` 和 `libxtst-dev`。

## 当前限制

0.1 是可用核心，以下还没做：

- 只有命令行，没有图形界面和托盘图标，配置靠手改 JSON
- 只同步剪贴板文本，不传图片，也不传文件
- 邻居之间不支持链式跳转（A → B → C 需要 A、C 也直接相连）
- 没有开机自启动（各平台用自己的方式加：launchd / 任务计划 / systemd）
- 没有商业代码签名和 Apple 公证，首次运行需要手动放行

## License

MIT
