# Glacc Auto Linux 服务化重构可行性报告

> 分析对象：`https://github.com/miaocell/glacc-auto`（NOTICE 中的版权方为 `JiangXu26710`）
> 代码版本：v0.2.2，提交 `126637b`
> 分析方式：全量静态阅读 + 上游依赖与接口核对；未修改仓库任何文件
> 报告日期：2026-11-14

> 分析边界：本机未安装 Rust 工具链，本次未做编译验证；也没有使用真实账号触发线上接口。“可行”是源码与依赖层面的判断，最终以 P0 的真实构建与登录结果为准。

---

## 1. 结论

**可行。** 这是一个“外壳重、内核轻”的项目：登录、任务、领取、重试、通知、存储约 5.0k 行 Rust 与操作系统无关；真正绑死 Windows 的只有 5 处 FFI、1 个 `schtasks` 调度模块和 1 个 Tauri 壳，合计约 3.3k 行（按涉及文件计）需要替换或删除。改造后可以做成单个 Linux 二进制：启动后监听 HTTP，浏览器访问，服务器上不需要任何图形环境。

一句话：**业务逻辑不用重写，重写的是“怎么启动、怎么记时间、怎么开锁、怎么加载 TLS 库、怎么把命令暴露成 HTTP”。**

| 项 | 判断 |
|---|---|
| 技术可行性 | 高。协议层是纯 `std` 同步代码，平台相关部分已隔离在少数文件 |
| 预计工作量 | 9–13.5 人日（熟悉 Rust 的一个人）；MVP 6–8 人日 |
| 最大风险 | 三个构建密钥是否可取得；Linux 版 tls-client 指纹是否一致 |
| 建议 | 目标是在 NAS/小主机上 7×24 运行 → 做；只是偶尔用 → Windows 虚拟机更省 |
| 前置验证 | 1–2 天 P0：Linux 编译核心、加载 `.so`、真实账号登录一次、调一个只读接口 |

---

## 2. 项目现状

### 2.1 技术栈与规模

| 项 | 现状 |
|---|---|
| 技术栈 | Tauri v2 + Rust 2021 + Svelte 5 + Tailwind 4 + Vite 7 |
| Rust | 11,254 行：协议核心约 4,979 行，Tauri 壳 1,440 行，测试 4,835 行 |
| 前端 | 约 2,668 行，2 个页面 + 3 个弹窗 + 约 15 个组件 |
| 测试 | 114 个 `#[test]`，6 个 `ignored` |
| 外部依赖 | `user.geilijiasu.net`（账号）、`game-xacc.xunlei.com`（业务）、`sctapi.ftqq.com`（通知）、`tls-client.dll` |
| 数据目录 | `%APPDATA%\glacc-auto`：`accounts/`、`settings.json`、`history.json`、`schedule.json`、`logs/` |
| 许可 | PolyForm Noncommercial 1.0.0；分发时须保留 LICENSE 与 NOTICE |

### 2.2 模块职责

| 模块 | 作用 | 与 OS 的关系 |
|---|---|---|
| `auth.rs` | 验证码、发码、验码、signin、token 刷新 | 无 |
| `game.rs` | 任务列表、任务推送、钱包查询 | 无 |
| `claim.rs` | 单账号一轮领取，阶段间 3s 对账 | 无 |
| `sweep.rs` | 多账号串行，号间 5s，取消令牌 | 无 |
| `session.rs` | 超时、5xx 退避、`10003` 自动刷新 | 无 |
| `credentials.rs` | 凭据读写、设备身份派生 | 无 |
| `store.rs` | 原子写、账号 key、上限 3 | 仅数据根路径 |
| `settings.rs` | 设置模型与夹紧 | 无 |
| `schedule.rs` | Windows 计划任务注册/查询/注销 | **整模块 Windows** |
| `notify.rs` | Server 酱推送 | 无 |
| `diag.rs` | 日志滚动、7 天清理 | 本地时间 |
| `history.rs` | 30 天领取历史 | 无 |
| `tls.rs` | tls-client 动态库封装 | **Windows FFI** |
| `instance.rs` | 跨进程领取互斥 | **Windows FFI** |
| `sign.rs` | 推送签名 MD5 | 无 |
| `lib.rs` | Tauri 命令、窗口、线程编排 | **Tauri/Windows** |

### 2.3 关键发现：设备身份是算出来的

`credentials.rs:120-146` 里，`device_id`、`login_device_id`、`peer_id` 全部由下式派生：

```
sha256(域前缀 + InstallSalt + ":" + 手机号) 的前 16 字节，转 hex
域前缀：glacc-deviceid: / glacc-peerid:
```

`InstallSalt` 是 32 位 hex，存在凭据 JSON 中；UA 与 TLS 指纹再按手机号哈希，从 12 个 Android 档案里选一个（`device.rs` 的 `PROFILES` 与 `select_for_phone`）。

这带来两个直接结论：

1. **换操作系统之后，服务端看到的仍是同一台设备**。同一份数据搬到 Linux，device id、peer id、UA、TLS 指纹保持一致。
2. **迁移时必须保留 `InstallSalt`**。丢了它，peer id 变化，游戏域已签发的 token 可能失效，账号需要重新短信登录。

这是整个移植成立的根，也是它比一般“桌面程序改 Web”容易得多的原因。

---

## 3. 可移植性核对

### 3.1 平台相关代码是穷举的

全仓只有 5 处 FFI：

| # | 位置 | 机制 | 用途 |
|---|---|---|---|
| 1 | `glacc/tls.rs:94-179` | `kernel32` `LoadLibraryW` / `GetProcAddress` | 加载 `tls-client.dll`，取 `request` / `freeMemory` |
| 2 | `glacc/device.rs:86-180` | `bcrypt` `BCrypt*` | SHA-256 与随机数（CNG） |
| 3 | `glacc/diag.rs:148-190` | `kernel32` `GetLocalTime` | 本地时间戳、日志文件名 |
| 4 | `glacc/instance.rs:1-81` | `kernel32` `CreateMutexW` | 跨进程领取互斥 |
| 5 | `glacc/schedule.rs` 全文件 | `schtasks` + 计划任务 XML | 定时领取 |

另有 4 处 `#[cfg(windows)]`（`lib.rs` 2 处、`schedule.rs` 2 处），没有隐藏的 Unix 分支。除此之外，`glacc` 模块只使用 `std`：同步 HTTP、`Mutex`、`Arc`、`AtomicBool`、文件 IO。没有 `tokio`，没有异步传染，没有平台抽象层缺失。

### 3.2 为什么同步引擎反而好移植

领取流程是同步阻塞的（`claim.rs`），依赖通过 `Waiter` / `Sleeper` / `Transport` / `Reporter` 注入。搬到服务端只需要：

- 把整轮领取放进 `tokio::task::spawn_blocking`；
- 用现有的 `AtomicBool` 做取消；
- 进度仍然写进 `LiveBoard`（`Arc<Mutex<...>>`），前端继续 1s 轮询。

不需要把 5k 行改成 `async`，这是工作量的关键分水岭。

### 3.3 测试资产

114 个测试里，约 96 个与平台无关，可直接保留；约 18 个需要重写或删除：

- `tls.rs` 2 个（依赖 DLL 路径）
- `instance.rs` 1 个（命名互斥）
- `schedule.rs` 13 个（schtasks）
- `tests/schedule_live.rs` 1 个
- `tests/e2e_probe.rs` 1 个（`include!` 了 `tls.rs`，依赖 DLL）

传输层可注入意味着大多数单测不触网，这在改传输实现时价值很大。

---

## 4. Windows 依赖替换方案

| # | 位置 | 现状 | Linux 方案 | 改动量 |
|---|---|---|---|---|
| 1 | `glacc/tls.rs` | `LoadLibraryW` 加载 `tls-client.dll` | `libloading` 加载 `libtls-client.so`；探测顺序改为 `GLACC_TLS_CLIENT_LIB` → 二进制同目录 → `/usr/local/lib` → `/app/lib` | 约 80 行 |
| 2 | `glacc/device.rs` | `BCryptHash` / `BCryptGenRandom` | `sha2` + `getrandom` | 约 40 行 |
| 3 | `glacc/diag.rs` | `GetLocalTime` | `chrono::Local`（日志名 `%Y%m%d`） | 约 30 行 |
| 4 | `glacc/instance.rs` | `CreateMutexW` 命名互斥 | 数据目录 `flock`（`fs2` 或 `rustix`），进程崩溃自动释放 | 约 60 行 |
| 5 | `glacc/schedule.rs` | `schtasks` + UTF-16 XML + GBK | 进程内调度 + `state.json` | 删 1,273 行，增约 150 行 |
| 6 | `lib.rs` | 21 个 Tauri 命令、窗口、插件 | `axum` 路由 + 静态托管 + 鉴权 | 重写约 900 行 |
| 7 | `glacc/store.rs:34-42` | `%APPDATA%` | `GLACC_AUTO_DATA`，否则 XDG `~/.local/share/glacc-auto` | 约 10 行 |
| 8 | `Cargo.toml` | `tauri`、`tauri-plugin-dialog` | `axum`、`tokio`、`tower-http`、`tower`、`sha2`、`getrandom`、`libloading`、`chrono`、`fs2`、`argon2`、`hmac` | 依赖替换 |

顺带删除：

- `CREATE_NO_WINDOW`（`lib.rs:27-35`）
- `open_external` 的 `start` 命令（`lib.rs:312`）
- `--scheduled` 分支（`lib.rs:1274`）
- 启动后 6 秒强制显示窗口的兜底逻辑
- `tauri.conf.json`、`build.rs` 的 `tauri_build`、图标与 NSIS 配置

### 4.1 一个容易被忽略的构建事实

`tauri.conf.json` 的 `bundle.resources` 指向：

```
../dist/glacc-auto-v0.2.2-win-x64/tls-client.dll
```

仓库里没有 `dist/`，也没有 `dist-app/`（前端构建产物）。三个密钥在 `constants.rs` 里用 `option_env!` 读取，`.env.example` 全为空；其中 `GLACC_SIGN_KEY` 在测试构建里固定为 `test-only-glacc-sign-key`，生产构建才读环境变量。

也就是说：**这个仓库在当前状态下，缺 DLL、缺前端产物、缺密钥，直接构建会失败。** 移植前本来就要补齐这三样；移植只是把“Windows DLL”换成“Linux `.so`”。

---

## 5. 目标架构

```
浏览器（无客户端安装）
   │  HTTP(S)
   ▼
┌──────────────────────────────────────┐
│ glacc-server（axum，单进程）          │
│  ├─ 会话鉴权（密码 / cookie）         │
│  ├─ REST API（15 个业务端点）         │
│  ├─ 静态资源（dist-app）              │
│  ├─ 调度器（进程内，Asia/Shanghai）   │
│  ├─ 单飞锁 + 取消令牌                 │
│  └─ 日志 / 健康检查                   │
└───────────────┬──────────────────────┘
                │ 同进程函数调用
┌───────────────▼──────────────────────┐
│ glacc-core（原 glacc 模块，原样保留） │
│  auth / game / claim / sweep /        │
│  session / credentials / store / ...  │
│  transport → libloading               │
│  platform  → Linux 实现               │
└───────────────┬──────────────────────┘
                ▼
      libtls-client.so（Go c-shared）
```

### 5.1 进程与线程

- 单进程：一个 `glacc-server`；静态资源内嵌或随包分发。
- HTTP 用 tokio 多线程；领取用 `spawn_blocking`，同一时刻最多一轮。
- 不引入多用户、多租户、任务队列；账号上限仍是 3。
- 前端构建仍然用 Node，但**线上不需要 Node**，只是构建期依赖。

### 5.2 建议的目录结构（最小改动版）

```
glacc/
├── src-tauri/            # 先保留目录名，减少 diff；后续可改 server/
│   ├── Cargo.toml        # 去掉 tauri，加 axum/tokio/...
│   └── src/
│       ├── main.rs       # 新增：启动 axum
│       ├── lib.rs        # 保留核心导出，删 Tauri 命令
│       ├── glacc/        # 原样
│       └── platform/     # 新增：time/hash/rand/lock/paths
├── app/                  # Svelte 源码，改 api.ts 与标题栏
├── deploy/
│   ├── Dockerfile
│   ├── docker-compose.yml
│   └── glacc.service
└── docs/
```

不建议一上来拆 Cargo workspace：先把同一个 crate 编出第二个 bin，能跑通再拆，重构风险更低。

---

## 6. HTTP 接口映射

15 个业务命令一一对应，窗口相关命令删除。

| 原 Tauri 命令 | 新端点 | 说明 |
|---|---|---|
| `get_accounts` | `GET /api/accounts` | 账号列表 |
| `set_active_account` | `POST /api/accounts/active` | body `{key}` |
| `get_snapshot` | `GET /api/snapshot` | 首页快照 |
| `get_claim_progress` | `GET /api/claim/progress` | 1s 轮询 |
| `start_claim` | `POST /api/claim/start` | 忙时 409 |
| `logout_account` | `DELETE /api/accounts/{key}` | key 可省略 |
| `get_settings` | `GET /api/settings` | |
| `save_settings` | `PUT /api/settings` | |
| `apply_schedule` | `POST /api/schedule` | body `{enabled,time}` |
| `send_sms_code` | `POST /api/auth/sms` | 60s 冷却 |
| `login_with_code` | `POST /api/auth/login-code` | |
| `read_logs` | `GET /api/logs?tail=N` | 文件名白名单 |
| `get_history` | `GET /api/history` | |
| `export_credentials` | `POST /api/credentials/export` | 首次 428，带密码重试 |
| `import_credentials` | `POST /api/credentials/import` | multipart |
| — | `POST /api/session/login` | 新增，密码登录 |
| — | `POST /api/session/logout` | 新增 |
| — | `GET /api/session` | 新增，判断登录态 |
| — | `GET /healthz` | 容器健康检查，免鉴权 |
| 5 个窗口命令 | 删除 | 浏览器不需要 |
| `open_external` | 删除 | 前端用 `<a target="_blank">` |

错误模型统一为：

```json
{ "error": "另一轮领取正在进行", "code": "busy" }
```

- `409 busy`：已有领取
- `401 unauthorized`：会话失效
- `428 reauth_required`：导出凭据前需再次输入密码
- `400 bad_request`：参数或导入文件不合法
- `500 internal`：其余，日志里留详情，响应里只留摘要

静态托管：`/api/*` 之外的路径回退 `index.html`（SPA 路由），`/assets/*` 长缓存。

前端 `api.ts` 的 `call()` 是唯一入口，替换它即可，页面方法签名保持不变：

```ts
const ROUTES: Record<string, { method: string; path: (p: any) => string }> = {
  get_accounts:      { method: "GET",  path: () => "/api/accounts" },
  get_snapshot:      { method: "GET",  path: () => "/api/snapshot" },
  start_claim:       { method: "POST", path: () => "/api/claim/start" },
  // ...
};

async function call<T>(cmd: string, payload?: Record<string, unknown>): Promise<T> {
  const r = ROUTES[cmd];
  const res = await fetch(r.path(payload), {
    method: r.method,
    headers: { "Content-Type": "application/json" },
    body: r.method === "GET" ? undefined : JSON.stringify(payload ?? {}),
    credentials: "same-origin",
  });
  if (!res.ok) throw new Error((await res.json()).error ?? res.statusText);
  return res.status === 204 ? (undefined as T) : res.json();
}
```

大约 80 行替换，`Home.svelte`、`Settings.svelte`、`stores.svelte.ts` 不用动业务逻辑。

---

## 7. 前端需要改的地方

| 文件 | 改动 | 量级 |
|---|---|---|
| `app/src/api.ts` | `invoke` → `fetch` 路由表；删窗口方法与 `openExternal` | 约 80 行 |
| `app/src/App.svelte` | 去掉 `windowShow()`；增加会话检查与密码登录页 | 约 60 行 |
| `app/src/lib/components/TitleBar.svelte` | 去掉拖拽/最小化/最大化/关闭，保留标题与主题切换 | 约 40 行 |
| 布局 | 固定 900×600 改为 `max-width` 容器；窄屏折行 | 约 30 行 CSS |
| 导入/导出 | 系统文件框 → `<input type=file>` 与下载链接 | 约 40 行 |
| 设置页 GitHub 链接 | `openExternal` → 普通链接 | 1 行 |

现有视觉与交互可以整体保留，不需要重做 UI。Web 端只增加一个密码登录页；加速器账号的短信登录仍然是业务内弹窗，两者不是一回事。

---

## 8. 调度、并发与恢复

### 8.1 调度

原实现是 `schtasks` 注册一个每天 `HH:MM` 的任务，到点以 `--scheduled` 拉起第二个进程，跑完退出。服务化后改为进程内调度：

```text
loop:
  now = Local::now() in Asia/Shanghai
  target = today at HH:MM
  if now >= target: target = tomorrow at HH:MM
  sleep_until(target)
  if not running and schedule.enabled:
      run_one_sweep()          # 阻塞，spawn_blocking
      state.last_attempt = today
```

补充规则：

- 启动时若 `now > 今天 HH:MM` 且 `last_attempt < 今天`，补跑一次（可配置开关）。
- 手动领取与定时领取共用同一把单飞锁；手动进行中到点，定时跳过并在日志记一条。
- `state.json` 记录 `last_attempt` / `last_success`，与 `schedule.json` 分开放，避免污染现有配置结构。
- 时区显式 `Asia/Shanghai`，不跟随容器默认 UTC；日志里打印生效时区。

### 8.2 并发

三层保护，保持原有“严格串行”的设计：

1. `claim.rs` 的 `SerialGate`：同进程内挡重复调用。
2. `AtomicBool` CAS：HTTP 层快速返回 409。
3. 数据目录 `flock`：防止两个进程（例如误开的第二个容器）同时领取。

注意：`flock` 只覆盖同一数据目录。两份数据副本跑在两个主机上，仍会并发领取同一个账号——部署文档里要写清楚“只跑一个实例”，并停掉 Windows 上的旧计划任务。

### 8.3 长任务与重启

- 一轮 3 个账号约 10–30 分钟（任务数、网络重试决定）。
- `POST /api/claim/start` 立即返回 202，不占住 HTTP 连接；进度靠现有 1s 轮询。
- 浏览器关闭不影响服务端领取。
- 进程重启后 `running` 复位；凭据与历史都是原子写，最多丢当前这一轮的进度，不会写坏文件。
- SIGTERM：置取消令牌 → 等当前 HTTP 请求（15s 超时）→ 最多 60s 退出；容器 `stop_grace_period` 设 90s。

---

## 9. 数据布局与迁移

```
$GLACC_AUTO_DATA（默认 /data，或 ~/.local/share/glacc-auto）
├── accounts/
│   ├── <key>.json          # 账号与 token
│   └── _pending.json       # 登录中间态
├── settings.json
├── schedule.json
├── history.json
├── state.json              # 新增：last_attempt / last_success / schema_version
├── server-secret           # 新增：会话签名密钥，0600
└── logs/
    └── glacc-auto-YYYYMMDD.log
```

迁移步骤：

1. 在 Windows 上退出应用，停用 `glacc-auto-claim` 计划任务。
2. 备份 `%APPDATA%\glacc-auto` 整个目录。
3. 复制到 Linux 数据目录，保持文件名与层级。
4. 检查每个账号 JSON 里 `InstallSalt` 存在且为 32 位 hex；记录迁移前后的 device id 前 8 位做对照（日志里只打前 8 位）。
5. 启动服务，先看 `/api/snapshot` 的登录态；有效则直接可用，失效则短信重登一次。

权限：目录 `0700`，文件 `0600`，容器内用非 root UID（如 1000）。

---

## 10. 鉴权与暴露面

这部分是服务化新增的，也是最容易出事的地方。

| 项 | 设计 |
|---|---|
| 默认监听 | `127.0.0.1:8080`；容器内用 `GLACC_LISTEN=0.0.0.0:8080`，宿主侧只映射到 `127.0.0.1`；监听非回环地址且未设密码时，首启生成随机密码并打印；`GLACC_REQUIRE_PASSWORD=1` 时改为启动失败 |
| 密码 | `GLACC_WEB_PASSWORD`、`GLACC_WEB_PASSWORD_FILE` 或 `GLACC_WEB_PASSWORD_HASH`（argon2id）；首次启动未设置则生成随机密码打印一次 |
| 会话 | HMAC-SHA256 签名 cookie，`HttpOnly` + `SameSite=Strict` + `Path=/`；7 天滑动过期 |
| CSRF | `SameSite=Strict` + 校验 `Origin`/`Referer` + 前端带 `X-Requested-With` |
| 登录限流 | 5 次 / 5 分钟 / IP；失败不区分“密码错”和“用户不存在” |
| 短信接口 | 必须已登录；同一手机号 60s 冷却 |
| 导出凭据 | 需再次输入密码（428 → 重验），响应带 `Content-Disposition: attachment` |
| 日志 | 手机号保持现有脱敏；请求体不入日志；日志接口文件名白名单，防目录穿越 |
| 响应头 | `CSP: default-src 'self'; style-src 'self' 'unsafe-inline'`、`X-Frame-Options: DENY`、`Referrer-Policy: no-referrer` |
| 公网 | 只走反向代理（Caddy/Nginx）加 TLS；容器端口只绑 `127.0.0.1` |
| 密钥 | 构建期 `option_env!` 改为运行时读取优先，避免把 `CLIENT_SECRET`/`SIGN_KEY` 烤进镜像层 |

威胁排序：公网暴露 > 本机其他进程 > 同 LAN。默认配置直接关掉公网暴露；本机访问同样要密码。LAN 场景由用户自己决定是否开 `0.0.0.0`。

---

## 11. 风险清单

| # | 风险 | 概率 | 影响 | 缓解 | 判定点 |
|---|---|---|---|---|---|
| 1 | 三个构建密钥缺失 | 取决于你 | 阻断：登录与推送签名都不成立 | 从官方客户端提取；构建时注入，不入库 | 开工前 |
| 2 | Linux `.so` 指纹与 Windows DLL 不一致 | 20–30% | 登录被风控、任务接口异常 | 固定 tls-client 版本；真实账号验证；必要时抓 ClientHello 对比 JA3/JA4 | P0 |
| 3 | 第三方接口/版本漂移 | 12 个月内 30–50% | 任务列表或推送失效 | 版本常量集中；失败告警；保留快速改版能力 | 长期 |
| 4 | 账号风控 | 中 | 限流、封禁 | 家用 IP、串行、保持 30–40s 节奏、不引入代理池 | 运行期 |
| 5 | Web 暴露导致 token 泄漏 | 低概率、高影响 | 账号被操控 | 默认本机、密码、会话、CSRF、反代 TLS | 上线前 |
| 6 | 明文凭据落盘 | 中 | 数据泄露 | `0600`、加密盘、备份加密 | 部署时 |
| 7 | 许可约束 | 低 | 只能非商业使用 | 保留 LICENSE/NOTICE；自用改造；不对外提供代挂服务 | 分发时 |
| 8 | 长期运行内存/日志增长 | 中 | 小主机 OOM | 日志已按天滚动 7 天；加 RSS 监控与 24h soak | 验收 |

关于第 2 项，上游 `bogdanfinn/tls-client` 的 release 提供 Linux amd64/arm64（含 alpine/musl）预编译 `.so`，`build.sh` 支持 `go build -buildmode=c-shared`；CFFI 的 `RequestInput` 字段（`tlsClientIdentifier`、`headerOrder`、`withoutCookieJar`、`catchPanics` 等）与项目现有载荷完全对得上。真正要验证的是**版本行为**，不是接口形状。

另一个纯工程问题是 libc：Go 的 c-shared `.so` 带 glibc 符号版本要求。部署前在目标镜像里跑 `ldd libtls-client.so`，确认 `GLIBC_2.xx` 不超过运行时版本；Alpine 必须用 musl 变体，glibc 版 `.so` 在 musl 镜像里加载会失败。arm64 若缺少对应预编译产物，就在构建阶段用 Go 工具链自行编一个。

关于第 3 项，项目已经把“任务配置”做成每日从服务端现取（`mobileGLTaskList`），不是硬编码；这是好消息。硬编码的只有 `APP_VERSION=1.26.8.2`、`APP_ID=8`、`MASTER_TASK_ID=52` 这类客户端常量，改版时集中替换即可。

---

## 12. 工作量与里程碑

| 阶段 | 内容 | 产出 | 人日 |
|---|---|---|---|
| P0 | Linux 编译核心（平台层先 stub）；加载 `.so`；跑签名测试向量；真实账号登录一次；调一个只读接口 | go / no-go 结论 | 1–2 |
| P1 | `tls/device/diag/instance` 四个平台模块；删 `schedule.rs`；修 18 个平台相关测试 | `cargo test` 在 Linux 全绿 | 2–3 |
| P2 | axum 服务：15 个端点、鉴权、静态资源、调度器、单飞锁、优雅退出 | 本机浏览器可用 | 2–3 |
| P3 | 前端：`api.ts`、标题栏、登录页、响应式、导入导出 | 功能对齐桌面版 | 1–2 |
| P4 | Dockerfile、compose、systemd、迁移脚本、部署文档 | 一条命令起服务 | 1–1.5 |
| P5 | 真实账号 e2e、24h soak、鉴权与越权检查、故障注入 | 验收报告 | 1.5–2 |
| **合计** | | | **9–13.5** |

MVP 口径（只本机、单账号、API + 一个极简页面、不做导入导出 UI 与 Docker）：6–8 人日。

风险缓冲建议 +25%，即对外报 **12–16 人日**。

### 12.1 部署形态（示意）

```yaml
services:
  glacc:
    image: glacc-auto:linux
    restart: unless-stopped
    ports: ["127.0.0.1:8080:8080"]
    volumes: ["./data:/data"]
    environment:
      TZ: Asia/Shanghai
      GLACC_AUTO_DATA: /data
      GLACC_LISTEN: 0.0.0.0:8080   # 容器内必须监听 0.0.0.0，暴露面由上面的宿主映射限制
    # 首次启动生成随机密码并打印到 docker logs；
    # 或用 GLACC_WEB_PASSWORD_FILE 挂载一个 0600 的密码文件。
```

- 两段构建：Node 只用于 `npm run build`；运行层是 Debian slim + 二进制 + `.so` + `ca-certificates`，不含 Node。
- amd64/arm64 分别构建；glibc 镜像配 glibc 版 `.so`，Alpine 配 musl 版。
- systemd 版：`ExecStart=/opt/glacc/glacc-server --data-dir /var/lib/glacc --listen 127.0.0.1:8080`，`User=glacc`，`Restart=on-failure`。

---

## 13. 备选方案对比

| 方案 | 开发成本 | 运行成本 | 问题 |
|---|---|---|---|
| **本方案：Rust core + axum + 保留 Svelte** | 9–13 人日 | 一个二进制，约 30–60MB 内存 | 需要一次性移植 |
| Windows 虚拟机 | 0 | 2–4GB 内存，需要一台常开的 Windows 宿主 | 仍然是 Windows GUI；升级、重启、远程桌面都麻烦 |
| Wine 跑 Windows 版 | 1–3 人日 | 中 | WebView2、计划任务、DLL 加载都不稳；排障成本高于重写 |
| Xvfb + Tauri + noVNC | 3–5 人日 | 高 | 服务器要装 WebKitGTK 与 X；本质仍是 GUI，违背目标 |
| 用 Go/Python 重写 | 15–25 人日 | 低 | 5k 行协议与 4.8k 行测试全部作废；细节重做风险高 |

结论：只要目标是“服务化 + 网页访问”，本方案是成本和风险的最优解。

---

## 14. 验收标准

- [ ] 一条命令（或一个容器）启动，浏览器打开即用；服务器不安装任何 GUI/X11/WebKit 包。
- [ ] 短信登录、手动领取、定时领取、通知、历史、日志、导入导出全部可用。
- [ ] 从 Windows 迁移数据后，账号无需重新短信登录（token 未过期时）。
- [ ] 重复点击、双标签页、双实例、断网、进程中途被杀，都不产生并发领取或损坏 JSON。
- [ ] 默认只监听 `127.0.0.1`；非回环监听时必须有密码（首启生成或显式提供）。
- [ ] 未登录访问任何 `/api/*` 返回 401；导出凭据需要二次密码。
- [ ] 24h 连续运行 RSS 稳定（建议 < 120MB，以实测基线为准），日志按天滚动、7 天清理。
- [ ] `cargo test` 在 Linux 通过；Windows 专属测试已替换为平台无关测试。
- [ ] 镜像/包内保留 LICENSE 与 NOTICE。

---

## 15. 开工前需要准备的东西

1. **三个构建密钥**：`GLACC_CLIENT_ID`、`GLACC_CLIENT_SECRET`、`GLACC_SIGN_KEY`。仓库里是空值；前两个在官方客户端的 JS 字符串里，`SIGN_KEY` 位于推送签名逻辑中，需要自行提取。缺了登录与推送签名都不成立。
2. **一个可登录的测试账号**（能收短信），用于 P0 与 P5 的真实验证。
3. **目标机器架构**：amd64 还是 arm64；glibc 还是 musl（决定选哪个 `.so` 变体）。
4. **旧数据备份**：`%APPDATA%\glacc-auto` 整目录，尤其是每个账号 JSON 里的 `InstallSalt`。

## 16. 默认决策（按此设计，不需要现在回答）

| 项 | 默认 |
|---|---|
| 用户模型 | 单用户、单实例；账号上限继续 3 |
| 监听 | `127.0.0.1:8080`；远程访问走反代 |
| 时区 | `Asia/Shanghai`，显式配置，不跟随容器 |
| 数据目录 | `GLACC_AUTO_DATA`，容器内 `/data` |
| UI | 保留现有视觉与 1s 轮询；首版不做 SSE |
| 通知 | 保留 Server 酱；Webhook 作为后续扩展 |
| 调度 | 进程内；错过补跑一次；与手动领取共用单飞锁 |
| 鉴权 | 单密码 + 签名 cookie；首次启动生成随机密码 |
| 部署 | Docker 为主，systemd 为辅 |

---

## 17. 最终建议

**建议做，但先做 P0。**

理由：

- 协议层与平台层的边界非常干净，5 处 FFI 都是小接口，不存在“牵一发动全身”的异步改造。
- 设备身份是合成的，迁移不会改变服务端视角，这是很多同类项目没有的优势。
- 服务化后 7×24 常驻，token 到期前自动刷新，持续运行时不会撞上 7 天保活上限，桌面版“超过 7 天不开就要重登”的麻烦自然消失。
- 真正的未知数只有两个：密钥和 `.so` 指纹。两者都能在 1–2 天内用真实账号验证。

P0 的通过标准：Linux 上编译出核心库；`libtls-client.so` 加载成功；`push_sign(78,178,1786116529)` 仍等于 `7539e3580ea553cd906e129145fdd72e`；真实账号完成一次登录并拉到任务列表。四条全过，再投入 P1–P5；任何一条不过，先停下复盘，避免把 10 人日投进一个不确定的前提。

---

### 附：关键代码位置

| 内容 | 位置 |
|---|---|
| 设备身份派生 | `src-tauri/src/glacc/credentials.rs:120-146` |
| 设备档案选择 | `src-tauri/src/glacc/device.rs`（`PROFILES`、`select_for_phone`） |
| TLS 动态库加载 | `src-tauri/src/glacc/tls.rs:94-179` |
| 任务推送签名 | `src-tauri/src/glacc/sign.rs` |
| 单账号领取 | `src-tauri/src/glacc/claim.rs` |
| 多账号串行 | `src-tauri/src/glacc/sweep.rs`（`ACCOUNT_GAP`） |
| 数据根路径 | `src-tauri/src/glacc/store.rs:34-42` |
| 调度（待替换） | `src-tauri/src/glacc/schedule.rs` |
| Tauri 命令与线程 | `src-tauri/src/lib.rs:1380-1439` |
| 前端调用封装 | `app/src/api.ts:18-30` |
