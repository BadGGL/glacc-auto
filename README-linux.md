# glacc-auto Linux 服务版

原项目是 Windows Tauri 桌面应用；这里是 Linux 无 GUI 重构版：Rust 常驻服务 + axum HTTP API + 原 Svelte 网页，支持多条 cron 定时任务。交付形态为 Docker 镜像。

> 仅限个人学习、研究与非商业使用，遵循原仓库 PolyForm Noncommercial 1.0.0。使用风险自负。

> 云服务器部署的完整步骤见 [DEPLOY.md](DEPLOY.md)。

## 1. 前置

- Docker Engine 24+ 与 Docker Compose v2。
- Linux 版 TLS 指纹库已内置在 `lib/libtls-client.so`（`bogdanfinn/tls-client` v1.18.1，linux/amd64）；换架构时替换该文件。
- 三个编译期常量：`GLACC_CLIENT_ID`、`GLACC_CLIENT_SECRET`、`GLACC_SIGN_KEY`。本工作区已从官方二进制提取到 `keys/glacc-build.env`（不要提交）。

## 2. 构建

```bash
# 注入三个编译期常量（只影响 cargo build，不是账号密码）
set -a; . keys/glacc-build.env; set +a

cp .env.example .env
# 编辑 .env，至少设置 GLACC_PASSWORD

docker compose build
```

Dockerfile 是多阶段构建：Node 构建 `web/dist`，Rust release 编译服务，运行时只带二进制、静态资源和 `lib/`。

## 3. 启动

```bash
docker compose up -d
docker compose logs -f glacc
```

打开 `http://<服务器IP>:8080`，输入 `GLACC_PASSWORD`（未设置时首次启动会在容器日志打印随机密码）。

默认端口映射 `0.0.0.0:8080`；只想本机访问就把 `.env` 的 `GLACC_BIND` 改成 `127.0.0.1`。

## 4. 定时任务

网页左侧「定时」页可新建多条规则，字段：

| 字段 | 说明 |
| --- | --- |
| `cron` | 5 段：分 时 日 月 周；也接受 6 段（秒 分 时 日 月 周）。例：`0 8 * * *` 每天 08:00 |
| `timezone` | IANA 时区，例 `Asia/Shanghai` |
| `scope` | `all` 或某个账号 key |
| `catchUp` | 服务停机期间错过最近一次，启动后补跑 |
| `jitterSecs` | 触发后随机延后 0–3600 秒，避免整点并发 |

规则保存在数据卷的 `schedules.json`；`run_now` 可立即执行一次。执行仍受全局单飞锁约束：上一轮未结束时，本轮记「跳过」。

## 5. 数据与备份

- 数据卷 `glacc-data` 挂到容器 `/data`，含账号、凭据、设置、历史、日志、`auth.json`、`schedules.json`。
- 首页支持导出/导入凭据 JSON；导出接口需要已登录会话。
- 迁移旧 Windows 数据时，务必保留每个账号 JSON 里的 `InstallSalt`，否则设备 ID / peer ID 会变化，可能触发重新登录。

## 6. 安全

- 默认强制密码登录：`GLACC_PASSWORD` 为空时随机生成并打印一次；`auth.json` 只存 Argon2 派生值。
- 会话 Cookie：HttpOnly + SameSite=Strict；写请求要求 `X-GLACC-CSRF: 1` 且 Origin/Referer 匹配 Host。
- 公网暴露必须放在 HTTPS 反代之后，并设 `GLACC_COOKIE_SECURE=true`；反代需保留原始 `Host`（如 nginx `proxy_set_header Host $host;`），否则 Origin 校验会拒绝写请求。
- 反代后把 `GLACC_TRUST_PROXY=true`，让登录限流按真实客户端 IP 计数；同时让反代覆盖写入 `X-Real-IP`/`X-Forwarded-For`，不要直接透传客户端自带的转发头。
- 登录限流 5 次 / 5 分钟 / IP。
- 建议只允许可信 IP 访问 8080，或使用 VPN。

## 7. 本地开发（不用 Docker）

```bash
# 后端
GLACC_DATA_DIR=/tmp/glacc-dev GLACC_PASSWORD=dev-pass \
GLACC_LISTEN=127.0.0.1:8080 GLACC_WEB_DIR=web/dist \
cargo run

# 前端热更新（另开终端）
cd web && npm install && npm run dev
# Vite 已把 /api 与 /healthz 代理到 127.0.0.1:8080
```

生产前端构建：`cd web && npm run build`，产物在 `web/dist`。

## 8. 已知边界

- 没有 `libtls-client.so` 时服务能启动，但登录/领取等请求会失败；`GLACC_REQUIRE_TLS_LIB=true` 可让启动直接失败。
- 服务端接口与任务配置随官方客户端版本变化；`APP_VERSION` 等常量在 `src/glacc/constants.rs`。
- 原 Windows 计划任务（schtasks）逻辑已删除，全部由进程内 cron 调度器替代。
- 网页不提供修改密码功能；改密码请改 `GLACC_PASSWORD` 后重启（会重写 `auth.json` 的密码哈希）。
