# DeskSync

一个二进制文件，让局域网里的几台电脑共用同一套键盘鼠标，并同步剪贴板。

鼠标移到屏幕边缘再往外推，光标就出现在旁边那台电脑上，键盘跟着一起过去；沿原路推回来，控制回到本机。复制的文字在任意一台机器上都能直接粘贴。

不需要 KVM 切换器，不需要云端账号，不装驱动，不进内核。

## 特性

- **单文件**：一个可执行程序。纯命令行内核只有 1 MB 出头，带上图形界面也就几 MB。不依赖 Python、.NET、Java 或其他运行时。
- **跨平台**：Windows 10/11（x64）、macOS 13+（Apple Silicon / Intel）、Linux（X11，x64 / ARM64）。
- **有界面也有命令行**：菜单栏图标 + 设置窗口，改完配置当即生效；不想装界面时用 `desksync run` 一样能跑，两者可以随时切换。
- **mTLS 双向认证**：每台机器首次启动时自签一张 P-256 证书，连接双方互相验证证书指纹。没有 CA，也没有中间服务器。
- **局域网直连**：UDP 广播自动发现同一网段内的其他机器，也可以手动填 IP。数据不出局域网。
- **相对位移转发**：鼠标按移动量转发而不是绝对坐标，两台分辨率不同的机器也能对得上。
- **剪贴板同步**：任意一端复制，另一端立即可以粘贴（文本）。
- **自动收回**：被控端检测到本机真实键鼠活动，立刻把控制权拿回来。

## 下载

从 [Releases](https://github.com/KiwiTK/DeskSync/releases) 下载对应平台的安装包。

| 电脑 | 推荐 | 免安装 / 其他 |
| --- | --- | --- |
| macOS 13+（通用包，Apple Silicon 与 Intel 都能用） | `DeskSync-<版本>-macOS-universal.dmg`（打开后把 DeskSync 拖进 Applications） | `DeskSync-<版本>-macOS-universal.pkg`（安装器，装到 `/Applications`） |
| Windows 10/11 x64 | `DeskSync-<版本>-Windows-x64-Setup.exe`（安装器，可选开机自启、桌面快捷方式） | `DeskSync-<版本>-Windows-x64-portable.zip`（解压后双击 `desksync.exe`） |
| Linux x64 | `DeskSync-<版本>-Linux-x64.tar.gz` | 包内 `install.sh` 装好图标和启动器 |
| Linux ARM64 | `DeskSync-<版本>-Linux-arm64.tar.gz` | 同上 |

每个正式包都带 `.sha256`，Release 里还有汇总的 `SHA256SUMS.txt`。

Linux 上装桌面图标和启动器：

```bash
tar -xzf DeskSync-<版本>-Linux-x64.tar.gz
cd DeskSync-<版本>-Linux-x64
./install.sh                 # 装到 ~/.local，不需要 root
sudo ./install.sh --system   # 或装到 /usr/local
```

**macOS**：首次打开会被 Gatekeeper 拦（没有 Apple 公证）。到「系统设置 → 隐私与安全性」点「仍要打开」，或者先解除隔离属性：

```bash
xattr -dr com.apple.quarantine /Applications/DeskSync.app
```

**Linux**：除了 X11，还需要这几个运行时库（缺了会启动失败或没有托盘图标）：

```bash
sudo apt install libx11-6 libxtst6 libxdo3 libgtk-3-0 libayatana-appindicator3-1
# Fedora：sudo dnf install libX11 libXtst libxdo gtk3 libayatana-appindicator-gtk3
```

其中 `libxdo3` 负责注入键鼠事件，缺了会直接报错 `error while loading shared libraries: libxdo.so.3`。

Linux 上键鼠捕获依赖 X11（Xorg）会话；Wayland 会话下注入不可用，请登录时切换到 Xorg。

## 界面

安装完是一个普通桌面应用：**macOS 在屏幕顶部菜单栏、Windows 在任务栏右下角的托盘区、Linux 在状态栏常驻一个图标**。macOS 和 Linux 上不会有 Dock / 任务栏窗口，这是刻意的——共享工具该待在角落里，不占地方。

关掉设置窗口只是把它**藏起来**，进程还在菜单栏继续干活；要彻底退出，用托盘菜单里的「退出」（那会连后台服务一起停掉）。

点托盘图标里的「打开 DeskSync」弹出设置窗口，能看到：

- **当前状态**：后台服务是否运行、正在控制谁 / 被谁控制、已连接哪些邻居
- **暂停共享**：临时把共享停掉，本机输入不外发，也不接受接管
- **收回控制**：立刻把光标从对方机器上抢回来
- **本机设置**：名称、端口、是否自动信任首次连接的机器
- **屏幕布局**：每个邻居一行，选它在本机的哪个方向（左/右/上/下），可增删
- **证书指纹**：可一键复制，用来核对对端是不是你想连的那台

改完点保存，**方向、邻居这类改动当场生效，不用重启服务**；改名字或换端口需要重启（界面上有「停止服务 / 启动服务」按钮）。

界面和后台是两个独立进程，中间靠配置文件和状态文件通信。界面崩了或者干脆关掉，共享照样在跑。命令行 `desksync run` 可以完全脱离界面使用。

> 注：Linux 的桌面环境不支持状态栏图标时（部分精简 WM），托盘图标可能不显示，用 `desksync run` 直接跑即可。

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
  "port": 24987,
  "layout": {
    "left-pc": { "x": 0, "y": 0, "w": 1920, "h": 1080 },
    "right-pc": { "x": 1920, "y": 0, "w": 2560, "h": 1440 }
  },
  "peers": [{ "name": "right-pc", "addr": "192.168.1.20:24987" }],
  "auto_trust": true,
  "screen": null
}
```

编辑 `right-pc` 的配置：

```json
{
  "name": "right-pc",
  "port": 24987,
  "layout": {
    "right-pc": { "x": 0, "y": 0, "w": 2560, "h": 1440 },
    "left-pc": { "x": -1920, "y": 0, "w": 1920, "h": 1080 }
  },
  "peers": [{ "name": "left-pc", "addr": "192.168.1.10:24987" }],
  "auto_trust": true,
  "screen": null
}
```

`layout` 是虚拟画布上每台机器的摆放：本机放哪都行（推荐 `(0,0)`），邻居的矩形
和本机有公共边就能跨屏——**公共边的哪一段相接，光标就从那段过去**，所以
同一侧可以并排多台、可以错位摆放（比如侧屏只跟主屏的上半部分相接）。
尺寸写对方的真实分辨率；界面的「屏幕布局」画布里拖动即可，不用手写。
矩形尺寸与对方实际屏幕不符时，连接握手会自动校正。

同一网段且都开着广播发现时，`addr` 可以省略，会自动找到对方。

**第 3 步：各自启动**

```bash
./desksync run
```

看到 `已启动` 和证书指纹就说明在跑了。把鼠标推到右边界之外，光标出现在右机上。

## 命令

| 命令 | 作用 |
| --- | --- |
| `desksync`（不带参数） | 拉起后台服务并打开界面（就是双击 app 时做的事） |
| `desksync ui` | 只打开界面，后台服务没起的话会顺手拉起来 |
| `desksync init <名字>` | 生成配置文件和本机证书 |
| `desksync status` | 显示名字、端口、证书指纹、已信任的邻居 |
| `desksync run` | 只跑后台服务，不显示界面（服务器、SSH 环境用这个） |
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
| `port` | 监听端口，默认 24987（广播发现用 24986，即端口减一） |
| `layout` | 每台机器在虚拟画布上的矩形 `{x, y, w, h}`（含本机）。挨着谁就能跨到谁，公共边支持部分相接；旧版的 `edges` 会被自动转换成默认摆放 |
| `peers` | 邻居列表，`addr` 形如 `192.168.1.20:24987`；留空则依赖广播发现 |
| `trusted` | 已确认的邻居证书指纹（十六进制 SHA-256），首次连接后自动写入 |
| `auto_trust` | 首次连接自动信任。家用局域网建议 `true`；想严格一点设 `false`，连接后手动把指纹写进 `trusted` |
| `screen` | 手动指定虚拟桌面尺寸 `[宽, 高]`。多显示器探测不准时用它覆盖 |

## 权限

键鼠捕获需要操作系统授权，程序自己弹不出这个框，得手动加：

- **macOS**：系统设置 → 隐私与安全性 → 辅助功能（以及输入监控），把 `desksync` 加进去并打开开关。首次运行没授权的话，日志会提示 `输入捕获失败`。
- **Windows**：不需要额外授权，但以管理员权限运行时，无法控制以更高完整性级别运行的窗口（UAC 弹窗等）。普通用户权限即可正常使用。
- **Linux**：需要 X11 会话（Wayland 会话下键鼠注入不可用，请切换到 Xorg 登录）。

## 排障

程序出问题时先看日志，界面进程和后台服务都写同一个文件：

| 系统 | 日志位置 |
| --- | --- |
| Windows | `%LOCALAPPDATA%\DeskSync\desksync.log` |
| macOS | `~/Library/Application Support/DeskSync/desksync.log` |
| Linux | `~/.config/desksync/desksync.log` |

- **双击完全没反应、也不出界面**：先确认这个程序到底带不带界面。PowerShell 里跑：

  ```powershell
  .\desksync.exe status
  ```

  输出里必须有 `界面支持: 是`。如果是「否」，说明这个 exe 编译时就没带界面特性（双击只会静默跑后台），请重新下载 v0.2.4 及以上版本的安装包。

- 界面点不开、一闪而过：看日志最后的 `PANIC` 或 `启动失败`。Windows 上启动失败还会弹一个错误框，把里面的文字发出来即可。
- **Windows 图标显示不对（白角、旧图）**：多半是系统的图标缓存，换版本后不会自动刷新。管理员 PowerShell 里跑：

  ```powershell
  ie4uinit.exe -show
  taskkill /f /im explorer.exe; start explorer.exe
  ```

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
cargo build --release --features ui
```

产物在 `target/release/desksync`。带上界面体积会涨到几 MB；如果只要纯命令行内核，去掉 `--features ui`，体积回到 1 MB 出头——这是 `feature` 刻意分开的原因。

release 配置开了 `opt-level="z"`、fat LTO、单编译单元和符号剥离，这是体积能做到这么小的主要原因。

Linux 构建需要这些系统库：`libx11-dev`、`libxtst-dev`、`libxdo-dev`（键鼠注入）、`libgtk-3-dev`、`libayatana-appindicator3-dev`、`libxcb-*-dev`、`libxkbcommon-dev`（界面用）。macOS 可以直接 `./scripts/package-macos.sh` 打出 `.app` / `.dmg` / `.pkg`。

## 当前限制

0.1 是可用核心，以下还没做：

- 界面刚起步：够用，但还比较朴素，后续会按使用手感迭代
- 只同步剪贴板文本，不传图片，也不传文件
- 邻居之间不支持链式跳转（A → B → C 需要 A、C 也直接相连）
- 没有开机自启动（各平台用自己的方式加：launchd / 任务计划 / systemd）
- 没有商业代码签名和 Apple 公证，首次运行需要手动放行

## License

MIT
