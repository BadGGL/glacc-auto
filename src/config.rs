// 运行配置：只从环境变量读取，便于 Docker / systemd 注入。

use std::net::SocketAddr;

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub secure_cookie: bool,
    pub web_dir: String,
    pub require_tls_lib: bool,
    pub trust_proxy: bool,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let listen_raw =
            std::env::var("GLACC_LISTEN").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
        let listen = listen_raw
            .parse::<SocketAddr>()
            .map_err(|e| format!("GLACC_LISTEN 无效（{listen_raw}）：{e}"))?;
        let secure_cookie = env_bool("GLACC_COOKIE_SECURE", false);
        let web_dir =
            std::env::var("GLACC_WEB_DIR").unwrap_or_else(|_| "web/dist".to_string());
        let require_tls_lib = env_bool("GLACC_REQUIRE_TLS_LIB", false);
        let trust_proxy = env_bool("GLACC_TRUST_PROXY", false);
        Ok(Self {
            listen,
            secure_cookie,
            web_dir,
            require_tls_lib,
            trust_proxy,
        })
    }
}

fn env_bool(key: &str, default: bool) -> bool {
    match std::env::var(key) {
        Ok(v) => matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => default,
    }
}

/// 读取工作目录下的 `.env`（不覆盖已存在的环境变量）。
/// Docker 部署直接用 env，本地开发才需要这个文件。
pub fn load_dotenv() {
    let Ok(text) = std::fs::read_to_string(".env") else {
        return;
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || std::env::var_os(key).is_some() {
            continue;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'');
        std::env::set_var(key, value);
    }
}
