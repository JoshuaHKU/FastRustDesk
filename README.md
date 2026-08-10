**中文** | [English](README-EN.md)

# FastRustDesk

**Windows ↔ macOS 互控的自建远程桌面**，基于 [RustDesk](https://github.com/rustdesk/rustdesk) v1.4.9 的**非官方分支**。

配套服务端：**FastRDServer**（同一作者，另一个仓库）—— 两者搭配才是完整的一套。

---

> ### ⚠️ 非官方分支声明
>
> 本项目是 RustDesk 的**非官方个人分支**，与 RustDesk 官方、Purslane Ltd
> **没有任何关联**，未获其认可或背书。`RustDesk` 是 Purslane Ltd 的商标；
> 本项目名称中出现该词仅用于说明血缘关系。
>
> 这是**个人自用**的改造版：**不提供商业支持**，**不承担任何担保**，
> 不保证适用于任何特定用途。请自行评估后使用。
>
> 遇到问题请**不要**向 RustDesk 官方反馈——那不是他们的代码。

---

## 这个分支解决什么问题

一台 Mac 和几台 Windows，想要**不经第三方服务器**地互相远程控制，并且**画面流畅、
被控端不烫手**。

官方版在这个场景下有三个具体障碍，本分支逐一处理了：

| 障碍 | 本分支的做法 |
|---|---|
| macOS 被控端 CPU 占用高（5K 屏满负载 45%+） | 采集与编码链路重写，**降到 9%** |
| macOS 采集用的 `CGDisplayStream` 已被 Apple 弃用 | 换成 **ScreenCaptureKit** |
| 自建服务器时「可访问的设备」页只能显示一个登录按钮（那是商业版功能） | 改为**直接向自建服务器查询设备列表**，点击即连 |

在此之上还加了自建环境下真正好用的几项：账号分组、同组免密、地址簿自动补全。

## 主要改进

### 1. macOS 被控端 CPU 占用降到 1/5

实测同一台 5120×2880 的 Mac、同一负载、同一会话：**45.6% → 9.0%**。

| 改动 | 省下了什么 |
|---|---|
| **直接采集 NV12**，不再采 BGRA 再用 CPU 转 | 每帧 1470 万像素的色彩空间运算；缓冲区 59 MB → 22 MB |
| **VideoToolbox 零拷贝编码**：IOSurface 直送编码器 | 像素全程不经 CPU |
| 布局吻合时把采集缓冲区直接交给编码器 | 每帧 22 MB 读 + 22 MB 写 |
| 去掉与 ScreenCaptureKit 重复的整帧去重 | 每帧 59 MB memcmp + 59 MB 拷贝 |
| 采集帧率上限 60 → 30 | 约一半的回调本来就是白做的 |

零拷贝那条路**不需要 Metal**——`VTCompressionSessionEncodeFrame` 本来就收
CVPixelBuffer / IOSurface。这一点在动手前验证过，省掉了一整套 Metal 管线。

> 静止画面几乎不耗 CPU：ScreenCaptureKit 对无变化的桌面回报 `Idle`，这类帧在
> 回调里就被丢弃。**对比 CPU 时务必制造画面变化**，否则两个版本都是 0。

### 2. macOS 采集换成 ScreenCaptureKit

Apple 已在 macOS 14 弃用 `CGDisplayStream`。新后端在 `libs/scrap/src/sck/`，
最低系统要求因此从 10.14 提到 **12.3**。

顺带修掉了移植中暴露的健壮性问题：显示器睡眠/唤醒时 SCK 会短暂返回空列表，
把首次答复当真会让一次瞬时抖动变成**永久黑屏**；流被停止后上层无感知，
会话会冻结在最后一帧。

### 3. 「可访问的设备」：自建服务器也能用

官方的这个页签是**商业版功能**——数据来自 Server Pro 的 HTTP API，且要求账号登录。
开源服务端没有那个接口，所以自建环境下它永远只是一个登录按钮。

本分支改为**直接通过信令协议向自建服务器查询**：无需登录，列出服务器上的设备，
点击即连。显示名的优先级是 **你起的别名 → 主机名 → ID**。

> 枚举接口**强制校验服务器密钥**。相邻的在线状态查询上游是不校验的——那没问题，
> 因为它只能确认调用方**已知 ID** 的设备状态；枚举不同，不校验的话任何能连到
> 信令端口的人都能把服务器上全部设备的 ID 和 IP 扒下来。

### 4. 账号、分组与同组免密（需搭配 FastRDServer）

- 服务端 SQLite 增加用户表，支持建号／改密／登录
- 同一账号下的设备自动归为一组，「可访问的设备」按组过滤
- **同一账号内的设备互相连接不需要输密码**
- 启动时自动把组内设备补进地址簿

> 同组免密的判定在**服务端**完成，结果随信令下发给被控端。这道放行**位于
> 「手动确认」那道闸之后**——设了必须点同意的机器，同组仍然要点。
> **分组放宽的是密码，不是同意权。**
>
> 代价要说清楚：账号令牌是**持有即可用**的凭证，拿到它就能免密进入该组所有
> 设备，且它在网络上是明文的（只有登录载荷做了加密）。自建自用可以接受，
> 公网暴露则不行。

### 5. 质量监测窗显示 P2P 状态

勾选「显示质量监测」后，右上角小窗新增一行 `P2P: Yes/No`：
**绿色 Yes = 直连**，**橙色 No = 经服务器中继**，灰色 `-` = 连接尚未建立。

中继不是错误，但它多一跳延迟、占服务器带宽。以前只能从工具栏图标的提示文字里
猜，现在一眼可见。

### 6. Windows 安装程序可预置服务器配置

安装程序把服务器地址和公钥编码在**自己的文件名**里，双击即装，装完自动指向你的
服务器，不需要手工填。

> 冒号在 Windows 文件名里非法，所以本分支用 **`+` 代替冒号**来携带端口：
> `host=rd.example.com+41116` → `rd.example.com:41116`。
> 上游只能写裸主机名，对着非默认端口的服务器会静默连错端口。

## 搭配 FastRDServer 能得到什么

单独用本客户端 + 官方开源服务端，你得到的是「性能更好的 RustDesk」。
配上 **FastRDServer** 才有完整的一套：

| 能力 | 只有客户端 | 客户端 + FastRDServer |
|---|---|---|
| 自建服务器、不经第三方 | ✅ | ✅ |
| macOS 低 CPU 占用 | ✅ | ✅ |
| 「可访问的设备」列出服务器上的设备 | ✅ | ✅ |
| 账号登录（服务端命令行建号） | ❌ | ✅ |
| 设备按账号分组，只看得到自己的 | ❌ | ✅ |
| **同组设备互连免密** | ❌ | ✅ |
| 地址簿多端同步 | ❌ | ✅ |

FastRDServer 的详细说明见它自己的 README 与
[INSTALL-FastRDServer-ubuntu.md](INSTALL-FastRDServer-ubuntu.md)。

## 平台支持

| 平台 | 状态 |
|---|---|
| **macOS**（Apple Silicon，12.3+） | 主控 + 被控，性能优化全部生效 |
| **Windows** 10/11 x64 | 主控 + 被控，功能完整 |
| Linux / Android / iOS | 上游代码保留，**未在本分支测试** |

性能优化是 macOS 采集与编码链路上的，**不会平移到其他平台**；账号、分组、
设备列表、P2P 显示这些是跨平台的。

## 编译

macOS：

```bash
export VCPKG_ROOT=~/vcpkg
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
export PATH="$HOME/flutter/bin:$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
python3 build.py --flutter --hwcodec
```

Windows（Flutter 桌面端**无法从 macOS 交叉编译**，必须在 Windows 上编）：

```powershell
$env:VCPKG_ROOT = "D:\vcpkg"
$env:VCPKG_DEFAULT_HOST_TRIPLET = "x64-windows-static"
python build.py --portable --flutter --hwcodec
```

完整前置条件与必须钉住的版本，见 **[FastRustDesk.md](FastRustDesk.md)**。

## 文档

| 文件 | 内容 |
|---|---|
| [FastRustDesk.md](FastRustDesk.md) | 构建与部署：改动的技术说明、完整构建步骤、部署注意事项 |
| [FastRustDesk-SELFHOST.md](FastRustDesk-SELFHOST.md) | 自建服务器的部署与运维 |
| [INSTALL-FastRustDesk-macos.md](INSTALL-FastRustDesk-macos.md) | macOS 客户端安装配置 |
| [INSTALL-FastRustDesk-windows.md](INSTALL-FastRustDesk-windows.md) | Windows 客户端安装配置 |
| [INSTALL-FastRDServer-ubuntu.md](INSTALL-FastRDServer-ubuntu.md) | 服务端安装配置 |
| [README-upstream.md](README-upstream.md) | RustDesk 官方原始 README |

文档里所有地址、密钥、设备 ID 都是占位符（`rd.example.com`、
`<YOUR_SERVER_PUBLIC_KEY>` 等），换成你自己的。

## 与上游的关系

基线：`rustdesk/rustdesk` **v1.4.9**（commit `11190fa`）。

**这些改动不适合提交给上游**，也无意提交：提高了 macOS 部署目标、删掉了
CGDisplayStream 后端、改了「可访问的设备」这个商业版页签的语义、
向信令协议加了字段。上游要兼容所有平台、还要给商业版留位置，这些取舍对他们
不成立。

想要官方版请去 [rustdesk/rustdesk](https://github.com/rustdesk/rustdesk)。

## 许可

**AGPL-3.0**，与上游一致 —— 见 [LICENCE](LICENCE)。

作为 AGPL 派生作品，本项目**必须**且**只能**以 AGPL-3.0 发布。
上文「个人自用、不提供商业支持、不承担担保」是作者的**声明**，
**不是**附加的许可条款——AGPL-3.0 不允许附加任何额外限制。

⚠️ **AGPL 的网络条款**：如果你把它改造后**对外提供服务**，就有向使用者
提供对应源码的义务。自建自用没有这个义务。
