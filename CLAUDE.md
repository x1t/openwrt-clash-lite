# CLAUDE.md

本文件给在这个仓库里工作的 AI 编码助手看。用户文档见 [README.md](README.md)。

## 项目一句话

OpenWrt 上的 mihomo 守护进程（Rust，静态 musl，armv7 + aarch64）+ ubus 插件 + LuCI 页面，替代 OpenClash。**目标是省内存、简单、不弄坏用户的网络**。

## 硬性约束（改代码前先看）

1. **接管交给 mihomo**：TUN `auto-route` + `auto-redirect`，守护进程不写 nft/iptables 规则，不改 dnsmasq。唯一例外是 `takeover.rs`：清理上次残留，以及在 `/etc/firewall.include` 缺失时创建带标记的空文件（Kwrt 上没有它 `fw4 reload` 会返回 1）。
2. **TUN 先关后开**：`merge.rs` 生成的配置里 `tun.enable: false`，`supervisor::Startup` 等 `/providers/*` 全部加载完（最多 60 秒）再 `PATCH /configs` 打开。原因：mihomo 在 `tunnel.OnRunning()` 之前会静默丢弃所有连接，先开 TUN 会让局域网断网 10–15 秒。
3. **不用 fake-ip**（用户要求）：订阅里的 `enhanced-mode` 一律改成 `redir-host`。
4. **订阅原文基本不动**：逐行处理，不用 YAML 解析器。只删我们接管的顶层键（见 `OWNED_KEYS`）、`dns.listen`（会和 dnsmasq 抢 53 端口）和 `---` 文档分隔符（mihomo 只读第一个文档）。
5. **配置在 UCI**：`/etc/config/openwrt-clash-lite`，由 `uci.rs` 自己解析和写出（不调用 `uci` 命令，方便在主机上测试）。坏文件要报错，**不能静默重置**（下一次保存会清空用户的订阅列表）。写文件用临时文件 + 改名，权限 600。
6. **下载订阅的 User-Agent 固定为 `xctcc`**，用户的订阅只认这个。
7. **安装后不自动启动、不开开机自启**：会和 OpenClash 抢网络。
8. **升级不能丢数据**：opkg 升级时会运行旧版本的 `prerm`/`postrm`，脚本必须看 `PKG_UPGRADE=1`（OpenWrt 自己的 `default_prerm` 也这么做）。改这些脚本时，要考虑已经装在路由器上的旧脚本会在下一次升级时运行。
9. **ipk 按 opkg 架构名分别打包**：这个 opkg 遇到 `preinst` 失败**不会中止安装**，只能靠 `Architecture` 让 opkg 拒绝。armv7 只列带硬件浮点的架构名。
10. **更新内核要能失败**：空间检查、gzip 流完整、`-v`、对运行中配置 `-t`，全部通过才在同目录改名替换。已是最新版就不下载。
11. **依赖要克制**：HTTP 用阻塞式 `ureq`，不引入 tokio。新增 crate 前先问用户。

## 模块地图

| 文件 | 职责 |
|---|---|
| `main.rs` | 命令行：run/add/edit/update/use/remove/list/status/update-core |
| `supervisor.rs` | 写运行时配置、启动 mihomo、`Startup`（等 provider → 开 TUN）、崩溃退避重启、SIGTERM 优雅停止 |
| `merge.rs` | 我们的头部（控制器、密钥、日志级别、面板、TUN）+ 处理过的订阅 |
| `takeover.rs` | 冲突检测（OpenClash 等内核在跑时拒绝启动）、残留清理、firewall.include |
| `state.rs` / `uci.rs` | UCI 配置 ↔ `State`；`uci.rs` 只认 `config`/`option` 和 uci 的引号规则 |
| `api.rs` | mihomo REST：版本、TUN 状态、provider 是否加载完、打开 TUN |
| `mihomo.rs` | `update-core` |
| `cron.rs` | 每天自动更新订阅：只增删 crontab 里带 `# openwrt-clash-lite` 标记的一行；init 脚本的 `reload_service`（UCI 变更触发）只同步 crontab，不重启、不启动服务 |
| `subscription.rs` | 下载订阅、文件名、校验 |
| `geo.rs` | 把 OpenClash 已有的 Country.mmdb / GeoSite.dat 链接过来，首次启动不用等下载 |
| `sys.rs` | 唯一有系统调用的模块；非 Linux 下是空实现，纯逻辑可以在 Windows 上测 |
| `files/oclite.uc` | rpcd ucode 插件，ubus 对象 `oclite`，调用命令行 |
| `files/openwrt-clash-lite.init` | procd 服务（`term_timeout 20`：守护进程最多等 mihomo 10 秒清理） |
| `files/ipk/*` | 安装脚本；`luci/` 是 LuCI 页面、菜单和权限 |
| `scripts/build-ipk.sh` | 不用 SDK 打 ipk（必须在 Linux/WSL 上跑，否则文件权限不对） |

## 常用命令

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings   # 必须没有任何警告
cargo test
cargo zigbuild --release --target armv7-unknown-linux-musleabihf
cargo zigbuild --release --target aarch64-unknown-linux-musl
wsl -d Debian -- bash scripts/build-ipk.sh <版本> armv7 target/armv7-unknown-linux-musleabihf/release/openwrt-clash-lite dist
```

环境变量 `OCLITE_DIR`、`OCLITE_RUN_DIR`、`OCLITE_UCI`、`OCLITE_MIHOMO` 可以把所有路径挪到 /tmp，真机测试时不碰正式文件。

## 真机测试注意

- 测试路由器上常常同时有 OpenClash。实验前停掉它，结束后**一定要恢复**（脚本里用 trap），再检查：没有 `ip rule` 9000–9010、没有 `inet mihomo` 表、没有 `lite0`、没有残留的 include 文件。
- `procd` 的 `stop`/`restart` 是异步的，检查残留前要等进程真正退出。
- 测接管效果要等规则集加载完（约 10–20 秒），不要清空 data 目录后马上测，否则看到的都是"连接被重置"。
- busybox 可能没有 `timeout`、`stat`、`pkill`、`diff`。
- 24.10 的 rpcd `reload` 不会加载新的 ucode 插件，要 `restart`（会让 LuCI 用户掉线，所以安装脚本只在必要时才重启）。

## 发版

1. 修改 `Cargo.toml` 的 `version`
2. 提交，推到 `main`
3. `git tag -a vX.Y.Z -m vX.Y.Z && git push origin vX.Y.Z`
4. `.github/workflows/build.yml`：fmt、clippy、测试、交叉编译两个架构、`build-ipk.sh`，打标签时把全部 ipk 发布到 Release（标签必须和 `Cargo.toml` 版本一致，否则失败）

## 隐私

- 不要提交真实订阅链接、节点、密钥、路由器地址。`_notes/` 是本地笔记和测试脚本，已在 `.gitignore` 里。
- 提交前 grep 一遍订阅域名和密钥。
- 运行时配置里有节点密码，只写在 `/tmp`（内存），权限 600。

## 代码风格

- 函数短小，注释只写代码本身表达不了的约束和原因。
- 给用户看的页面文字用中文；代码、注释、命令行输出用英文。
- 换行用 LF（`.gitattributes` 已强制）。
