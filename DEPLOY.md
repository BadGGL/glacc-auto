# 云服务器 Docker 部署

以下以 Ubuntu 22.04/24.04、x86_64 云服务器为例，域名用 `glacc.example.com`，服务器 IP 用 `<SERVER_IP>` 代替。项目目录用 `/opt/glacc-auto`。

## 0. 先确认机器

```bash
uname -m                 # 应为 x86_64；arm64 见第 9 节
docker --version         # 24+
docker compose version   # v2
free -h                  # 构建建议 2GB 内存；1GB 机器加 swap
df -h /opt               # 预留 3GB
```

安全组/防火墙只需放行 `22`、`80`、`443`。`8080` 不要对公网开放。

## 1. 上传项目

在 Mac 上执行：

```bash
cd /Volumes/Info/DeepSeek/GeiLi
rsync -av --progress \
  --exclude 'target' \
  --exclude 'web/node_modules' \
  --exclude 'web/dist' \
  --exclude 'docs/screenshots' \
  ./ root@<SERVER_IP>:/opt/glacc-auto/
```

必须上传的内容包括：

- `Cargo.toml`、`Cargo.lock`、`src/`
- `web/`（含 `package.json`、`package-lock.json`、`vite.config.ts`、源码）
- `Dockerfile`、`compose.yaml`、`.dockerignore`
- `lib/libtls-client.so`（18MB）与 `lib/LICENSE-tls-client`
- `keys/glacc-build.env`
- `LICENSE`、`NOTICE`、`README-linux.md`、`.env.example`

到服务器上收紧密钥权限：

```bash
cd /opt/glacc-auto
chmod 700 keys
chmod 600 keys/glacc-build.env
```

如果服务器没有 `rsync`，也可以在 Mac 上打包：

```bash
tar --exclude=target --exclude=web/node_modules --exclude=web/dist \
    -czf glacc-auto.tgz .
scp glacc-auto.tgz root@<SERVER_IP>:/opt/
ssh root@<SERVER_IP> 'mkdir -p /opt/glacc-auto && tar -xzf /opt/glacc-auto.tgz -C /opt/glacc-auto'
```

## 2. 安装 Docker（已装可跳过）

```bash
curl -fsSL https://get.docker.com | sh
sudo systemctl enable --now docker
sudo usermod -aG docker "$USER"
# 重新登录后生效；或当前会话用 sudo docker ...
```

## 3. 配置环境变量

```bash
cd /opt/glacc-auto
set -a; . keys/glacc-build.env; set +a   # 三个编译期常量
cp .env.example .env
# 生成一个强密码（hex 不含 shell/Compose 的特殊字符）
openssl rand -hex 24
nano .env
```

`.env` 至少改成：

```dotenv
GLACC_PASSWORD=换成一个长随机密码
GLACC_BIND=127.0.0.1
GLACC_PORT=8080
GLACC_COOKIE_SECURE=true
GLACC_TRUST_PROXY=true
CARGO_BUILD_JOBS=1
```

说明：

- `GLACC_BIND=127.0.0.1` 表示只监听回环，由 Nginx/Caddy 反代进来；想直接暴露 8080 才改 `0.0.0.0`。
- `GLACC_COOKIE_SECURE=true` 要求 HTTPS；如果暂时用 HTTP 直连，先设 `false`。
- `GLACC_TRUST_PROXY=true` 只在可信反代之后开；否则登录限流会把所有人算成同一个 IP。

## 4. 构建镜像

```bash
cd /opt/glacc-auto
set -a; . keys/glacc-build.env; set +a
docker compose build
```

1.8GB 内存的机器把 `CARGO_BUILD_JOBS` 设为 1；构建前最好再加 4GB 临时 swap（见第 10 节 OOM）。

构建阶段会：

1. `node:22-alpine` 执行 `npm ci && npm run build`，生成 `web/dist`
2. `rust:1.90-bookworm` 把三个 key 作为编译期常量注入，`cargo build --release`
3. `debian:bookworm-slim` 只复制二进制、静态资源、`libtls-client.so`

三个 key 会留在镜像构建历史里，构建机不要多人共用；更高要求可用 BuildKit secret，但当前 compose 用 build args 最简单。

## 5. 启动

```bash
docker compose up -d
docker compose ps
docker compose logs -f --tail=100 glacc
curl -fsS http://127.0.0.1:8080/healthz && echo
```

看到 `ok` 即服务正常。首次启动若没有设置 `GLACC_PASSWORD`，会在日志里打印一次随机密码。

## 6. 反向代理与 HTTPS

### Caddy（最简单）

`/etc/caddy/Caddyfile`：

```caddyfile
glacc.example.com {
    encode gzip
    reverse_proxy 127.0.0.1:8080
}
```

```bash
sudo systemctl reload caddy
```

Caddy 自动申请证书，并会设置 `X-Forwarded-For`。

### Nginx + Certbot

先只写 80 端口的反代，证书交给 certbot 自动补 443：

```nginx
server {
    listen 80;
    server_name glacc.example.com;

    client_max_body_size 16m;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 300s;
    }
}
```

证书：

```bash
sudo apt install -y certbot python3-certbot-nginx
sudo certbot --nginx -d glacc.example.com
sudo nginx -t && sudo systemctl reload nginx
```

要点：`Host` 必须保留为外部域名，否则 Origin/CSRF 校验会拒绝写请求；`X-Real-IP` 由 Nginx 覆盖写入，配合 `GLACC_TRUST_PROXY=true` 才能按真实 IP 限流。

## 7. 首次登录

浏览器打开 `https://glacc.example.com`，输入 `.env` 里的 `GLACC_PASSWORD`。

登录后：

1. 左侧「设置」确认领取间隔、网络重试。
2. 左侧「定时」新建规则，例如每天 08:00：
   - cron：`0 8 * * *`
   - 时区：`Asia/Shanghai`
   - 范围：全部账号
   - 按需开启「停机错过后补跑」
3. 回到首页，点「登录账号」，用手机号 + 短信验证码登录。
4. 点「开始领取」验证整条链路。

## 8. 数据、备份与升级

数据都在 Docker 命名卷 `glacc-data` 的 `/data`：

```bash
# 备份
docker run --rm -v glacc-data:/data -v "$PWD":/backup alpine \
  tar czf /backup/glacc-data-$(date +%F).tgz -C /data .

# 查看
docker run --rm -v glacc-data:/data alpine ls -la /data
```

升级：

```bash
cd /opt/glacc-auto
# 重新 rsync 或 git pull 新代码
set -a; . keys/glacc-build.env; set +a
docker compose build
docker compose up -d
```

只重启：

```bash
docker compose restart glacc
```

## 9. arm64 服务器

当前 `lib/libtls-client.so` 是 `linux/amd64`。如果 `uname -m` 是 `aarch64`：

```bash
curl -L -o lib/libtls-client.so \
  https://github.com/bogdanfinn/tls-client/releases/download/v1.18.1/tls-client-linux-arm64-1.18.1.so
file lib/libtls-client.so   # 应显示 ARM aarch64
docker compose build
```

Dockerfile 按构建机架构原生编译，不需要额外 `--platform`。

## 10. 常见问题

**`/healthz` 不通**

```bash
docker compose logs --tail=200 glacc
docker compose ps
ss -lntp | grep 8080
```

**登录/领取提示 TLS 指纹库不可用**

```bash
docker compose exec glacc ls -l /app/lib
docker compose exec glacc sh -c 'echo $GLACC_TLS_LIB'
```

确认文件是当前架构的 ELF，并且容器内路径是 `/app/lib/libtls-client.so`。可用 `GLACC_REQUIRE_TLS_LIB=true` 让启动直接失败，避免带病运行。

**网页能开，但保存设置/登录返回 403**

反代没保留 `Host`，或浏览器 Origin 与访问域名不一致。检查 Nginx `proxy_set_header Host $host;`，并用同一个域名访问，不要混用 IP 和域名。

**登录一直 429**

`GLACC_TRUST_PROXY=false` 时，反代后的所有请求都来自 `127.0.0.1`，会共用 5 次/5 分钟额度。设 `GLACC_TRUST_PROXY=true`，并让反代覆盖 `X-Real-IP`。

**构建时 OOM**

Node + Vite + Rust 同时跑，1GB 机器容易失败。加 2GB swap，或临时升配到 2GB 构建后再降配。

**误删数据**

不要执行 `docker compose down -v`，`-v` 会删除 `glacc-data` 卷。升级只用 `up -d`。

## 11. 安全清单

- 强随机 `GLACC_PASSWORD`，不要用 `change-me`。
- 安全组只开 `22/80/443`，`8080` 只监听 `127.0.0.1`。
- 全站 HTTPS，`GLACC_COOKIE_SECURE=true`。
- 反代后 `GLACC_TRUST_PROXY=true`，并确保反代覆盖转发头。
- 定期备份 `glacc-data`。
- 仅限个人学习研究与非商业使用，遵循 PolyForm Noncommercial 1.0.0。
