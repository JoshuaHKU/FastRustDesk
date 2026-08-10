# FastRustDesk macOS 客户端（改造版）— 安装配置说明

`FastRustDesk-macos-arm64.zip` — Apple Silicon，**macOS 12.3 及以上**。

## 与官方版的区别

**唯一真正的行为差异是屏幕采集层**：
用 **ScreenCaptureKit** 替换了官方的 CGDisplayStream（Apple 已在 macOS 14 弃用后者）。
因此最低系统要求从 10.14 提到 12.3。

H265 硬编（VideoToolbox）**官方版本来就有**，用 `--hwcodec` 编译即启用，不是本版差异。
其余改动是打包与签名修正，详见 [FastRustDesk.md](FastRustDesk.md)。

本机实测硬件编码器只有 `hevc_videotoolbox`，**没有 `h264_videotoolbox`** ——
H265 协商失败会退到 VP9 软编，不是 H264 硬编。

## 一、安装

> ⚠️ **从旧的 RustDesk 升级，必须先把旧的守护进程停掉。**
>
> 自启项的标签由 `<ORG>.<APP_NAME>` 派生（`src/platform/macos.rs` 的
> `get_full_name()`），改名后新版注册的是 `com.carriez.FastRustDesk_service`，
> 与旧的 `com.carriez.RustDesk_service` **并存而不是替换**。两个都是 KeepAlive，
> 会抢同一个 ID 向服务器注册，表现为连上去的是随机一个。
>
> ```bash
> sudo launchctl bootout system/com.carriez.RustDesk_service 2>/dev/null
> launchctl bootout gui/$(id -u)/com.carriez.RustDesk_server 2>/dev/null
> sudo rm -f /Library/LaunchDaemons/com.carriez.RustDesk_service.plist \
>            /Library/LaunchAgents/com.carriez.RustDesk_server.plist
> pkill -x RustDesk; sudo pkill -f "MacOS/service"
> sudo rm -rf /Applications/RustDesk.app
> ```
>
> **配置和固定密码会自动继承**：首次启动时 FastRustDesk 发现自己的配置目录不
> 存在，会把 `~/Library/Preferences/com.carriez.RustDesk/` 整个复制过来并把文件名
> 里的 `RustDesk` 前缀换成 `FastRustDesk`（`Config::migrate_legacy_config_dir()`）。
> **复制不是移动**，旧目录留着不动。
>
> 密码之所以能跟着走，是因为它用**本机 UUID** 加密，UUID 没变，同一台机器解得开
> ——但也正因如此，**换台机器拷配置文件是无效的**（见第四节）。
>
> 三项系统权限也不用重授：TCC 绑定的是 bundle id + 签名，两者都没变（见第二节）。

```bash
unzip FastRustDesk-macos-arm64.zip
xattr -dr com.apple.quarantine FastRustDesk.app
sudo ditto FastRustDesk.app /Applications/FastRustDesk.app
open -a /Applications/FastRustDesk.app
```

安装后系统会多出两个自启项，都是 KeepAlive 的：

- `/Library/LaunchDaemons/com.carriez.FastRustDesk_service.plist` —— **root** 身份
- `/Library/LaunchAgents/com.carriez.FastRustDesk_server.plist` —— 登录用户身份

## 二、三项系统权限，缺一不可

系统设置 → 隐私与安全性，逐项把 FastRustDesk 加进去：

| 权限 | 缺了的症状 |
|---|---|
| 屏幕录制 | 黑屏 |
| **辅助功能** | **有画面，但键盘鼠标控制不了对方** |
| 输入监控 | 键盘行为异常 |

**这三项只能在图形界面点，SSH 无法代劳。**

⚠️ **权限绑定在代码签名的 designated requirement 上**：

```
identifier "com.carriez.rustdesk" and certificate root = H"fee2a49b..."
```

本包用自签证书 `macrdp-dev` 签名（证书嵌在签名里，目标机钥匙串里没有也能验证通过）。
**用别的身份重新签名等于换了个应用，三项权限都要重新授权。**
私钥在源机 `~/.config/macrdp/codesign/`，不在本归档中。

## 三、配置指向自建服务器

**不要直接改 `~/Library/Preferences/com.carriez.FastRustDesk/FastRustDesk2.toml`** ——
那个 root 守护进程持有配置状态并会回写用户配置文件，手改的项会被静默抹掉；
而 LaunchAgent 是 KeepAlive 的，杀进程会立刻被拉起。

正确做法是走 root IPC：

```bash
RD=/Applications/FastRustDesk.app/Contents/MacOS/FastRustDesk
sudo "$RD" --option custom-rendezvous-server rd.example.com:41116
sudo "$RD" --option key '<YOUR_SERVER_PUBLIC_KEY>'
sudo "$RD" --option relay-server rd.example.com:41117
```

写完必须重启才生效：

```bash
pkill -x FastRustDesk
sudo pkill -f "FastRustDesk.app/Contents/MacOS/service"
# 两个 KeepAlive 会自动把它们拉回来
```

也可以用图形界面：设置 → 网络 → ID/中继服务器，效果相同。

### 验证真的切过去了

```bash
grep -E "start rendezvous mediator|Latency of" \
  ~/Library/Logs/FastRustDesk/server/FastRustDesk_rCURRENT.log | tail -2
```

必须看到 `start rendezvous mediator of rd.example.com`。
**`--option <键>` 读回值不可靠（同样的命令有时返回空），以磁盘配置文件和日志为准。**

## 四、固定密码

只能在**本机**设（图形界面，或 `sudo "$RD" --password <密码>`）。

**不能把别的 Mac 的 `FastRustDesk.toml` 拷过来** —— 密码存盘时用**本机 UUID**
派生的密钥加密（`password_security.rs` 的 `symmetric_crypt` 调 `get_uuid()`），
拷过去解不开。

## 五、可选：允许 IP 直接访问

同网段直连不经任何服务器，是延迟最低的方式：

```bash
sudo "$RD" --option direct-server Y     # 默认监听 41116+2 = 41118
```

客户端连接时**填 IP 不填 ID** 即可。

## 六、卸载

```bash
sudo launchctl bootout system/com.carriez.FastRustDesk_service
launchctl bootout gui/$(id -u)/com.carriez.FastRustDesk_server
sudo rm -f /Library/LaunchDaemons/com.carriez.FastRustDesk_service.plist
sudo rm -f /Library/LaunchAgents/com.carriez.FastRustDesk_server.plist
sudo rm -rf /Applications/FastRustDesk.app
rm -rf ~/Library/Preferences/com.carriez.FastRustDesk ~/Library/Logs/FastRustDesk
```

系统设置里的三项权限条目要手动删（TCC 记录只能图形界面移除）。

## 七、已知问题

> ⚠️ **升级前必须先停掉正在运行的 FastRustDesk，不能直接覆盖 `.app`。**
>
> `ditto` 覆盖 `/Applications/FastRustDesk.app` 时，如果旧进程还在跑，它的
> 可执行文件和 dylib 会被从脚下换掉，进程立刻段错误：
>
> ```
> ERROR [libs/hbb_common/src/platform/mod.rs:50] Got signal 11 and exit. stack:
> _cg_event_tap_callback_internal
> ```
>
> **而且它不会自己恢复**，原因有三条，缺一不可：
>
> 1. 崩溃后进程的退出码是 **0**；
> 2. LaunchAgent 的 KeepAlive 是 `SuccessfulExit=false` —— 只在**非**正常退出时
>    重启，所以 launchd 认为它善终了；
> 3. 随后的 `launchctl bootstrap` 对一个尚未 bootout 的 label 会失败，而失败
>    通常被 `2>/dev/null || true` 吞掉。
>
> 三件事叠起来，机器**静悄悄地变成「离线」**，而现场看起来一切正常：
> `service` 还在跑、`.app` 是新的、配置也对，只有 `--server` 那个 agent 没了。
>
> 判据：`pgrep -x FastRustDesk` 有没有输出。没有就是这个问题，
> `launchctl kickstart -k gui/$(id -u)/com.carriez.FastRustDesk_server` 可立即恢复。
>
> 正确的升级顺序：先 bootout + pkill 新旧两套标签、确认没有进程占用 bundle，
> 再覆盖；装完**实证** `pgrep -x FastRustDesk` 有输出，没有就 kickstart。


- 显示器配置变动（睡眠/唤醒）时 `CGMainDisplayID` 可能变成 1，
  而 SCK 的可共享列表里没有这个 id，此时采集会失败直到显示器醒来。
- 无 `h264_videotoolbox`（见开头）。
- **无法合入上游**：提高了部署目标、删掉了 CGDisplayStream 后端、提交了生成文件。

## 性能与调优

被控端 macOS 的 CPU 占用，5120×2880 显示器、硬件 H265、画面持续变化时
**实测约 9%**（官方版同条件约 45%）。

省下来的开销来自采集与编码链路：直接采集 NV12 而不是采 BGRA 再用 CPU 转、
IOSurface 直送 VideoToolbox 编码、布局吻合时把采集缓冲区直接交给编码器、
以及去掉与 ScreenCaptureKit 重复的整帧比对。

**像素格式是 `420v` 而不是 `420f`**：libyuv 的 `ARGBToNV12` 产出 BT.601
video range，管线和编码器都按这个标定，选全范围会让所有颜色偏移。

### 静止画面几乎不耗 CPU

SCK 对没有变化的桌面回报 `SCFrameStatus::Idle`，这类帧在流回调里就被丢弃，
不会产生任何拷贝或编码。所以**空闲会话的开销接近 0**，
只有画面真在动时才有上表的负载。对比 CPU 时务必制造画面变化，否则两个版本
都是 0，看不出差别。

### 还能再降的两条路（都不需要改代码）

1. **降分辨率**——5120×2880 → 2560×1440 是像素减少到 1/4，采集和编码同步降。
   在客户端工具栏「显示 → 分辨率」里选，或直接改 Mac 的显示器设置。
2. **降帧率**——客户端工具栏「显示 → 帧率」，默认 30。降到 10~15 通常够用。

画质设置对 CPU 帮助最小：它只影响编码码率，不减少采集和格式转换的工作量。

## 关于 `api-server` 与心跳

`api-server` 为空时，上游会拿 ID 服务器地址**减 2 个端口**推导出一个
`http://<host>:<port-2>` 去发心跳（`/api/heartbeat`）——那是 RustDesk Server
Pro 的接口，开源服务端没有。

本分支改为**只有显式设置了 `api-server` 才发心跳**，所以默认不会有这类失败日志。

> 即使那个端口上跑着 FastRDServer 的账号 API，它也**没有 `/api/heartbeat`**。
> 端口有人监听 ≠ 那个接口存在。账号功能走的是客户端主动发起的
> `/api/login`、`/api/ab`，与心跳无关。

## 零拷贝硬件编码

H265 会话默认走 **VideoToolbox 零拷贝路径**：ScreenCaptureKit 产出的 IOSurface
直接提交给编码器，像素全程不经 CPU。

### 确认它是否生效

```bash
grep -E "using zero-copy|new encoder" \
  ~/Library/Logs/FastRustDesk/server/FastRustDesk_rCURRENT.log | tail -2
```

应看到 `using zero-copy VideoToolbox encoder for H265` 与
`new encoder: VTENC(...)`。若显示 `HWRAM(...)` 则说明走的是原路径。

### 回退

不需要重新编译，也不用换二进制：

```bash
launchctl setenv RUSTDESK_VT_ZEROCOPY 0
pkill -x FastRustDesk; sudo pkill -f "MacOS/service"
```

切回：

```bash
launchctl unsetenv RUSTDESK_VT_ZEROCOPY
pkill -x FastRustDesk; sudo pkill -f "MacOS/service"
```

### 适用范围

- **仅 H265**。Apple Silicon 上没有 `h264_videotoolbox`；H265 协商失败时会退到
  VP9 软编，那条路仍走原来的内存帧路径，不受影响。
- 仅 macOS。Windows 端的 DXGI 路径本来就没有这类开销。

### 若画面异常

先用上面的开关回退确认是否与本改动有关，再看日志里有没有
`Annex-B conversion` / `VideoToolbox encode failed` / `carries no IOSurface`。
