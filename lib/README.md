# libtls-client.so

本目录已内置 Linux x86_64 版 TLS 指纹库，Docker 构建时复制到镜像 `/app/lib/`。

- 来源：`bogdanfinn/tls-client` v1.18.1 的 `tls-client-linux-ubuntu-amd64-1.18.1.so`
- 本地文件名：`libtls-client.so`
- sha256：`719ad7f9772d4a72572e4eb9c1aace289a13eaa9bf3862c32f5dce6222d5644f`
- 许可：BSD-3-Clause，见 `LICENSE-tls-client`
- 服务默认读取 `GLACC_TLS_LIB=/app/lib/libtls-client.so`；找不到时服务仍可启动，但所有需要 TLS 指纹的请求会失败。
- 更换版本时直接替换本文件，并确保镜像架构与目标机一致（当前为 linux/amd64）。
