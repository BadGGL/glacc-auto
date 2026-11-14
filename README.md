# glacc-auto Linux 服务版

> **继承声明**
>
> 本项目是 [miaocell/glacc-auto](https://github.com/miaocell/glacc-auto) 的 Linux 重构衍生版本，继承自 **https://github.com/miaocell/glacc-auto**。
>
> 原项目作者：**JiangXu26710**（原仓库：https://github.com/JiangXu26710/glacc-auto）。原项目是 Windows Tauri 桌面应用；本仓库把它的协议核心重构成 Linux 下可常驻运行的 Rust 服务，去掉 GUI，改为网页访问，并新增多条 cron 定时任务。
>
> 原项目及本项目均采用 **PolyForm Noncommercial License 1.0.0（非商业许可 1.0.0）**，仅限个人学习、研究与非商业用途。修改、二次开发与再分发必须保留原许可文本和 `NOTICE` 中的 Required Notice 版权声明。完整许可见 [LICENSE](LICENSE) 与 [NOTICE](NOTICE)。

> **本项目用途：梨加速器自动领取每日免费时长。**
>
> 这是一个给 **梨加速器（GeiLi）** 使用的自动领取工具：登录梨加速器账号后，自动拉取每日任务并逐个完成，从而领取免费时长。它把原 Windows 桌面版重构成 Linux 后台服务，带网页管理界面和定时任务，适合放在 NAS、小主机或云服务器上 7×24 运行。
>
> 本项目不是梨加速器官方客户端，仅用于个人学习、研究与合法的非商业场景；请遵守梨加速器的服务条款和当地法律法规。

## 项目简介

`glacc-auto` Linux 服务版是一个无 GUI 的后台服务：

- **Rust + axum** 常驻 HTTP 服务
- **网页访问**，复用原项目的 Svelte 前端
- **多账号**串行领取，保留原项目的协议、设备指纹与签名逻辑
- **多条 cron 定时任务**，支持时区、账号范围、补跑、随机抖动
- **Docker 交付**，镜像内自带 Linux TLS 指纹库

> 仅限个人学习、研究与非商业使用。使用风险自负。

## 简单使用说明

1. **部署服务**：用下面的 Docker 教程启动容器，拿到访问地址和登录密码。
2. **登录网页**：浏览器打开 `http://<服务器IP>:8080`，输入 `GLACC_PASSWORD`。
3. **添加账号**：进入「账号」，输入梨加速器绑定的手机号，获取短信验证码并登录；最多可添加 3 个账号。
4. **自动领取**：点「开始领取」，服务会拉取当天任务并逐个推送；也可以只保留定时任务，让它每天自动跑。
5. **配置定时**：进入「定时」页，新建 cron 规则，例如 `0 8 * * *` 表示每天 08:00 自动领取。
6. **查看结果**：首页查看可用时长、运行状态和 7 天记录；需要通知可在设置里填 Server酱 SendKey。

## 与上游的主要差异

| 项目 | 上游 Windows 版 | 本仓库 Linux 版 |
| --- | --- | --- |
| 运行形态 | Tauri 桌面应用（WebView + GUI） | Rust 常驻服务 + 浏览器网页 |
| 系统绑定 | Win32 API、`schtasks`、`kernel32`/`bcrypt` | 跨平台 Rust；进程内 cron 调度 |
| 定时任务 | Windows 计划任务，单条时间 | 多条 cron 规则，可按账号范围执行 |
| 网络层 | 加载 `tls-client.dll` | `libloading` 加载 `libtls-client.so` |
| 数据目录 | `%APPDATA%\glacc-auto` | `GLACC_DATA_DIR` / XDG 数据目录 |
| 交付 | NSIS 安装包 | Docker 镜像 / Docker Compose |
| 网页鉴权 | 无 | 密码登录、HttpOnly Cookie、CSRF 校验、登录限流 |

协议核心（登录、验证码、任务列表、任务推送、签名、设备档案、账号存储、通知等）来自上游，尽量保持行为一致；平台相关代码（TLS 加载、随机数、日志时间、单实例锁、计划任务）已替换为 Linux 实现。

## 功能

- 手机号 + 短信验证码登录
- 多账号管理（最多 3 个）
- 自动拉取每日任务并逐个推送
- 领取进度实时显示
- 领取历史与 7 天记录
- Server酱通知
- 多条 cron 定时任务
- 网页导出/导入凭据
- 密码登录与访问保护

## 使用本仓库 Docker 镜像部署教程

### 1. 准备

- Linux x86_64（ARM64 需替换 `libtls-client.so`）
- Docker Engine 24+ 与 Docker Compose v2
- 本仓库 GitHub Actions 已构建好的镜像：`ghcr.io/badggl/glacc-auto:0.3.0`
- 该镜像已内置三个编译期常量（`GLACC_CLIENT_ID`、`GLACC_CLIENT_SECRET`、`GLACC_SIGN_KEY`），**不需要在服务器上再配置**；它们只影响构建，不是你的梨加速器账号密码。
- 该 GHCR 镜像包目前是 private。需要仓库 owner 授权你的 GitHub 账号读取，或使用有权限的账号登录。没有权限时看文末「方式二：Fork 后构建自己的镜像」。

### 2. 登录 GHCR 并拉取镜像

```bash
# 如果你的账号有该私有包读取权限
echo "$GHCR_TOKEN" | docker login ghcr.io -u <你的GitHub用户名> --password-stdin

docker pull ghcr.io/badggl/glacc-auto:0.3.0
docker logout ghcr.io   # 拉取完成后可以退出登录
```

### 3. 启动

```bash
mkdir -p /opt/glacc-auto && cd /opt/glacc-auto

# 下载本仓库的生产 compose 文件
curl -fsSL https://raw.githubusercontent.com/BadGGL/glacc-auto/main/compose.prod.yaml -o compose.prod.yaml

# 创建 .env
cat > .env <<'ENV'
GLACC_PASSWORD=换成一个强密码
GLACC_BIND=0.0.0.0
GLACC_PORT=8080
GLACC_COOKIE_SECURE=false
GLACC_TRUST_PROXY=false
ENV
chmod 600 .env

docker compose -f compose.prod.yaml up -d
```

打开 `http://<服务器IP>:8080`，输入 `GLACC_PASSWORD` 登录。

首次启动如果没有设置 `GLACC_PASSWORD`，服务会生成随机密码并打印到容器日志：

```bash
docker compose -f compose.prod.yaml logs glacc | grep -i password
```

### 4. 常驻与重启

```bash
docker compose -f compose.prod.yaml ps
docker compose -f compose.prod.yaml logs -f --tail=100
docker compose -f compose.prod.yaml restart
```

### 方式二：Fork 后构建自己的镜像

如果你没有本仓库 GHCR 私有包的读取权限：

1. Fork 本仓库到你的 GitHub 账号。
2. 在仓库 `Settings → Secrets and variables → Actions` 里添加三个 Secrets：`GLACC_CLIENT_ID`、`GLACC_CLIENT_SECRET`、`GLACC_SIGN_KEY`。
3. 打开 `Actions → build-image → Run workflow`，等待构建完成，镜像会推送到你自己的 GHCR。
4. 把 `compose.prod.yaml` 里的 `image:` 改成你的镜像地址，再按上面的步骤启动。

> 三个 key 只用于构建；不要把 `keys/`、`.env` 或真实账号密码提交到仓库。

## 配置项

| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `GLACC_PASSWORD` | 空 | 网页登录密码；为空时首次启动随机生成并打印 |
| `GLACC_LISTEN` | `0.0.0.0:8080` | 监听地址 |
| `GLACC_BIND` / `GLACC_PORT` | `0.0.0.0` / `8080` | Compose 的宿主绑定地址与端口 |
| `GLACC_DATA_DIR` | `/data`（容器内） | 数据目录，含账号、设置、日志、历史、`auth.json`、`schedules.json` |
| `GLACC_WEB_DIR` | `web/dist` | 前端静态文件目录 |
| `GLACC_COOKIE_SECURE` | `false` | HTTPS 反代后设为 `true` |
| `GLACC_TRUST_PROXY` | `false` | 反代后设为 `true`，按真实客户端 IP 限流 |
| `GLACC_REQUIRE_TLS_LIB` | `false` | 设为 `true` 时，缺少 TLS 库直接启动失败 |
| `GLACC_TLS_LIB` | 空 | 手动指定 `libtls-client.so` 路径 |

## 定时任务

网页左侧「定时」页可以新建多条规则：

| 字段 | 说明 |
| --- | --- |
| `cron` | 5 段（分 时 日 月 周）或 6 段（秒 分 时 日 月 周）。例：`0 8 * * *` 每天 08:00 |
| `timezone` | IANA 时区，例 `Asia/Shanghai` |
| `scope` | `all` 或某个账号 key |
| `catchUp` | 服务停机期间错过最近一次，启动后补跑 |
| `jitterSecs` | 触发后随机延后 0–3600 秒，避免整点并发 |

规则保存在数据卷的 `schedules.json`。执行仍受全局单飞锁约束：上一轮未结束时，本轮记「跳过」。

## 数据与备份

- 数据卷 `glacc-data` 挂载到容器 `/data`
- 包含：账号 JSON、凭据、设置、领取历史、日志、`auth.json`、`schedules.json`
- 备份示例：

```bash
docker run --rm \
  -v glacc-auto_glacc-data:/data \
  -v /root:/backup \
  alpine tar czf /backup/glacc-data-$(date +%F).tar.gz -C /data .
```

- 从 Windows 版迁移时，务必保留每个账号 JSON 里的 `InstallSalt`，否则设备 ID / peer ID 会变化，可能触发重新登录。

## 安全说明

- 强制密码登录；`auth.json` 只保存 Argon2 派生值。
- 会话 Cookie：HttpOnly + SameSite=Strict。
- 写请求要求 `X-GLACC-CSRF: 1`，并校验 Origin/Referer 与 Host 匹配。
- 登录限流 5 次 / 5 分钟 / IP。
- 公网使用必须放在 HTTPS 反代之后，并设置 `GLACC_COOKIE_SECURE=true`；反代需保留原始 `Host`。
- 反代后设置 `GLACC_TRUST_PROXY=true`，并让反代覆盖写入 `X-Real-IP` / `X-Forwarded-For`，不要直接透传客户端自带的转发头。
- 建议安全组只开放 22/80/443；8080 仅在内网或本机监听。

## 本地开发（不用 Docker）

```bash
# 后端
GLACC_DATA_DIR=/tmp/glacc-dev GLACC_PASSWORD=change-me-for-local-dev \
GLACC_LISTEN=127.0.0.1:8080 GLACC_WEB_DIR=web/dist \
cargo run

# 前端热更新（另开终端）
cd web && npm install && npm run dev
# Vite 已把 /api 与 /healthz 代理到 127.0.0.1:8080
```

生产前端构建：

```bash
cd web && npm run build
```

## 目录结构

```text
.
├── src/
│   ├── main.rs          # 服务入口
│   ├── api.rs           # axum 路由
│   ├── config.rs        # 环境配置
│   ├── scheduler.rs     # cron 调度器
│   ├── service.rs       # 业务编排
│   ├── web_auth.rs      # 网页登录鉴权
│   └── glacc/           # 从上游移植的协议核心
├── web/                 # Svelte 前端
├── lib/libtls-client.so # Linux TLS 指纹库
├── Dockerfile
├── compose.yaml
├── compose.prod.yaml
├── DEPLOY.md
├── LICENSE
└── NOTICE
```

## 已知边界

- 没有 `libtls-client.so` 时服务能启动，但登录/领取等请求会失败；可用 `GLACC_REQUIRE_TLS_LIB=true` 让启动直接失败。
- 服务端接口与任务配置随官方客户端版本变化；相关常量在 `src/glacc/constants.rs`。
- 原 Windows 计划任务（`schtasks`）逻辑已删除，全部由进程内 cron 调度器替代。
- 网页不提供修改密码功能；改密码请改 `GLACC_PASSWORD` 后重启（会重写 `auth.json` 的密码哈希）。
- 仅供个人学习、研究与非商业使用，请遵守原项目许可与当地法律法规。

## 许可与致谢

- 本项目继承自 **[miaocell/glacc-auto](https://github.com/miaocell/glacc-auto)**，原项目作者 **JiangXu26710**。
- 原项目及本项目均采用 **PolyForm Noncommercial License 1.0.0**。完整文本见 [LICENSE](LICENSE)，Required Notice 见 [NOTICE](NOTICE)。
- 感谢上游作者对协议、设备指纹、签名与业务逻辑的实现。
- Linux TLS 指纹库来自 [bogdanfinn/tls-client](https://github.com/bogdanfinn/tls-client)，BSD-3-Clause，许可见 `lib/LICENSE-tls-client`。

## 免责声明

本项目仅用于个人学习、技术研究与合法的非商业场景。使用者应自行承担使用风险，并遵守相关服务条款与法律法规。项目作者不对因使用本项目产生的任何直接或间接损失负责。
