# FastRustDesk Windows 客户端（改造版）— 安装配置说明

`FastRustDesk-win-x64.zip` — Windows 10/11 x64，含 hwcodec（H265 硬件解码）。

## 说明

Windows 端的代码与官方版**没有行为差异**（改造集中在 macOS 采集层）。
本包的价值是：与自建服务器配套、版本可控、输入框提示补了「或 IP 地址」。

> **装成服务和便携运行，配置的归属完全不同** —— 装成服务后必须改服务那份
> 配置，改用户那份没有任何效果。见第二节末尾的警告。
> 便携方式适合临时场合，装成服务才能在注销/锁屏状态下被控。

## 一、便携方式运行

解压到任意目录即可，例如 `D:\Apply\FastRustDesk\`：

```
FastRustDesk.exe              0.35 MB   Flutter 启动器
librustdesk.dll          34.6 MB    真正的程序主体
flutter_windows.dll      18.2 MB
*_plugin.dll             各插件
data\                    Flutter 资源
```

> `FastRustDesk.exe` 只有 0.35 MB 是正常的 —— Flutter 桌面版的结构就是启动器 + DLL，
> **不要以为包不完整**。全部文件必须放在同一目录。

双击 `FastRustDesk.exe` 运行。便携运行**不会**注册 Windows 服务，
因此配置文件改了就生效（和 macOS 不同）。

## 二、配置指向自建服务器

图形界面：设置 → 网络 → ID/中继服务器，填

```
ID 服务器: rd.example.com:41116
Key:       <YOUR_SERVER_PUBLIC_KEY>
```

或直接改配置文件 `%APPDATA%\FastRustDesk\config\FastRustDesk2.toml`
（改之前先退出 FastRustDesk.exe）：

```toml
[options]
custom-rendezvous-server = 'rd.example.com:41116'
key = '<YOUR_SERVER_PUBLIC_KEY>'
relay-server = 'rd.example.com:41117'
```

重新启动后，文件里的 `rendezvous_server` 会自动变成 `rd.example.com:41116` ——
**这一行自动改变就是配置生效的标志**。

### 用 PowerShell 批量配置

```powershell
$cfg = "$env:APPDATA\FastRustDesk\config\FastRustDesk2.toml"
Get-Process FastRustDesk -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep 3
$txt = [System.IO.File]::ReadAllText($cfg)
if ($txt -notmatch '(?m)^\[options\]') { $txt = $txt.TrimEnd() + "`n`n[options]`n" }
foreach ($kv in @(
    @('custom-rendezvous-server','rd.example.com:41116'),
    @('key','<YOUR_SERVER_PUBLIC_KEY>'),
    @('relay-server','rd.example.com:41117'))) {
  $k=$kv[0]; $v=$kv[1]; $pat="(?m)^"+[regex]::Escape($k)+" = .*$"
  if ($txt -match $pat) { $txt=[regex]::Replace($txt,$pat,"$k = '$v'") }
  else { $txt=$txt.TrimEnd()+"`n$k = '$v'`n" }
}
[System.IO.File]::WriteAllText($cfg, $txt, (New-Object System.Text.UTF8Encoding $false))
Start-Process "D:\Apply\FastRustDesk\FastRustDesk.exe"
```

> 通过 SSH 远程执行时，**不要把 PowerShell 内联在命令行里** ——
> cmd 会吃掉管道、引号和正则，报出 `'win32' 不是内部或外部命令` 之类的怪错。
> 把脚本存成 `.ps1` 传过去，用
> `powershell -ExecutionPolicy Bypass -File <路径>` 执行。
>
> 另外 `.ps1` 里**一个非 ASCII 字符都不能有**：PowerShell 5.1 把无 BOM 的
> UTF-8 当 GBK 解析，中文注释会让整个脚本报语法错。

### ⚠️ 装成服务后，改用户那份配置是无效的

服务以 **LocalSystem** 身份运行，它读的是自己那份配置：

```
C:\Windows\ServiceProfiles\LocalService\AppData\Roaming\FastRustDesk\config\FastRustDesk2.toml
```

而不是 `%APPDATA%\FastRustDesk\config\` 下那份。**被控端的注册行为由服务那份
决定** —— 只改用户那份，界面上看着变了，服务器那边纹丝不动。
（这和 macOS 上 root 守护进程持有配置是同一个道理。）

两种正确做法：

```powershell
# 甲：已安装时走 IPC 交给正在运行的服务（推荐）
& "C:\Program Files\FastRustDesk\FastRustDesk.exe" --option custom-rendezvous-server rd.example.com:41116
& "C:\Program Files\FastRustDesk\FastRustDesk.exe" --option relay-server rd.example.com:41117

# 乙：停服务 -> 两份配置都改 -> 起服务（便携装法下 --option 不认「已安装」，只能这样）
Stop-Service FastRustDesk -Force
#   ...改 %APPDATA% 和 ServiceProfiles\LocalService 两份...
Start-Service FastRustDesk
```

判据仍然是 `rendezvous_server` 那行**自动变成新地址**。

## 三、连接

- **填 ID**：走自建服务器，先打洞，失败才走中继
- **填 IP**：直连，不经任何服务器（对方需开「允许 IP 直接访问」）

连上后看工具栏锁形图标的提示文字判断实际路径：

| 提示 | 含义 |
|---|---|
| 加密直连 | P2P 成功 |
| 加密中继连接 | 打洞失败，走了中继 |

## 四、注意

- 客户端和被控端**必须填同一组 ID 服务器 + Key**，否则互相找不到。
- 不要填官方公共服务器 —— 它现在因滥用问题要求登录。
- 便携运行的配置在 `%APPDATA%\FastRustDesk\config\`，**不在程序目录**，
  升级程序不会丢配置；换用户账号则要重配。

---

# 安装程序（预置服务器配置）

`FastRustDesk-1.4.9-host=rd.example.com,key=...,.exe` — 22.5 MB 单文件，
双击即装，**安装后自动指向自建服务器，不需要手工填 ID 服务器和 Key**。

## 原理：配置编码在文件名里

这是 RustDesk 原生机制（`src/custom_server.rs` 的 `get_custom_server_from_string`），
不是外挂的补丁。程序启动时解析自己的文件名：

```
FastRustDesk-1.4.9-host=rd.example.com,key=<YOUR_SERVER_PUBLIC_KEY>,.exe
                    └──────┬──────┘     └───────────────────┬────────────────────┘  │
                        ID 服务器                          公钥                  结尾分隔符
```

规则：

- 从 `host=` 开始截取，按 `,` 分段，支持 `host=` / `key=` / `api=` / `relay=`
- **结尾那个逗号不要省** —— Windows 遇到重名会插入 `(1)` 变成
  `xxx (1).exe`，有结尾逗号才不会把 key 弄脏
- 便携包会把外层文件名通过环境变量传给解包后的程序，所以自解压这层不影响解析

## 为什么 Key 能放进文件名

FastRDServer 生成密钥对时会刻意重试，直到公钥里不含 `/` 和 `:`
（`libs/hbb_common` 的 `gen_sk`），就是为了能安全地放进文件名。

**但带端口的地址一概放不进去** —— 冒号在 Windows 文件名里非法。这影响两项：

- `relay=` 放不进去。不过不需要：FastRDServer 用 `-r` 启动，会把中继地址
  下发给客户端，客户端自动学到。
- **`host=` 也只能写主机名，带不了 `:41116`。** 服务器搬到非默认端口之后，
  这是个真问题：装完客户端会按默认的 21116 去连 `rd.example.com`，
  那个端口上没有东西，表现为「装好了但连不上」。

所以**装完必须补一步设端口**（已安装状态下走 IPC）：

```powershell
& "C:\Program Files\FastRustDesk\FastRustDesk.exe" --option custom-rendezvous-server rd.example.com:41116
& "C:\Program Files\FastRustDesk\FastRustDesk.exe" --option relay-server rd.example.com:41117
```

设完 `rendezvous_server` 会自动变成 `rd.example.com:41116`，那才算好。

## 用法

> ⚠️ **从旧的 RustDesk 升级，必须先卸载旧版。**
>
> 安装目录和注册表项都由程序名派生（`get_default_install_path()` →
> `%ProgramFiles%\<应用名>`），改名后新版装到 `C:\Program Files\FastRustDesk`，
> 与 `C:\Program Files\RustDesk` **并存而不是覆盖**。两套都注册了 Windows 服务，
> 会抢同一个 ID 和同一批端口。先卸干净：
>
> ```powershell
> & "C:\Program Files\RustDesk\RustDesk.exe" --uninstall
> Get-Process rustdesk -ErrorAction SilentlyContinue | Stop-Process -Force
> ```
>
> 配置会自动继承：首次启动时 `FastRustDesk` 发现自己的配置目录不存在，
> 会把旧的 `%APPDATA%\RustDesk\config\` 整个复制过来并改名
> （`Config::migrate_legacy_config_dir()`）。**复制不是移动**，旧目录留着不动，
> 万一要退回旧版还在。

```powershell
# 图形安装
.\"FastRustDesk-1.4.9-host=rd.example.com,key=...,.exe"

# 静默安装（装到 C:\Program Files\FastRustDesk）
.\"FastRustDesk-1.4.9-host=rd.example.com,key=...,.exe" --silent-install
```

装完检查配置是否带上了：

```powershell
Select-String -Path "$env:APPDATA\FastRustDesk\config\FastRustDesk2.toml" -Pattern "rendezvous|key"
```

应当看到 `custom-rendezvous-server = 'rd.example.com:41116'` 和 `key = '...'`。

> 本包实测验证过：安装前先删掉 `FastRustDesk2.toml`，安装后这两项重新出现，
> 证明确实来自文件名而非残留配置。

## 换服务器怎么办

**改文件名即可，不用重新编译。** 把 `host=` 和 `key=` 换成新服务器的地址和公钥。

同目录下另有一份 `FastRustDesk-1.4.9-install.exe`（不带配置），
适合手工填服务器的场合。

## 自己重新打包

```powershell
cd <源码目录>
$env:VCPKG_ROOT="D:\Apply\vcpkg"; $env:PATH="D:\Apply\flutter\bin;"+$env:PATH
python build.py --flutter --hwcodec --portable
```

产物 `FastRustDesk-1.4.9-install.exe` 在仓库根目录，改名即可。

> 打包器是仓库自带的 `libs/portable/generate.py`（需要 `brotli`），
> 不需要 Inno Setup / NSIS。

### ⚠️ 重新打包时必须强制重编打包器

自解压安装程序不会自动跟着 `data.bin` 走。
`generate.py` 只产出 `data.bin` 和 `app_metadata.toml`，真正的单文件安装程序是
`libs/portable` 这个 Rust crate（`build.rs` 把 `data.bin` 嵌进去）。而 `build.rs`
**没有声明 `data.bin` 为 rerun-if-changed**，所以 cargo 认为 crate 是新鲜的，
直接给你一个嵌着**上一次 payload** 的 exe —— 表现为 `Finished in 0.4s`，
产物看着正常，装出来却是旧版本。

```powershell
cargo clean --release -p rustdesk-portable-packer
cargo build --release -p rustdesk-portable-packer
```

判据：重编后 `rustdesk-portable-packer.exe` 的哈希必须变。没变就是没重编。
