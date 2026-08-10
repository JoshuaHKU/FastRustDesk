# FastRDServer 自建服务端 — Ubuntu 安装配置说明

`FastRDServer`（ID/信令）+ `FastRDRelay`（中继）+ `FastRDServer-utils`（管理命令行）。
基线 rustdesk-server 开源版 **1.1.17**（AGPL-3.0，上游 commit
`a7736be5e40f85bfc141120dce587e836e5d4b80`），加上本分支的账号/地址簿功能。

在 Ubuntu 22.04+ / x86_64 上部署。下文用 `rd.example.com` 指代你的域名，
端口以 `-p 41116` 那一套为例（21116 那套同理，见「二、端口」）。

## 包内容

```
server-ubuntu/
├── INSTALL.md                          本文件
├── bin/FastRDServer                    ID/信令服务
├── bin/FastRDRelay                     中继服务
├── bin/FastRDServer-utils              管理命令行（建账号、改密码、诊断）
├── systemd/FastRDServer.service        在用的 systemd 单元
├── systemd/FastRDRelay.service
└── src/FastRDServer-src.tar.gz         源码（含 db_v2.sqlite3，见「五、重新编译」）
```

二进制是 **glibc 动态链接**（非 musl 静态），需要 Ubuntu 22.04 及以上。
更老的发行版请用源码重编。

## 一、部署

```bash
# 1. 系统依赖（WebSocket 端口用到 openssl，不是可选的）
sudo apt-get update && sudo apt-get install -y libssl3 sqlite3

# 2. 目录与专用账号（端口都 >1024，不需要 root）
sudo useradd --system --no-create-home --shell /usr/sbin/nologin fastrd
sudo mkdir -p /home/FastRDSvr/{bin,data,logs}

# 3. 二进制
sudo install -m 755 bin/FastRDServer bin/FastRDRelay bin/FastRDServer-utils /home/FastRDSvr/bin/
sudo chown -R fastrd:fastrd /home/FastRDSvr/data /home/FastRDSvr/logs

# 4. systemd 单元（先按需改 -r 后面的域名）
sudo cp systemd/FastRDServer.service systemd/FastRDRelay.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now FastRDServer FastRDRelay

# 5. 取公钥，客户端要用
sudo cat /home/FastRDSvr/data/id_ed25519.pub; echo
```

启动参数（写在 .service 里）：

```
FastRDServer -p 41116 -r rd.example.com:41117 -k _
FastRDRelay  -p 41117 -k _
```

`-k _` 表示使用工作目录（`/home/FastRDSvr/data`）下的 `id_ed25519` 密钥对，
并**强制客户端持有匹配公钥**。没有公钥的人即使知道域名也用不了这台服务器。
首次启动时若没有密钥对会自动生成。

## 二、端口：只有一个能指定，其余是派生的

`-p` 之外的端口**不能分别指定**，服务端和客户端两侧按同一套公式算：

| 端口 | 协议 | 用途 | 来源 |
|---|---|---|---|
| 41114 | TCP | 账号 HTTP API（登录 / 地址簿同步） | `port - 2` |
| 41115 | TCP | NAT 类型探测（与 41116 比对回报端口，判断锥形/对称） | `port - 1` |
| 41116 | **TCP + UDP** | ID 注册、打洞信令（**UDP 别漏**） | `-p` |
| 41117 | TCP | 中继（打洞失败时兜底） | relay `-p` |
| 41118 | TCP | WebSocket 版 ID 服务 | `port + 2` |
| 41119 | TCP | WebSocket 版中继 | relay `port + 2` |

> ⚠️ **六个都要放行，41114 最容易漏**。它不在「21115-21119 → 41115-41119」
> 这批平移里。漏了的表现很有迷惑性：远程控制完全正常，只有地址簿登录连不上。
>
> 云主机上真正的准入在**云安全组**，不是机器里的 iptables/ufw ——
> SSH 进去改不了，要去控制台。

## 三、验证部署成功

```bash
systemctl is-active FastRDServer FastRDRelay     # 应为 active active
sudo ss -lntup | grep -E ':4111[4-9]'            # 应有 7 行（41116 占 tcp+udp）
sudo tail -f /home/FastRDSvr/logs/FastRDServer.log
```

客户端配好之后，日志里出现 `update_pk <ID> <地址>` 才算真正连通；
也可以查库：

```bash
sqlite3 /home/FastRDSvr/data/db_v2.sqlite3 "SELECT id, created_at FROM peer;"
```

**只看端口在监听是不够的** —— 密钥不匹配的客户端能连上端口，但注册会被拒。

外网可达性要从**外面**测，机器里 `ss` 看到监听不代表安全组放行了：

```bash
for p in 41114 41115 41116 41117 41118 41119; do
  nc -z -w 5 rd.example.com $p && echo "$p ok" || echo "$p BLOCKED"
done
```

## 四、账号

首次启动自动建 `user` / `user_token` 表并创建 **admin/admin**（日志里有提示）。

```bash
cd /home/FastRDSvr/data
/home/FastRDSvr/bin/FastRDServer-utils user list
/home/FastRDSvr/bin/FastRDServer-utils user add <名字>
/home/FastRDSvr/bin/FastRDServer-utils user passwd admin     # 重置密码
/home/FastRDSvr/bin/FastRDServer-utils user disable <名字>
```

密码以 argon2id13 加盐哈希存储。登录载荷走 libsodium sealed box
（匿名公钥加密，不需要握手和证书），所以 41114 虽是明文 HTTP，
**密码不会明文过线**。

## 五、从源码重新编译

```bash
tar -xzf src/FastRDServer-src.tar.gz
cd rustdesk-server
sudo apt-get install -y libssl-dev pkg-config build-essential sqlite3
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
. "$HOME/.cargo/env"
cargo build --release --all-features --locked      # 2 核约 25 分钟
# 产物在 target/release/{FastRDServer,FastRDRelay,FastRDServer-utils}
```

### ⚠️ 源码包里必须带上根目录的 `db_v2.sqlite3`

sqlx 是**编译期**校验 SQL 的（`sqlx::query!` / `query_as!` 宏），编译时会按
`.env` 里的 `DATABASE_URL=sqlite://./db_v2.sqlite3` 打开这个库读表结构。
少了它，编译会在 `src/database.rs` 上报一片：

```
error: error returned from database: unable to open database file
   --> src/database.rs:83:9
```

看着像运行时问题，其实发生在编译期。这个库里必须已有
`peer` / `user` / `user_token` 三张表，否则查询宏校验不过。
它只用于编译校验，**运行时的库是工作目录下那个**，两者互不相干。

### 联网 vs 离线

本项目有 7 个依赖直接指向 github
（`reqwest / confy / sysinfo / default_net / machine-uid / tokio-socks /
async-speed-limit`）。

- **构建机能访问 github 时**，直接联网编译即可（上面的命令）。
- 若目标机访问不了 github，就要在能访问的机器上
  `cargo vendor`，把 572 个 crate 连源码打包过去，配好 `.cargo/config.toml`
  的 source replace，再 `cargo build --offline`。

### 打包源码时的两个坑

1. **tar 的 exclude 要锚定**：`--exclude='target'` 会匹配任意层级，
   把 `vendor/cc/src/target/` 也排除掉，编译时报
   `failed to open file vendor/cc/src/target/apple.rs`。
   要写 `--exclude='./rustdesk-server/target'`。
2. **在 macOS 上打包必须 `COPYFILE_DISABLE=1`**：否则会带出三万多个 `._*`
   AppleDouble 文件，`ring` 的 build.rs 遍历源码目录撞上不在清单里的
   `._x86asm.pl` 会直接 panic。加 `--no-xattrs` 可顺带压掉 Linux 解包时
   刷屏的 `LIBARCHIVE.xattr.com.apple.provenance` 警告。

### 二进制哈希对不上是正常的

重新编译出的 `FastRDServer` 与归档的二进制**哈希不同**。Rust 默认不做可复现
构建，会把源码绝对路径编进二进制。版本号一致即可，不必追求哈希相同。

## 六、运维

```bash
journalctl -u FastRDServer -f
tail -f /home/FastRDSvr/logs/{FastRDServer,FastRDRelay}.log
systemctl restart FastRDServer FastRDRelay
```

**`/home/FastRDSvr/data/id_ed25519` 是服务器私钥，务必备份且不要外泄。**
丢了等于换服务器身份，所有客户端都得改 Key。
`db_v2.sqlite3`（设备表 + 用户表 + 地址簿）一并备份。

## 七、从旧的 hbbs/hbbr 升级

新单元 `FastRDServer.service` 与旧的 `rustdesk-hbbs.service` 是两个独立单元，
systemd 不会替换，**两套一起跑会抢同一批端口**，表现为随机连不上。

```bash
sudo systemctl disable --now rustdesk-hbbs rustdesk-hbbr
sudo rm -f /etc/systemd/system/rustdesk-{hbbs,hbbr}.service
sudo rm -f /usr/bin/{hbbs,hbbr,rustdesk-utils}
sudo systemctl daemon-reload
```

**数据要搬过去**：上游默认的工作目录是 `/var/lib/rustdesk-server`。
复制前先做一次 WAL checkpoint，否则未落盘的写会丢：

```bash
sudo sqlite3 /var/lib/rustdesk-server/db_v2.sqlite3 "PRAGMA wal_checkpoint(TRUNCATE);"
sudo cp -a /var/lib/rustdesk-server/{id_ed25519,id_ed25519.pub,db_v2.sqlite3} \
           /home/FastRDSvr/data/
sudo chown fastrd:fastrd /home/FastRDSvr/data/*
sudo chmod 600 /home/FastRDSvr/data/id_ed25519
```

搬完核对公钥没变 —— 公钥不变，客户端一个都不用改 Key：

```bash
sudo cat /home/FastRDSvr/data/id_ed25519.pub; echo
```

## 八、注意事项

- `ALWAYS_USE_RELAY=N`（默认）：优先 P2P 打洞，失败才走中继。
  想强制全部走中继（便于排查）设环境变量 `ALWAYS_USE_RELAY=Y`。
- 客户端 NAT 为**对称型**时代码里直接跳过打洞走中继，这不是配置问题。
- **AGPL-3.0**：自建自用无义务；一旦对外提供服务即有开源义务。
