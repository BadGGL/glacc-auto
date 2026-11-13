// glacc 协议层模块。
// 由原 Tauri 版逐字移植；平台相关部分（TLS 动态库、时间、单实例）已替换为 Linux 实现。

pub mod auth;
pub mod claim;
pub mod constants;
pub mod credentials;
pub mod device;
pub mod diag;
pub mod game;
pub mod history;
pub mod instance;
pub mod notify;
pub mod result;
pub mod session;
pub mod settings;
pub mod sign;
pub mod store;
pub mod sweep;
pub mod tls;
