// glacc-auto Linux 服务入口：axum + 内部 cron 调度 + 原协议核心。

mod api;
mod config;
mod glacc;
mod scheduler;
mod service;
mod web_auth;

use std::net::SocketAddr;
use std::sync::Arc;

use tracing_subscriber::EnvFilter;

use crate::scheduler::Scheduler;
use crate::service::AppState;
use crate::web_auth::WebAuth;

#[tokio::main]
async fn main() {
    config::load_dotenv();

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tower_http=warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    if let Err(e) = run().await {
        eprintln!("启动失败：{e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let cfg = config::Config::from_env()?;

    // 数据目录先就绪，后续 auth/settings/logs 都依赖它。
    let data_root = glacc::store::data_root();
    std::fs::create_dir_all(&data_root)
        .map_err(|e| format!("创建数据目录失败（{}）：{e}", data_root.display()))?;
    std::fs::create_dir_all(data_root.join("accounts"))
        .map_err(|e| format!("创建账号目录失败：{e}"))?;
    glacc::diag::prune();

    let tls_ready = match glacc::tls::initialize() {
        Ok(()) => {
            glacc::diag::info("TLS 指纹库加载成功");
            true
        }
        Err(e) => {
            let msg = format!("TLS 指纹库不可用：{e}");
            glacc::diag::warn(&msg);
            tracing::warn!("{msg}");
            if cfg.require_tls_lib {
                return Err(msg);
            }
            false
        }
    };

    let auth = WebAuth::new(cfg.secure_cookie, cfg.trust_proxy)?;
    let state = Arc::new(AppState::new(tls_ready));
    let scheduler = Scheduler::load(state.clone())?;
    scheduler.start();

    let app = api::router(state.clone(), auth, scheduler, &cfg.web_dir);
    let listener = tokio::net::TcpListener::bind(cfg.listen)
        .await
        .map_err(|e| format!("监听 {} 失败：{e}", cfg.listen))?;

    glacc::diag::info(&format!(
        "服务启动：http://{}，数据目录 {}，TLS 指纹库 {}",
        cfg.listen,
        data_root.display(),
        if tls_ready { "已加载" } else { "未加载" }
    ));
    println!("glacc-auto listening on http://{}", cfg.listen);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .map_err(|e| format!("HTTP 服务异常：{e}"))?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    glacc::diag::info("收到退出信号，正在停止服务");
}
