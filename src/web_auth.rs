// Web 访问鉴权：单用户密码 + HMAC 签名 Cookie + 登录限流 + CSRF 头校验。
//
// 设计目标：公网 IP 直接暴露时，未认证请求拿不到任何账号数据；密码只以 Argon2 派生值保存；
// Cookie 为 HttpOnly + SameSite=Strict；所有写操作要求自定义头，配合 SameSite 阻断 CSRF。

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::{Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::glacc::store;

type HmacSha256 = Hmac<Sha256>;

pub const COOKIE_NAME: &str = "glacc_session";
const SESSION_DAYS: i64 = 30;

#[derive(Debug, Serialize, Deserialize)]
struct AuthFile {
    version: u32,
    password_hash: String,
    secret_hex: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    exp: i64,
    iat: i64,
    nonce: String,
}

pub struct AuthStore {
    path: PathBuf,
    password_hash: String,
    secret: Vec<u8>,
    pub generated_password: Option<String>,
}

impl AuthStore {
    pub fn load_or_init() -> Result<Self, String> {
        let path = store::data_root().join("auth.json");
        let env_password = std::env::var("GLACC_PASSWORD").ok().filter(|s| !s.is_empty());
        let mut generated = None;

        let (mut password_hash, secret) = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<AuthFile>(&text) {
                Ok(f) if !f.password_hash.is_empty() && f.secret_hex.len() == 64 => {
                    let secret = hex_decode(&f.secret_hex)
                        .ok_or_else(|| "auth.json 的 secret 不是合法 hex".to_string())?;
                    (f.password_hash, secret)
                }
                _ => return Err(format!("auth.json 无法解析：{}", path.display())),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let secret = random_secret();
                let password = match &env_password {
                    Some(p) => p.clone(),
                    None => {
                        let p = random_password(18);
                        generated = Some(p.clone());
                        p
                    }
                };
                (hash_password(&password)?, secret)
            }
            Err(e) => return Err(format!("读取 auth.json 失败：{e}")),
        };

        // 环境变量优先：显式给了 GLACC_PASSWORD 就以它为准（便于部署时重置）。
        if let Some(p) = env_password {
            if self::verify_password(&password_hash, &p).is_err() {
                password_hash = hash_password(&p)?;
            }
        }

        let store = Self {
            path,
            password_hash,
            secret,
            generated_password: generated.clone(),
        };
        store.save()?;
        if let Some(p) = generated {
            // 只打印一次，不写日志文件，避免明文密码落盘。
            eprintln!("============================================================");
            eprintln!("  首次启动，已生成网页访问密码：{p}");
            eprintln!("  请立即登录后修改，或设置 GLACC_PASSWORD 环境变量固定密码。");
            eprintln!("============================================================");
        }
        Ok(store)
    }

    fn save(&self) -> Result<(), String> {
        let file = AuthFile {
            version: 1,
            password_hash: self.password_hash.clone(),
            secret_hex: hex_encode(&self.secret),
        };
        let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
        store::write_atomic(&self.path, &json).map_err(|e| format!("写入 auth.json 失败：{e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn verify_password(&self, password: &str) -> bool {
        self::verify_password(&self.password_hash, password).is_ok()
    }

    pub fn issue_token(&self) -> String {
        let now = chrono::Utc::now().timestamp();
        let claims = Claims {
            exp: now + SESSION_DAYS * 86400,
            iat: now,
            nonce: uuid::Uuid::new_v4().to_string(),
        };
        let payload = serde_json::to_vec(&claims).expect("claims 序列化");
        let body = URL_SAFE_NO_PAD.encode(&payload);
        let sig = self.sign(body.as_bytes());
        format!("{body}.{sig}")
    }

    pub fn verify_token(&self, token: &str) -> bool {
        let Some((body, sig)) = token.split_once('.') else {
            return false;
        };
        let Some(expected) = URL_SAFE_NO_PAD.decode(sig).ok() else {
            return false;
        };
        let mut mac = HmacSha256::new_from_slice(&self.secret).expect("HMAC key");
        mac.update(body.as_bytes());
        if mac.verify_slice(&expected).is_err() {
            return false;
        }
        let Ok(payload) = URL_SAFE_NO_PAD.decode(body) else {
            return false;
        };
        let Ok(claims) = serde_json::from_slice::<Claims>(&payload) else {
            return false;
        };
        claims.exp > chrono::Utc::now().timestamp()
    }

    fn sign(&self, data: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(&self.secret).expect("HMAC key");
        mac.update(data);
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    }
}

fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("密码哈希失败：{e}"))
}

fn verify_password(hash: &str, password: &str) -> Result<(), String> {
    let parsed = PasswordHash::new(hash).map_err(|e| e.to_string())?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|e| e.to_string())
}

fn random_secret() -> Vec<u8> {
    use rand::RngCore;
    let mut b = vec![0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    b
}

fn random_password(len: usize) -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

// ── 登录限流 ──

pub struct LoginLimiter {
    inner: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
}

impl LoginLimiter {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// 5 次 / 5 分钟。失败也计数，成功后清零。
    pub fn allow(&self, ip: IpAddr) -> bool {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let q = map.entry(ip).or_default();
        let cutoff = Instant::now() - Duration::from_secs(300);
        while q.front().is_some_and(|t| *t < cutoff) {
            q.pop_front();
        }
        q.len() < 5
    }

    pub fn record_failure(&self, ip: IpAddr) {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(ip).or_default().push_back(Instant::now());
    }

    pub fn clear(&self, ip: IpAddr) {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        map.remove(&ip);
    }
}

#[derive(Clone)]
pub struct WebAuth {
    pub store: Arc<AuthStore>,
    pub limiter: Arc<LoginLimiter>,
    pub secure_cookie: bool,
    /// 仅当服务位于可信反代之后时开启；开启后登录限流按 X-Real-IP / X-Forwarded-For 取客户端 IP。
    pub trust_proxy: bool,
}

impl WebAuth {
    pub fn new(secure_cookie: bool, trust_proxy: bool) -> Result<Self, String> {
        Ok(Self {
            store: Arc::new(AuthStore::load_or_init()?),
            limiter: Arc::new(LoginLimiter::new()),
            secure_cookie,
            trust_proxy,
        })
    }

    /// 解析登录限流用的客户端地址。
    /// 只有 trust_proxy=true 才看转发头；优先 X-Real-IP（反代覆盖写入），
    /// 其次取 X-Forwarded-For 最右一个（最近一跳追加的地址）。
    pub fn client_ip(&self, headers: &axum::http::HeaderMap, peer: IpAddr) -> IpAddr {
        if !self.trust_proxy {
            return peer;
        }
        let from_real = headers
            .get("x-real-ip")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<IpAddr>().ok());
        if let Some(ip) = from_real {
            return ip;
        }
        headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .and_then(|v| v.trim().parse::<IpAddr>().ok())
            .unwrap_or(peer)
    }

    pub fn cookie_header(&self, token: &str) -> String {
        let secure = if self.secure_cookie { "; Secure" } else { "" };
        format!(
            "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{secure}",
            SESSION_DAYS * 86400
        )
    }

    pub fn clear_cookie_header(&self) -> String {
        format!("{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
    }
}

pub fn token_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix(&format!("{COOKIE_NAME}=")) {
            return Some(v.to_string());
        }
    }
    None
}

fn origin_matches_host(req: &Request) -> bool {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if host.is_empty() {
        return false;
    }
    for name in [header::ORIGIN, header::REFERER] {
        if let Some(v) = req.headers().get(name).and_then(|h| h.to_str().ok()) {
            let without_scheme = v
                .split_once("://")
                .map(|(_, rest)| rest)
                .unwrap_or(v)
                .split('/')
                .next()
                .unwrap_or("");
            if without_scheme.eq_ignore_ascii_case(host) {
                return true;
            }
        }
    }
    false
}

fn is_public(path: &str) -> bool {
    path == "/healthz"
        || path == "/api/auth/state"
        || path == "/api/auth/login"
        || path == "/api/auth/logout"
}

/// 鉴权中间件：公开路径放行；其余要求有效 Cookie。
/// 写操作额外要求 Origin/Referer 与 Host 一致，或 `X-GLACC-CSRF: 1`。
pub async fn auth_middleware(
    State(auth): State<WebAuth>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    if !path.starts_with("/api") {
        return next.run(req).await;
    }
    if is_public(&path) {
        // 登录接口仍要求自定义头，挡住跨站表单。
        if req.method() != Method::GET
            && req.method() != Method::HEAD
            && !origin_matches_host(&req)
            && req
                .headers()
                .get("x-glacc-csrf")
                .and_then(|v| v.to_str().ok())
                != Some("1")
        {
            return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error":"csrf"}))).into_response();
        }
        return next.run(req).await;
    }

    if req.method() != Method::GET && req.method() != Method::HEAD {
        if !origin_matches_host(&req)
            && req
                .headers()
                .get("x-glacc-csrf")
                .and_then(|v| v.to_str().ok())
                != Some("1")
        {
            return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error":"csrf"}))).into_response();
        }
    }

    let ok = token_from_headers(req.headers())
        .map(|t| auth.store.verify_token(&t))
        .unwrap_or(false);
    if !ok {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    next.run(req).await
}
