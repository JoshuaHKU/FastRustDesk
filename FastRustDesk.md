# FastRustDesk — 构建与部署

改动的技术说明、完整的构建步骤、以及部署时要注意的地方。
项目介绍与功能列表见 [README.md](README.md)。

上游基线：`rustdesk/rustdesk` `11190fa`（v1.4.9）。

---

## 一、几处改动的技术说明

### 0. 三个刻意保持原样的标识

- **bundle identifier 仍是 `com.carriez.rustdesk`。** macOS 的 TCC 授权绑定在
  bundle id + 代码签名的 designated requirement 上。改了它，已授予的屏幕录制、
  辅助功能、麦克风权限全部作废，必须人工到「系统设置」里重新逐项勾选
  ——而远程的机器根本点不到那个授权对话框。
- **cargo crate 名仍是 `rustdesk`。** `librustdesk.dll` / `liblibrustdesk.dylib`
  和 flutter_rust_bridge 的生成文件都是由它产出并按它命名的。
- **配置目录随 `APP_NAME` 走。** 它不只是个标签 —— 还构成 IPC socket 路径和
  配置文件名（`FastRustDesk.toml` / `FastRustDesk2.toml`）。首次启动时若发现
  自己的配置目录不存在，会把上游 `RustDesk` 那份**复制**过来并改名
  （`Config::migrate_legacy_config_dir()`）；复制而非移动，旧目录留着不动。

### 1. ScreenCaptureKit 采集

Apple 在 macOS 14 弃用了 `CGDisplayStream`。新后端在 `libs/scrap/src/sck/`。

**只替换了「流」，没有动 `Display`。** `quartz::Display` 建立在
`CGDisplayPixelsWide` / `CGGetOnlineDisplayList` / `CGDisplayBounds` / `CGDisplayIsMain`
之上，这些都是**当前 API**，而且提供 SCK 拿不到的几何信息。更关键的是，显示器标识仍然是
`CGDirectDisplayID` —— 树里别处依赖这一点（`platform::macos::change_resolution_directly`
会把 `Display::name()` 解析回 display id）。因此 `common/quartz.rs` 只改了 4 行。

`Config` 的字段一一映射到 `SCStreamConfiguration`：

| Config | SCStreamConfiguration |
|---|---|
| `cursor` | `shows_cursor` |
| `letterbox` | `scales_to_fit`（保持宽高比，四周留黑边） |
| `throttle` | `minimum_frame_interval` |
| `queue_length` | `queue_depth` |

**两个行为差异**（都在 `sck/mod.rs` 的注释里写明了原因）：

- 帧在回调里就复制成 BGRA，而不是把加锁的 IOSurface 交出去。老后端在
  `surface_to_bgra` 里反正也要复制一次，所以不亏，而且避免了跨线程持有
  `CVPixelBuffer` 锁。
- `SCFrameStatus` 的 Idle / Blank / Suspended 在回调里直接丢弃。老后端靠整帧
  memcmp 达到同样效果，只是更晚更贵。

`find_display()` **坚持要 CG 主显示器那个确切 id**，不将就别的。显示器重配置期间
SCK 曾报出一个过期 id，随后又拒绝采集它（`No capture source provided`），表现为**永久黑屏**。
现在重试 5 次 × 400ms，仍拿不到就失败，让上层去处理。

### 2. 为什么必须改部署目标

`build.py` 原本硬编 `MACOSX_DEPLOYMENT_TARGET=10.14`，而 SCK 要 12.3+。
Xcode 工程里 6 处配置也一并改了（`Info.plist` 的 `LSMinimumSystemVersion` 由它派生）。

**这个改动上游不可能接受** —— 要上游就得让 SCK 和 CGDisplayStream 两个后端并存 +
运行时判断可用性。本分支只跑在一台 macOS 26.5 的机器上，所以直接替换。

### 3. H265

**官方 1.4.9 本来就会用 `hevc_videotoolbox`**，前提是编译时带 `--hwcodec`。
本分支在这一项上**与官方无差异**，只是确保用对了 feature。

⚠️ 这台机器上**没有 `h264_videotoolbox`**。H265 协商失败会掉回 **VP9 软编**，
而不是 H264 硬编。`examples/pipeline_probe.rs` 里有断言专门盯这一点。

---

## 二、构建

### macOS

```bash
export VCPKG_ROOT=~/vcpkg
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
export PATH="$HOME/flutter/bin:$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8
python3 build.py --flutter --hwcodec
```

产物：`flutter/build/macos/Build/Products/Release/FastRustDesk.app`

**前置条件**（都踩过坑，按顺序）：

| 依赖 | 说明 |
|---|---|
| Xcode（完整版） | 只有 Command Line Tools 不行。首次要接受许可：`sudo xcodebuild -license accept`，或打开一次 Xcode.app |
| Rosetta 2 | `sudo softwareupdate --install-rosetta --agree-to-license`。Flutter 3.24.5 的 `gen_snapshot_arm64` **自身是 x86_64 程序** |
| CocoaPods | `brew install cocoapods`（避开 `sudo gem install`） |
| Flutter **3.24.5** | 对齐 CI 的 pin。装在 `~/flutter` |
| vcpkg | 见下 |

**`.cargo/config.toml` 里那条 rpath 不能删**：

```toml
"-C", "link-arg=-Wl,-rpath,/usr/lib/swift",
```

`screencapturekit` crate 会构建 Swift 桥接，链接 Swift 并发运行时。没有这条
rpath，**能编译但一启动就崩**（`@rpath/libswift_Concurrency.dylib` 找不到）。
`/usr/lib/swift` 在磁盘上不存在（`ls` 看不到），它在 dyld 共享缓存里，macOS 12+ 都有。

### Windows 客户端

在一台 Windows 机器上编译（Flutter 桌面端**无法从 macOS 交叉编译**）：

```powershell
$env:VCPKG_ROOT = "D:\Apply\vcpkg"
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
$env:VCPKG_DEFAULT_HOST_TRIPLET = "x64-windows-static"
$env:USE_AOM_391 = "1"
$env:PATH = "D:\Apply\flutter\bin;$env:USERPROFILE\.cargo\bin;C:\Program Files\LLVM\bin;$env:PATH"
cd D:\Apply\RustDesk
python build.py --portable --flutter --skip-portable-pack --hwcodec
```

产物：`flutter\build\windows\x64\runner\Release`，整理到 `D:\Apply\RustDesk\rustdesk\`

⚠️ **`VCPKG_DEFAULT_HOST_TRIPLET` 不能漏**。`vcpkg.json` 把 ffmpeg 声明成
`"host": true`，宿主 triplet 默认是 `x64-windows`（动态），而 `hwcodec` 的
`build.rs` **硬编码**去 `x64-windows-static` 找 `avcodec/avformat/avutil/libmfx`。
两边对不上就是 `cl.exe exit code 2`。

### vcpkg

**必须把 vcpkg 检出到 CI 钉的那个 commit**，不只是 manifest 里的 baseline：

```bash
cd $VCPKG_ROOT
git checkout 120deac3062162151622ca4860575a33844ba10b
./bootstrap-vcpkg.sh -disableMetrics
cd <repo> && $VCPKG_ROOT/vcpkg install --triplet <triplet> --host-triplet <triplet> \
    --x-install-root="$VCPKG_ROOT/installed"
```

工具版本由**检出的 commit** 决定：钉住后它要 PowerShell 7.2.24 / 7zip 24.09 /
ninja 1.12.1；用 master 则要 nasm 3.01，而 **aom 会拒绝 nasm 3.01**
（`Unsupported nasm: multipass optimization not supported`）。

---

## 三、离线构建（构建机连不上 github 时）

若构建机**无法访问 github.com**，而 vcpkg 的下载器**没有超时** ——
被墙的下载表现为**进程挂死**：vcpkg.exe 常驻内存约 17 MB、无 cl/ninja 子进程、
buildtrees 数分钟无文件变动。

做法：在 Mac 上下好，`scp` 过去，重跑（vcpkg 会跳过已完成的包）。

**从日志里读确切文件名** —— vcpkg 对部分文件加哈希前缀：

```
Downloading https://github.com/ip7z/7zip/.../7zr.exe -> 44d8504a-7zr.exe
```

按常规命名放进去它不认。

需要预置的三类：

| 类别 | 内容 |
|---|---|
| vcpkg 工具与源码 | PowerShell 7.2.24、7zip、7zr、ninja、vswhere、strawberry-perl 5.42.0.1、AMF headers、nv-codec-headers、meson，以及 6 个源码 tarball |
| cargo git 依赖（**41 个**） | 打包 `~/.cargo/git/db` 推过去解开。cargo 在**锁定的 rev 已存在于本地 db 时会跳过 fetch** |
| pub git 依赖（**8 个**） | 打包 `~/.pub-cache/git`，解到 **`%LOCALAPPDATA%\Pub\Cache`**（Windows 不用 `~/.pub-cache`） |

源码传输用 `git bundle`（含子模块 `hbb_common`），初始化子模块时要放行 file 传输：

```
git -c protocol.file.allow=always submodule update --init --recursive
```

否则被 git 的 CVE-2022-39253 加固挡住（`transport 'file' not allowed`）。

**通过 SSH 在 Windows 上执行 PowerShell，一律先 `scp` 脚本文件再运行**：

```bash
scp x.ps1 user@host:C:/Users/user/x.ps1
ssh user@host 'powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\user\x.ps1'
```

内联传会被 cmd 吃掉 `|`、引号和正则，报出 `'win32' 不是内部或外部命令` 这类莫名其妙的错。

---

## 四、部署（macOS 被控端）

```bash
cp -R FastRustDesk.app /Applications/          # 或 ditto，保留签名元数据
codesign --force --deep --sign macrdp-dev --identifier com.carriez.rustdesk /Applications/FastRustDesk.app
open -a FastRustDesk
```

配置密码和直连（**不需要 root，也不需要装服务**）：

```bash
cargo run --release --features hwcodec,flutter --example setup_host -- '<密码>'
```

密码是**加盐哈希**存储的，不能手写进 `FastRustDesk.toml`；这个 helper 走的是 RustDesk
自己的 `Config::set_permanent_password`，同时把 `direct-server` 设成 `Y`。

`rustdesk --password` 这条 CLI 要求 `is_installed() && is_root()`，所以用不上。

### 签名为什么必须固定

**TCC 把权限绑定在应用的 designated requirement 上。** ad-hoc 签名没有稳定身份，
**每次重编译都会掉权限**，表现为：连上了、有画面（屏幕录制碰巧还在）、但**键鼠完全不起作用**
（`AXIsProcessTrustedWithOptions` 和 `InputMonitoringAuthStatus` 双双失败）。

用本机的 `macrdp-dev` 自签证书签名后：

```
designated => identifier "com.carriez.rustdesk" and certificate root = H"fee2a49b..."
```

跨重编译保持不变，**只需授权一次**。`build.py` 里已自动使用该证书，找不到时回退 ad-hoc。

### 需要的三项系统权限

系统设置 → 隐私与安全性：

| 权限 | 作用 | 缺了会怎样 |
|---|---|---|
| 屏幕录制 | 采集 | 黑屏 |
| **辅助功能** | 键鼠注入 | **有画面但控制不了** |
| 输入监控 | 键盘事件 | 键盘异常 |

### 客户端连接

输入框里填 **IP**（如 `192.168.1.10`），不是 ID。

填 ID 会走 RustDesk **公共服务器**，而它现在因滥用问题**要求登录**：

> The connection is not allowed. Due to ongoing scamming and botnet abuse,
> this public server currently requires users to log in before connecting.

填 IP 是**直连**，完全不碰他们的服务器。

---

## 五、验证工具

| 工具 | 用途 |
|---|---|
| `cargo run --example sck_probe` | 枚举显示器，验证 SCK 出帧、几何正确（`len == stride*height`） |
| `cargo run --release --features hwcodec --example pipeline_probe` | 端到端：SCK 采集 → H265 硬编，报 fps / 码率 / 编码耗时 |
| `cargo run -p scrap --features hwcodec --example hw_probe` | 列出本机可用的硬件编解码器 |
| `cargo run --features hwcodec,flutter --example setup_host -- <pw>` | 设密码 + 开直连 |

实测基线（M4 / macOS 26.5，4096×2304，10 秒）：

```
H265 encoder    : hevc_videotoolbox
captured frames : 300  (30.0/s)
encoded units   : 299 (1 keyframes)
bitstream       : 7.65 Mbps
worst encode    : 27.7 ms
```

⚠️ 这些探针需要**运行它们的进程**（终端）拥有屏幕录制权限，否则报
`用户拒绝了应用程序、窗口、显示器捕捉的TCC`。这与 FastRustDesk.app 自身的权限是两回事。

---

## 六、已知问题

- **显示器睡眠/唤醒期间采集会失败。** `CGMainDisplayID` 在那段时间可能变成 **1**，
  而 SCK 的可共享列表里没有这个 id，采集会一直失败到显示器醒来。分辨率也可能
  在几个值之间跳变，代码里对此做了重试而不是把首次答复当真。
- **无 `h264_videotoolbox`**：H265 失败会掉到 VP9 软编，不是 H264 硬编。
- **上游无法合并**：部署目标提到 12.3、直接删掉 CGDisplayStream 后端、提交了生成文件。
- **许可证 AGPL-3.0**：自己改自己用完全没有义务；一旦分发（哪怕只是提供网络服务）就有开源义务。
