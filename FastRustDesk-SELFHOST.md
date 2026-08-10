# 自建 FastRDServer ID/中继服务器

怎么把 FastRDServer 跑起来，以及运维时要知道的事。

下文一律用 `rd.example.com` 指代你自己的域名，`<YOUR_SERVER_PUBLIC_KEY>`
指代服务器生成的公钥。

## 服务端

| 项 | 值 |
|---|---|
| 版本 | rustdesk-server **1.1.17** (开源版, AGPL-3.0) + 本分支的账号功能 |
| 二进制 | `/home/FastRDSvr/bin/{FastRDServer,FastRDRelay,FastRDServer-utils}` |
| 工作目录 | `/home/FastRDSvr/data`（`id_ed25519` + `db_v2.sqlite3`） |
| 日志 | `/home/FastRDSvr/logs/{FastRDServer,FastRDRelay}.{log,error}` |
| 运行身份 | 专用系统用户 `fastrd`（非 root；端口均 >1024 无需特权） |
| systemd | `FastRDServer.service` / `FastRDRelay.service`，已 enable 开机自启 |

启动参数：

```
FastRDServer -p 41116 -r rd.example.com:41117 -k _
FastRDRelay  -p 41117 -k _
```

`-k _` 表示使用工作目录下的 `id_ed25519` 密钥对，并**强制客户端持有匹配公钥**。
没有公钥的人即使知道域名也用不了这台服务器。

### 端口：只能指定基准，其余是派生的

`-p 41116` 之外的端口**不能分别指定**，服务端和客户端两侧都按同一套公式算：

| 端口 | 协议 | 用途 | 来源 |
|---|---|---|---|
| 41114 | TCP | 账号 HTTP API（登录 / 地址簿） | `port - 2` |
| 41115 | TCP | NAT 类型探测（与 41116 比对回报端口） | `port - 1` |
| 41116 | **TCP + UDP** | ID 注册、打洞信令（UDP 别漏） | `-p` |
| 41117 | TCP | 中继（打洞失败兜底） | hbbr `-p` |
| 41118 | TCP | WebSocket 版 ID 服务 | `port + 2` |
| 41119 | TCP | WebSocket 版中继 | relay `port + 2` |

客户端侧同理：只要把 `custom-rendezvous-server` 写成 `rd.example.com:41116`，
41115（`increase_port(-1)`）和 41114（`increase_port(-2)`）自动跟着走，
不需要也不应该单独配。

> ⚠️ **41114 要单独在云安全组放行**。它不在「21115-21119 → 41115-41119」这批里，
> 容易漏。漏了的表现是：远程控制一切正常，但地址簿登录连不上。

## 客户端配置

**ID/中继服务器**：`rd.example.com:41116`
**Key**：`<YOUR_SERVER_PUBLIC_KEY>`

两端必须填同一组，否则找不到对方。

对应配置项（`FastRustDesk2.toml` 的 `[options]`）：

```toml
custom-rendezvous-server = 'rd.example.com:41116'
key = '<YOUR_SERVER_PUBLIC_KEY>'
relay-server = 'rd.example.com:41117'
```

> ⚠️ **macOS 端不能直接改这个文件**。装了 FastRustDesk 服务后，
> `/Library/LaunchDaemons/com.carriez.FastRustDesk_service.plist` 这个 root 守护进程
> 持有配置状态并会回写用户配置，手改的项会被抹掉；而
> `/Library/LaunchAgents/com.carriez.FastRustDesk_server.plist` 是 KeepAlive 的，
> 杀掉进程会立刻被拉起。**必须走 GUI**（设置 → 网络 → ID/中继服务器）
> 或 `sudo FastRustDesk --option ...`。
>
> Windows 便携运行时没有这个约束，改文件即生效（240 就是这么配的）。

macOS 端的正确做法：

```bash
sudo /Applications/FastRustDesk.app/Contents/MacOS/FastRustDesk --option custom-rendezvous-server rd.example.com:41116
sudo /Applications/FastRustDesk.app/Contents/MacOS/FastRustDesk --option relay-server rd.example.com:41117
```

`--option` 要求「已安装 + root」，而且是通过 IPC 交给**正在运行的** service 改的
（`core_main.rs`）——service 没起来时这条命令什么也不做。

## 账号（地址簿登录）

服务端首次启动会自动建 `user` / `user_token` 表，并创建 **admin/admin**：

```bash
cd /home/FastRDSvr/data
U=/home/FastRDSvr/bin/FastRDServer-utils
$U user list
$U user passwd admin        # 改密码
$U user add <名字>
$U user devices admin       # 这个账号名下有哪些设备
$U user bind admin <设备ID> # 把设备指派给账号
$U user unbind <设备ID>
```

**同一账号名下的设备互相连接不需要密码。** 判定在服务端做：比对请求方的
令牌解析出的用户与目标设备的 `peer.user`，相同则在转发给被控端的 `PunchHole`
上置 `same_account`，被控端据此跳过密码校验。日志里能看到：

```
INFO [src/rendezvous_server.rs] same-account access to <DEVICE-ID> from ..., password not required
```

设备平时靠客户端登录后随 `RegisterPk` 自动绑定，但那要求每台设备都手工登录
一次；`user bind` 让你在服务端直接指派。**设备必须先在服务器注册过**（连过
一次），否则 `peer` 表里没有那一行。

> ⚠️ 免密的代价：**账号令牌是持有即可用的凭证** —— 拿到它就能免密进入该组
> 所有设备，而它在网络上是明文传输的（只有登录载荷做了 sealed box 加密）。
> 私有部署可以接受，公网暴露则不行。
>
> 另外这道放行在 approve_mode 那道闸**之后** —— 设了「必须手动确认」的机器，
> 同组仍然要确认。**分组放宽的是密码，不是同意权。**

密码用 argon2id13 加盐哈希存储。登录载荷走 libsodium sealed box
（匿名公钥加密，无需握手和证书），所以 41114 虽是明文 HTTP，
**密码不会明文过线**。仍然建议不要把 41114 暴露给不必要的来源。

## 运维

```bash
systemctl status FastRDServer FastRDRelay
journalctl -u FastRDServer -f
tail -f /home/FastRDSvr/logs/FastRDServer.log
sqlite3 /home/FastRDSvr/data/db_v2.sqlite3 "SELECT id, created_at FROM peer;"
```

日志里出现 `update_pk <ID> <地址>` 才算某台客户端**真正**接上了。
**只看端口在监听是不够的** —— 密钥不匹配的客户端能连上端口但注册会被拒。

## 备份要点

`/home/FastRDSvr/data/id_ed25519` 是**私钥**，也是这台服务器的身份。
丢了等于换服务器，所有客户端都要改 Key。备份它，且不要外泄。
`db_v2.sqlite3` 存设备表、用户表和地址簿，一并备份。

## 构建方式

这台机器**能访问 github**，所以直接联网编译，不需要 vendor：

```bash
apt-get install -y libssl-dev pkg-config build-essential sqlite3
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
. "$HOME/.cargo/env"
cd <源码目录> && cargo build --release --all-features --locked
```

**源码包里必须带上仓库根目录的 `db_v2.sqlite3`**。sqlx 是**编译期**校验 SQL 的
（`sqlx::query!` 宏），要按 `.env` 里的 `DATABASE_URL` 打开这个库读表结构。
少了它，编译在 `database.rs` 上报一片 `unable to open database file`——
看着像运行时问题，其实是编译期的。这个库里必须已有 `peer` / `user` / `user_token`
三张表，否则查询宏校验不过。

`libssl-dev` 不是可选的：WebSocket 支持（41118/41119）经
`tokio-tungstenite → tokio-native-tls → openssl-sys` 引入 openssl。

## 已知状况

- 云主机上真正的端口准入常在**云安全组**，不在机器里的 iptables/ufw ——
  SSH 进去改不了，要去控制台。机器内看到端口在监听，不代表外面能连上。
- `ALWAYS_USE_RELAY=N` —— 优先打洞，失败才走中继，符合 P2P 预期。
- 客户端 NAT 为对称型时代码里直接跳过打洞走中继，这不是配置问题。
- AGPL-3.0：自建自用无义务；**一旦对外提供服务即有开源义务**。
