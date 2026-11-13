// tls-client 原生库（bogdanfinn/tls-client cffi）最小 FFI 封装。
// Linux 版：用 libloading 加载 libtls-client.so，导出函数与 Windows DLL 完全一致
// （request / freeMemory），每个请求载荷自带完整配置，返回响应 JSON（含 id），
// 用完 freeMemory(id)。
//
// 探测顺序：GLACC_TLS_LIB 环境变量 → 可执行文件同目录 → ./lib → /usr/local/lib
// → /usr/lib → 开发期 target 目录。绝不静默降级回系统 TLS 栈，否则设备指纹会变。

use std::ffi::{c_char, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use libloading::Library;
use serde::Serialize;

/// tls-client cffi 请求载荷。字段名与原版逐字一致（对应 Go 侧 RequestInput JSON tag）。
#[derive(Debug, Clone, Serialize)]
pub struct TlsRequestPayload {
    #[serde(rename = "tlsClientIdentifier")]
    pub tls_client_identifier: String,
    #[serde(rename = "requestMethod")]
    pub request_method: String,
    #[serde(rename = "requestUrl")]
    pub request_url: String,
    #[serde(rename = "requestBody")]
    pub request_body: Option<String>,
    /// 保序键值对；headerOrder 由它派生，顺序会直接影响发出的头部顺序与 TLS 指纹。
    #[serde(rename = "headers", serialize_with = "serialize_headers")]
    pub headers: Vec<(String, String)>,
    #[serde(rename = "headerOrder")]
    pub header_order: Option<Vec<String>>,
    #[serde(rename = "timeoutSeconds")]
    pub timeout_seconds: i32,
    #[serde(rename = "followRedirects")]
    pub follow_redirects: bool,
    #[serde(rename = "withoutCookieJar")]
    pub without_cookie_jar: bool,
    #[serde(rename = "isByteRequest")]
    pub is_byte_request: bool,
    #[serde(rename = "isByteResponse")]
    pub is_byte_response: bool,
    #[serde(rename = "catchPanics")]
    pub catch_panics: bool,
}

/// 按原版 Dictionary 序列化语义把保序键值对写成 JSON 对象（非数组）。
fn serialize_headers<S>(headers: &[(String, String)], s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeMap;
    let mut map = s.serialize_map(Some(headers.len()))?;
    for (k, v) in headers {
        map.serialize_entry(k, v)?;
    }
    map.end()
}

/// 原生库响应（仅业务需要的字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsResponse {
    pub status: i32,
    pub body: String,
}

/// 发送失败的两类故障：指纹库环境故障 vs 请求失败。
#[derive(Debug)]
pub enum TlsError {
    Unavailable(String),
    Request(String),
}

impl std::fmt::Display for TlsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TlsError::Unavailable(msg) | TlsError::Request(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for TlsError {}

// ── 原生库入口 ──

type RequestFn = unsafe extern "C" fn(*const c_char) -> *mut c_char;
type FreeMemoryFn = unsafe extern "C" fn(*const c_char);

/// 已加载的原生库。Library 必须常驻：函数指针指向它的映射内存。
struct TlsModule {
    _lib: Library,
    request: RequestFn,
    free_memory: FreeMemoryFn,
}

// SAFETY: 原生库是无会话的纯函数式 C ABI；每个请求载荷自带全部状态，
// 库内部对并发请求有 Go runtime 的调度保护。
unsafe impl Send for TlsModule {}
unsafe impl Sync for TlsModule {}

static MODULE: OnceLock<TlsModule> = OnceLock::new();

fn lib_names() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &["libtls-client.dylib"]
    } else {
        &["libtls-client.so", "libtls-client.so.1"]
    }
}

/// 定位并加载原生库；失败返回明确错误。
pub fn initialize() -> Result<(), TlsError> {
    if MODULE.get().is_some() {
        return Ok(());
    }

    let mut probes: Vec<PathBuf> = Vec::new();
    if let Ok(explicit) = std::env::var("GLACC_TLS_LIB") {
        if !explicit.is_empty() {
            probes.push(PathBuf::from(explicit));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in lib_names() {
                probes.push(dir.join(name));
                probes.push(dir.join("lib").join(name));
            }
        }
    }
    for name in lib_names() {
        probes.push(PathBuf::from("./lib").join(name));
        probes.push(PathBuf::from("/usr/local/lib").join(name));
        probes.push(PathBuf::from("/usr/lib").join(name));
    }
    // 开发期：cargo run 时库常放在仓库 lib/ 下
    for name in lib_names() {
        probes.push(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("lib")
                .join(name),
        );
    }

    let found = probes.iter().find(|p| p.is_file()).ok_or_else(|| {
        TlsError::Unavailable(format!(
            "未找到 TLS 指纹库（{}）；请把 libtls-client.so 放到 /app/lib 或设置 GLACC_TLS_LIB",
            lib_names().join(" / ")
        ))
    })?;
    initialize_from(found)
}

/// 按显式路径加载（测试/开发用）；与旧版一致：首次生效。
pub fn initialize_from(path: &Path) -> Result<(), TlsError> {
    if MODULE.get().is_some() {
        return Ok(());
    }
    let module = load(path)?;
    let _ = MODULE.set(module);
    Ok(())
}

fn load(path: &Path) -> Result<TlsModule, TlsError> {
    // SAFETY: 动态库路径来自受控探测；加载后 Library 随 TlsModule 常驻进程。
    let lib = unsafe { Library::new(path) }.map_err(|e| {
        TlsError::Unavailable(format!("加载 TLS 指纹库失败：{}（{e}）", path.display()))
    })?;
    // SAFETY: 导出签名由 tls-client cffi 固定；名称以 NUL 结尾。
    let request: RequestFn = unsafe {
        *lib.get::<RequestFn>(b"request\0").map_err(|e| {
            TlsError::Unavailable(format!("TLS 指纹库缺少导出函数 request：{e}"))
        })?
    };
    // SAFETY: 同上。
    let free_memory: FreeMemoryFn = unsafe {
        *lib.get::<FreeMemoryFn>(b"freeMemory\0").map_err(|e| {
            TlsError::Unavailable(format!("TLS 指纹库缺少导出函数 freeMemory：{e}"))
        })?
    };
    Ok(TlsModule {
        _lib: lib,
        request,
        free_memory,
    })
}

// ── 调用 ──

/// 执行一次 HTTP 请求（阻塞调用，调用方须放在后台线程）。
pub fn send(payload: &TlsRequestPayload) -> Result<TlsResponse, TlsError> {
    let raw = raw_request(payload)?;
    // id 必须最先取出：解析在 id 之前失败就拿不到 id，原生缓冲永不释放。
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return Err(TlsError::Request(format!("响应 JSON 解析失败：{e}"))),
    };
    if !value.is_object() {
        return Err(TlsError::Request("原生库响应不是 JSON 对象".to_string()));
    }
    let id = value.get("id").and_then(|v| v.as_str()).map(str::to_owned);
    let result = interpret(&value);
    if let Some(id) = id.as_deref().filter(|s| !s.is_empty()) {
        free(id);
    }
    result
}

fn raw_request(payload: &TlsRequestPayload) -> Result<String, TlsError> {
    initialize()?;
    let module = MODULE.get().expect("initialize() 成功后 MODULE 必已设置");
    let json = serde_json::to_string_pretty(payload)
        .map_err(|e| TlsError::Request(format!("载荷序列化失败：{e}")))?;
    let c_json =
        CString::new(json).map_err(|e| TlsError::Request(format!("载荷含 NUL：{e}")))?;
    // SAFETY: 入参为有效 NUL 结尾的 C 字符串且在调用期间存活；返回指针归原生库所有，
    // 这里只读借用，释放走 freeMemory(id)。
    let ptr = unsafe { (module.request)(c_json.as_ptr()) };
    if ptr.is_null() {
        return Err(TlsError::Request("TLS 库返回空指针".to_string()));
    }
    let raw = unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    if raw.is_empty() {
        return Err(TlsError::Request("TLS 库返回空响应".to_string()));
    }
    Ok(raw)
}

fn interpret(value: &serde_json::Value) -> Result<TlsResponse, TlsError> {
    let status = value
        .get("status")
        .and_then(|v| v.as_i64())
        .filter(|n| *n >= i32::MIN as i64 && *n <= i32::MAX as i64)
        .map(|n| n as i32)
        .unwrap_or(0);
    let body = value
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if status <= 0 {
        return Err(TlsError::Request(if body.is_empty() {
            "网络不可达或超时（原生库未给出原因）".to_string()
        } else {
            body
        }));
    }
    Ok(TlsResponse { status, body })
}

fn free(id: &str) {
    let Some(module) = MODULE.get() else {
        return;
    };
    if let Ok(c_id) = CString::new(id) {
        // SAFETY: id 为原生库自身生成的 UUID；对任意 id 调用均安全、幂等。
        unsafe { (module.free_memory)(c_id.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_json_shape_matches_legacy() {
        let headers: Vec<(String, String)> = vec![
            ("appid".to_string(), "8".to_string()),
            ("user-agent".to_string(), "UA".to_string()),
            ("authorization".to_string(), "Bearer x".to_string()),
        ];
        let payload = TlsRequestPayload {
            tls_client_identifier: "okhttp4_android_13".to_string(),
            request_method: "POST".to_string(),
            request_url: "https://example.com/a".to_string(),
            request_body: Some("{}".to_string()),
            header_order: Some(headers.iter().map(|(k, _)| k.clone()).collect()),
            headers,
            timeout_seconds: 15,
            follow_redirects: true,
            without_cookie_jar: true,
            is_byte_request: false,
            is_byte_response: false,
            catch_panics: true,
        };
        let v = serde_json::to_value(&payload).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 12);
        let h = obj["headers"].as_object().expect("headers 应为对象");
        assert_eq!(h["appid"], "8");
        assert_eq!(obj["headerOrder"].as_array().unwrap().len(), 3);
    }
}
