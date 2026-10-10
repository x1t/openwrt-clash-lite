<div align="center">

# 🐱 OpenWrt Clash Lite

**给 OpenWrt 用的轻量透明代理：一个 1.4 MB 的守护进程 + [mihomo](https://github.com/MetaCubeX/mihomo) 内核 + 一个 LuCI 页面**

OpenClash 的轻量替代品。不跑一堆 shell 脚本，不改 dnsmasq，不手写防火墙规则。

[![Release](https://img.shields.io/github/v/release/x1t/openwrt-clash-lite?style=flat-square&color=4CAF50)](https://github.com/x1t/openwrt-clash-lite/releases/latest)
[![Build](https://img.shields.io/github/actions/workflow/status/x1t/openwrt-clash-lite/build.yml?style=flat-square&label=build)](https://github.com/x1t/openwrt-clash-lite/actions/workflows/build.yml)
![Platform](https://img.shields.io/badge/OpenWrt-armv7%20%7C%20aarch64-00B5E2?style=flat-square&logo=openwrt)
![Rust](https://img.shields.io/badge/made%20with-Rust-B7410E?style=flat-square&logo=rust)

</div>

---

## ✨ 为什么做这个

OpenClash 很强大，但它是几千行 shell 脚本加一个 Lua 页面，启动时要改 dnsmasq、写 iptables/nftables 规则、跑看门狗。
Clash Lite 让 mihomo 自己用 **TUN 的 `auto-route` + `auto-redirect`** 接管网络，守护进程只负责启动、监视和清理：

| 作者路由器实测（同一份订阅） | 守护进程常驻内存 | mihomo 内存 |
|---|---:|---:|
| armv7（Kwrt 25.12） | **1.9 MB** | 50 MB |
| aarch64（Kwrt 24.10） | **2.3 MB** | 57 MB |

> mihomo 的内存和订阅大小有关，作者的订阅有 17 个规则集。数字只是两台路由器上的测量结果，仅供参考。

## 📦 功能

- 🛡️ **TUN 透明代理**：局域网设备和路由器自己的流量都接管，DNS 由 mihomo 劫持（只用 redir-host，不用 fake-ip）
- 🚦 **启动不断网**：规则集加载完才打开 TUN，加载期间局域网照常直连；停止或崩溃时 mihomo 自己清掉路由和防火墙规则
- 🔁 **自动恢复**：mihomo 崩溃后按 2、4、8…30 秒退避重启；守护进程被杀，mihomo 也会跟着退出并清理
- 🔗 **订阅**：添加、编辑（名称、链接、User-Agent）、更新、切换、删除；User-Agent 默认 `xctcc`，每个订阅可以单独设置
- ⏰ **每天自动更新订阅**：页面上开关、设时间；内容有变化才重启，没变化不断网
- 📊 **Yacd 面板**：打包在 ipk 里，页面上一键打开并自动登录
- ⬆️ **更新内核**：一键安装 mihomo 官方最新版，带空间检查和配置检查，失败不影响旧内核（见下文）
- 🔄 **一键更新 Clash Lite**：页面上点「更新 Clash Lite」，自动下载适合本机架构的新版 ipk 并安装，订阅、内核、开机自启都保留
- 🧩 **LuCI 页面**：「服务 → Clash Lite」，状态、启停、开机自启、订阅、设置、日志都在一页
- 🔌 **ubus 接口**：`ubus call oclite …`，脚本和其它程序也能控制
- ⚙️ **UCI 配置**：`/etc/config/openwrt-clash-lite`，可以用 `uci set` 修改，系统备份时会一起保留

## 🚀 安装

1. 看路由器的架构名：

   ```sh
   opkg print-architecture
   ```

   最后一行是 CPU 的架构名，比如 `arm_cortex-a5_vfpv4`、`aarch64_generic`、`aarch64_cortex-a53`。

2. 从 [Releases](https://github.com/x1t/openwrt-clash-lite/releases/latest) 下载两个文件：
   - `openwrt-clash-lite_<版本>_<你的架构名>.ipk`
   - `luci-app-openwrt-clash-lite_<版本>_all.ipk`

3. 传到路由器的 `/tmp` 后安装：

   ```sh
   opkg install /tmp/openwrt-clash-lite_*.ipk /tmp/luci-app-openwrt-clash-lite_*.ipk
   ```

   架构不对时 opkg 会直接拒绝安装。依赖的 `kmod-tun`、`kmod-nft-nat`、`rpcd-mod-ucode`、`ucode-mod-fs` 会自动装上。

4. 打开后台「服务 → Clash Lite」：粘贴订阅链接，点「添加订阅」，再点「启动」。如果路由器上没有 mihomo，先点「更新内核」。

> 安装后**不会自动启动**，也不会开启开机自启，避免和正在运行的 OpenClash 抢网络。

### 支持的架构

| CPU | 架构名 |
|---|---|
| armv7（带硬件浮点） | `arm_cortex-a5_vfpv4` `arm_cortex-a7_neon-vfpv4` `arm_cortex-a7_vfpv4` `arm_cortex-a8_vfpv3` `arm_cortex-a9_neon` `arm_cortex-a9_vfpv3-d16` `arm_cortex-a15_neon-vfpv4` |
| aarch64 | `aarch64_generic` `aarch64_cortex-a53` `aarch64_cortex-a72` `aarch64_cortex-a76` |

没有硬件浮点的 armv7（比如 `arm_cortex-a9`，博通 Northstar）不支持。

## 🔄 和 OpenClash 来回切换

两者不能同时接管网络。Clash Lite 发现 OpenClash 在运行时会拒绝启动，页面上会出现黄色提示和「停止 OpenClash」按钮。

**换成 Clash Lite**
1. 「服务 → OpenClash」点「停止 OpenClash」。OpenClash 会把自己的开关 `openclash.config.enable` 设成 0，以后开机也不会启动
2. 「服务 → Clash Lite」点「启动」，再点「开启开机自启」

**换回 OpenClash**
1. 如果在「系统 → 启动项」里禁用过 `openclash`，先改回「已启用」
2. 「服务 → Clash Lite」点「停止」，再点「关闭开机自启」
3. 「服务 → OpenClash」点「启动 OpenClash」

> 换成 Clash Lite 后，可以在「系统 → 启动项」里禁用 `openclash`：它的开机脚本即使不启动也会空转约 35 秒，禁用后开机能早这么久接管。

命令行：

```sh
# 换回 OpenClash
/etc/init.d/openwrt-clash-lite stop; /etc/init.d/openwrt-clash-lite disable
uci set openclash.config.enable=1; uci commit openclash; /etc/init.d/openclash start
```

## ⬆️ 更新内核为什么不会弄坏路由器

更新内核只是替换一个普通程序文件 `/etc/openwrt-clash-lite/mihomo`，不碰固件、引导程序和系统内核，所以不会变砖。更新时依次检查：

1. 当前用的内核已经是最新版：直接提示「已是最新」，不下载，不重启
2. 闪存剩余空间不够（下载大小的 3 倍再加 32 MB）：拒绝更新
3. 边下载边解压，下载不完整会报错
4. 运行新内核 `-v`，确认能运行
5. 服务在运行时，用新内核 `-t` 检查当前配置，新内核读不了就保留旧内核
6. 在同一个目录里原子改名替换，文件要么全是旧的、要么全是新的

任何一步失败，旧内核都原封不动。想退回系统自带的内核，删掉 `/etc/openwrt-clash-lite/mihomo` 再重启服务即可。

找内核的顺序：环境变量 `OCLITE_MIHOMO` → `/etc/openwrt-clash-lite/mihomo` → `/usr/bin/mihomo` → OpenClash 的 `/etc/openclash/core/clash_meta`。

## 🔌 ubus 接口

| 方法 | 参数 | 作用 |
|---|---|---|
| `status` | | 运行状态、版本、内存、当前订阅、服务和开机自启状态、OpenClash 是否在运行 |
| `list` | | 订阅列表 |
| `add` | `url` `ua`（可选） | 添加订阅 |
| `edit` | `id` `name` `url` `ua` | 改名称、链接或 User-Agent（新链接 / 新 UA 会先下载验证，`ua` 为空表示默认 `xctcc`） |
| `update` | `id`（可选） | 重新下载订阅，不填就是当前订阅 |
| `use` | `id` | 切换当前订阅 |
| `remove` | `id` | 删除订阅 |
| `service` | `action` | `start` `stop` `restart` `enable` `disable` |
| `update_core` | | 后台更新内核，用 `log` 查看进度 |
| `update_self` | | 后台更新 Clash Lite 本身，用 `log {"name":"self"}` 查看进度 |
| `version` | | ubus 插件的版本 |
| `log` | `name` `lines` | `service`（logread）、`mihomo`、`core`（内核更新进度）、`self`（自身更新进度） |

在路由器上：

```sh
ubus call oclite status
ubus call oclite update '{"id":"R1a2b3c"}'
ubus call oclite service '{"action":"restart"}'
```

从局域网用 HTTP 调用（用后台的账号密码登录）：

```sh
# 1. 登录，拿到 ubus_rpc_session
curl -s http://192.168.1.1/ubus -d '{"jsonrpc":"2.0","id":1,"method":"call","params":["00000000000000000000000000000000","session","login",{"username":"root","password":"你的密码"}]}'
# 2. 用 session 调用
curl -s http://192.168.1.1/ubus -d '{"jsonrpc":"2.0","id":2,"method":"call","params":["<session>","oclite","update",{}]}'
```

切换或更新订阅后，服务在运行时要 `service restart` 才生效（页面上会自动做）。

## ⌨️ 命令行

```text
openwrt-clash-lite run              前台运行（procd 用）
openwrt-clash-lite add <url> [ua]   添加订阅
openwrt-clash-lite edit <id> <名称> <url> [ua]
openwrt-clash-lite update [id]      重新下载订阅
openwrt-clash-lite use <id>         切换订阅
openwrt-clash-lite remove <id>      删除订阅
openwrt-clash-lite list [--json]
openwrt-clash-lite status [--json]
openwrt-clash-lite update-core      更新 mihomo
openwrt-clash-lite auto-update      定时任务用：更新当前订阅，有变化才重启
openwrt-clash-lite sync-cron        按 UCI 写入或删除 crontab 里的定时任务
openwrt-clash-lite self-update      更新 Clash Lite 本身
```

## ⏰ 每天自动更新订阅

页面「设置」里打开「每天自动更新订阅」并填时间（默认 06:10），点「保存并应用」即可。也可以用命令：

```sh
uci set openwrt-clash-lite.main.auto_update='1'
uci set openwrt-clash-lite.main.update_time='06:10'
uci commit openwrt-clash-lite
openwrt-clash-lite sync-cron
```

程序会在 `/etc/crontabs/root` 里维护一行带 `# openwrt-clash-lite` 标记的任务，你自己的其他定时任务不受影响；卸载时这一行会被删除。到时间后重新下载当前订阅：内容有变化并且服务在运行才重启，没变化什么都不做。

## 📁 文件

| 路径 | 内容 |
|---|---|
| `/etc/config/openwrt-clash-lite` | 设置和订阅列表（UCI，权限 600，卸载时保留） |
| `/etc/crontabs/root` 里带标记的一行 | 每天自动更新订阅的任务 |
| `/etc/openwrt-clash-lite/` | 下载的订阅、自己更新的内核、规则集缓存 |
| `/tmp/openwrt-clash-lite/` | 运行时配置（含节点密码，只在内存里）、mihomo 日志 |
| `/usr/share/openwrt-clash-lite/ui/` | Yacd 面板 |

面板和 API 端口默认 `19097`，监听所有接口，由密钥保护；OpenWrt 默认防火墙不放行 WAN 入站。

**升级**：页面上点「更新 Clash Lite」，或者手动 `opkg install` 新版本。开机自启设置、订阅、内核、运行状态都会保留，正在运行的服务会自动重启。

一键更新的流程参考 OpenClash：查 GitHub 最新版本 → 按 `opkg print-architecture` 选本机架构的 ipk（装了页面包就一起更新）→ 核对文件大小 → `opkg install --noaction` 试装 → 正式安装。试装失败就什么都不装。在 24.10 这类较老的系统上，安装时会重启一次 rpcd 来加载新的接口，后台页面需要重新登录。

**卸载**：`opkg remove luci-app-openwrt-clash-lite openwrt-clash-lite`。会停止服务、清掉路由和防火墙规则、删除 `/etc/openwrt-clash-lite`，只保留 `/etc/config/openwrt-clash-lite`（重装后订阅还在，不要的话手动删掉）。

## 🛠️ 从源码编译

需要 Rust、[zig](https://ziglang.org/) 和 [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild)：

```sh
cargo test
cargo zigbuild --release --target armv7-unknown-linux-musleabihf
cargo zigbuild --release --target aarch64-unknown-linux-musl
bash scripts/build-ipk.sh 0.1.8 armv7 target/armv7-unknown-linux-musleabihf/release/openwrt-clash-lite dist
```

`build-ipk.sh` 要在 Linux（或 WSL）上运行，Windows 文件系统存不了可执行权限。推送 `v*` 标签后，GitHub Actions 会自动编译并发布全部 ipk。

## 🙏 致谢

- [MetaCubeX/mihomo](https://github.com/MetaCubeX/mihomo)：内核
- [MetaCubeX/Yacd-meta](https://github.com/MetaCubeX/Yacd-meta)：面板
- [vernesong/OpenClash](https://github.com/vernesong/OpenClash)：接管方式和内核更新流程的参考
- [nikkinikki-org/OpenWrt-nikki](https://github.com/nikkinikki-org/OpenWrt-nikki)：LuCI 页面和 rpcd 插件写法的参考

## 📄 许可证

[MIT](LICENSE)
