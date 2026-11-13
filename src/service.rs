// 业务运行态与领取编排：把原 Tauri 命令里的逻辑抽成与传输层无关的函数，
// axum 处理器与内部调度器共用。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;

use crate::glacc::auth;
use crate::glacc::claim::{
    describe_reason, mask_phone, new_serial_gate, score_to_minutes, ClaimEvent, Reporter,
    SerialGate,
};
use crate::glacc::credentials::Credentials;
use crate::glacc::game::{GameApi, GameClient};
use crate::glacc::diag;
use crate::glacc::history;
use crate::glacc::instance::ClaimLock;
use crate::glacc::notify;
use crate::glacc::session::Session;
use crate::glacc::settings::AppSettings;
use crate::glacc::store;
use crate::glacc::sweep::{
    plan_from_store, AccountSweep, LiveClaimant, SweepAccount, SweepEntry, SweepEvent,
    SweepOutcome, SweepReporter, SweepState,
};

/// 首页共享状态：一轮领取的运行态与上一轮结果。
#[derive(Default)]
pub struct Runtime {
    pub running: bool,
    pub status: String,
    pub error: bool,
    pub last: Option<SweepOutcome>,
    pub live: SharedBoard,
}

#[derive(Debug, Clone)]
pub struct LiveEntry {
    pub key: String,
    pub label: String,
    pub state: &'static str,
    pub claimed: i32,
    pub total: i32,
    pub added_minutes: i32,
    pub balance_minutes: f64,
    pub message: Option<String>,
}

#[derive(Default)]
pub struct LiveBoard {
    pub entries: Vec<LiveEntry>,
    pub note: String,
    pub log: Vec<String>,
}

const LIVE_LOG_MAX: usize = 200;
pub type SharedBoard = Arc<Mutex<LiveBoard>>;

pub fn lock_board(board: &Mutex<LiveBoard>) -> std::sync::MutexGuard<'_, LiveBoard> {
    board.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn lock_runtime(runtime: &Mutex<Runtime>) -> std::sync::MutexGuard<'_, Runtime> {
    runtime.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn push_live_log(board: &mut LiveBoard, message: impl AsRef<str>) {
    let stamp = crate::glacc::diag::local_timestamp();
    let short = stamp.get(11..19).unwrap_or(&stamp).to_string();
    board.log.push(format!("{short}  {}", message.as_ref()));
    if board.log.len() > LIVE_LOG_MAX {
        let drop = board.log.len() - LIVE_LOG_MAX;
        board.log.drain(..drop);
    }
}

pub struct AppState {
    pub runtime: Arc<Mutex<Runtime>>,
    pub gate: SerialGate,
    pub auth_gate: SerialGate,
    pub cancel: Arc<AtomicBool>,
    pub started_at: std::time::Instant,
    pub tls_ready: bool,
    pub version: &'static str,
}

impl AppState {
    pub fn new(tls_ready: bool) -> Self {
        Self {
            runtime: Arc::new(Mutex::new(Runtime::default())),
            gate: new_serial_gate(),
            auth_gate: new_serial_gate(),
            cancel: Arc::new(AtomicBool::new(false)),
            started_at: std::time::Instant::now(),
            tls_ready,
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDto {
    pub key: String,
    pub label: String,
    pub is_current: bool,
    pub last_used: i64,
}

pub fn accounts_dto() -> Vec<AccountDto> {
    store::list_accounts()
        .into_iter()
        .enumerate()
        .map(|(i, account)| AccountDto {
            label: mask_phone(&account.phone_digits),
            key: account.key,
            is_current: i == 0,
            last_used: account.last_used,
        })
        .collect()
}

pub fn mask_phone_safe(phone: &str) -> String {
    let digits = Credentials::phone_digits(phone);
    if digits.len() == 11 {
        mask_phone(&digits)
    } else {
        "未知账号".to_string()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProgressDto {
    pub key: String,
    pub label: String,
    pub state: &'static str,
    pub claimed: i32,
    pub total: i32,
    pub added_minutes: i32,
    pub balance_minutes: f64,
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimProgressDto {
    pub running: bool,
    pub claimed: i32,
    pub total: i32,
    pub added_minutes: i32,
    pub balance_minutes: f64,
    pub status_text: String,
    pub status_is_error: bool,
    pub accounts: Vec<AccountProgressDto>,
    pub events: Vec<String>,
}

pub fn state_name(state: SweepState) -> &'static str {
    match state {
        SweepState::Pending => "pending",
        SweepState::Running => "running",
        SweepState::Done => "done",
        SweepState::Failed => "failed",
        SweepState::Skipped => "skipped",
    }
}

pub fn entry_dto(entry: &SweepEntry) -> AccountProgressDto {
    AccountProgressDto {
        key: entry.key.clone(),
        label: entry.label.clone(),
        state: state_name(entry.state),
        claimed: entry.claimed,
        total: entry.total,
        added_minutes: entry.added_minutes,
        balance_minutes: entry.balance_minutes,
        message: entry.message.clone(),
    }
}

pub fn progress_dto(runtime: &Runtime) -> ClaimProgressDto {
    if runtime.running {
        let board = lock_board(&runtime.live);
        let claimed = board.entries.iter().map(|e| e.claimed).sum();
        let total = board.entries.iter().map(|e| e.total).sum();
        let added = board.entries.iter().map(|e| e.added_minutes).sum();
        let balance = board
            .entries
            .iter()
            .map(|e| e.balance_minutes)
            .fold(0.0_f64, f64::max);
        return ClaimProgressDto {
            running: true,
            claimed,
            total,
            added_minutes: added,
            balance_minutes: balance,
            status_text: runtime.status.clone(),
            status_is_error: runtime.error,
            accounts: board
                .entries
                .iter()
                .map(|e| AccountProgressDto {
                    key: e.key.clone(),
                    label: e.label.clone(),
                    state: e.state,
                    claimed: e.claimed,
                    total: e.total,
                    added_minutes: e.added_minutes,
                    balance_minutes: e.balance_minutes,
                    message: e.message.clone(),
                })
                .collect(),
            events: board.log.clone(),
        };
    }

    let (claimed, total, added, balance, accounts) = match &runtime.last {
        Some(outcome) => (
            outcome.entries.iter().map(|e| e.claimed).sum(),
            outcome.entries.iter().map(|e| e.total).sum(),
            outcome.total_added_minutes,
            outcome
                .entries
                .iter()
                .map(|e| e.balance_minutes)
                .fold(0.0_f64, f64::max),
            outcome.entries.iter().map(entry_dto).collect(),
        ),
        None => (0, 0, 0, 0.0, Vec::new()),
    };
    let events = {
        let board = lock_board(&runtime.live);
        board.log.clone()
    };
    ClaimProgressDto {
        running: false,
        claimed,
        total,
        added_minutes: added,
        balance_minutes: balance,
        status_text: runtime.status.clone(),
        status_is_error: runtime.error,
        accounts,
        events,
    }
}

/// 每次领取之间的随机间隔。
pub fn claim_interval(min_sec: i32, max_sec: i32) -> Arc<dyn Fn() -> Duration + Send + Sync> {
    Arc::new(move || {
        let span = (max_sec - min_sec).max(0) as u64;
        let extra = if span == 0 {
            0
        } else {
            use rand::Rng;
            rand::thread_rng().gen_range(0..=span)
        };
        Duration::from_secs(min_sec as u64 + extra)
    })
}

pub fn plan_for(scope: &str) -> Vec<SweepAccount> {
    let all = plan_from_store();
    if scope.is_empty() || scope == "all" {
        return all;
    }
    all.into_iter().filter(|a| a.key == scope).collect()
}

/// 手动触发领取：先占住运行态再起线程，避免两个请求同时通过检查。
pub fn start_claim(state: Arc<AppState>, scope: &str) -> Result<(), String> {
    let queue = plan_for(scope);
    if queue.is_empty() {
        return Err("本机没有已登录账号，请先登录".to_string());
    }
    {
        let mut runtime = lock_runtime(&state.runtime);
        if runtime.running {
            return Err("正在领取，请等待本轮结束".to_string());
        }
        runtime.running = true;
        runtime.error = false;
        runtime.status = "正在启动领取…".to_string();
    }
    let trigger = if scope == "all" || scope.is_empty() {
        "手动领取".to_string()
    } else {
        format!("手动领取（单账号）")
    };
    std::thread::spawn(move || {
        let _ = run_sweep_blocking(state, queue, &trigger, true);
    });
    Ok(())
}

pub fn cancel_claim(state: &AppState) {
    state.cancel.store(true, Ordering::SeqCst);
}


// ── 短信登录 ──

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmsCodeDto {
    pub phone_label: String,
    pub resend_seconds: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResultDto {
    pub key: String,
    pub label: String,
}

fn cap_message(for_send: bool) -> String {
    if for_send {
        format!(
            "最多只能保存 {} 个账号（家庭自用上限），请先删除一个不再使用的账号，或改用已有账号登录",
            store::MAX_ACCOUNTS
        )
    } else {
        format!(
            "最多只能保存 {} 个账号（家庭自用上限），请先删除一个不再使用的账号后重新登录",
            store::MAX_ACCOUNTS
        )
    }
}

pub fn send_sms_blocking(phone: &str) -> Result<SmsCodeDto, String> {
    let digits = Credentials::phone_digits(phone);
    if digits.chars().count() == 11 && !store::can_add(&digits) {
        diag::warn(&format!("拒绝新增账号：已达上限 {} 个", store::MAX_ACCOUNTS));
        return Err(cap_message(true));
    }
    let mut cred = store::load(store::NO_KEY);
    let result = auth::send_sms(&mut cred, phone);
    if !result.ok {
        return Err(result.error);
    }
    Ok(SmsCodeDto {
        phone_label: mask_phone(&Credentials::phone_digits(cred.login_phone())),
        resend_seconds: 60,
    })
}

pub fn login_blocking(code: &str) -> Result<LoginResultDto, String> {
    let mut cred = store::load(store::NO_KEY);
    let result = auth::login(&mut cred, code.trim());
    if !result.ok {
        return Err(result.error);
    }
    if !store::can_add(&Credentials::phone_digits(&cred.phone)) {
        cred.block_persist = true;
        diag::warn(&format!(
            "拒绝保存新账号凭据：已达上限 {} 个",
            store::MAX_ACCOUNTS
        ));
        return Err(cap_message(false));
    }
    let key = store::adopt_phone_key(&mut cred);
    if key.is_empty() {
        return Err("登录成功但手机号无效，未能保存账号".to_string());
    }
    diag::info("短信登录成功：账号凭据已保存");
    Ok(LoginResultDto {
        key,
        label: mask_phone(&Credentials::phone_digits(&cred.phone)),
    })
}

/// 同步跑一轮领取。调用方负责放进阻塞线程。
pub fn run_sweep_blocking(
    state: Arc<AppState>,
    queue: Vec<SweepAccount>,
    trigger: &str,
    preclaimed: bool,
) -> Result<SweepOutcome, String> {
    if queue.is_empty() {
        return Err("本机没有已登录账号，请先登录".to_string());
    }

    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::SeqCst);
    let board: SharedBoard = Arc::new(Mutex::new(LiveBoard {
        entries: queue
            .iter()
            .map(|a| LiveEntry {
                key: a.key.clone(),
                label: a.label(),
                state: "pending",
                claimed: 0,
                total: 0,
                added_minutes: 0,
                balance_minutes: 0.0,
                message: None,
            })
            .collect(),
        note: String::new(),
        log: Vec::new(),
    }));

    {
        let mut runtime = lock_runtime(&state.runtime);
        if runtime.running && !preclaimed {
            return Err("正在领取，请等待本轮结束".to_string());
        }
        runtime.running = true;
        runtime.error = false;
        runtime.status = format!("正在领取（{trigger}）…");
        runtime.last = None;
        runtime.live = board.clone();
        push_live_log(&mut lock_board(&runtime.live), format!("开始本轮领取：{trigger}"));
    }

    // 文件锁：跨进程/跨容器副本互斥。
    let _cross_process = match ClaimLock::acquire() {
        Some(lock) => lock,
        None => {
            let mut runtime = lock_runtime(&state.runtime);
            runtime.running = false;
            runtime.error = true;
            runtime.status = "另一个实例正在领取，请稍后重试".to_string();
            return Err("另一个实例正在领取".to_string());
        }
    };

    let settings = AppSettings::load();
    let retry = settings.network_retry_count;
    let (min_sec, max_sec) = settings.interval_secs();
    let interval = claim_interval(min_sec, max_sec);

    let board_for_sweep = board.clone();
    let sweep_reporter: SweepReporter = Arc::new(move |ev: &SweepEvent| {
        let mut board = lock_board(&board_for_sweep);
        match ev {
            SweepEvent::Started { index, label, .. } => {
                let total = board.entries.len();
                if let Some(slot) = board.entries.get_mut(*index) {
                    slot.state = "running";
                    slot.message = None;
                }
                push_live_log(
                    &mut board,
                    format!("账号 {label}（{}/{}）开始领取", index + 1, total),
                );
            }
            SweepEvent::Finished { index, entry } => {
                if let Some(slot) = board.entries.get_mut(*index) {
                    slot.state = state_name(entry.state);
                    slot.claimed = entry.claimed;
                    slot.total = entry.total;
                    slot.added_minutes = entry.added_minutes;
                    slot.balance_minutes = entry.balance_minutes;
                    slot.message = entry.message.clone();
                }
                push_live_log(
                    &mut board,
                    format!(
                        "账号 {} {}：{}/{} 次，+{} 分钟",
                        entry.label,
                        sweep_state_cn(entry.state),
                        entry.claimed,
                        entry.total,
                        entry.added_minutes
                    ),
                );
            }
        }
    });

    let board_for_claim = board.clone();
    let claim_reporter: Reporter = Arc::new(move |ev: &ClaimEvent| {
        let mut board = lock_board(&board_for_claim);
        board.note = ev.message.clone();
        push_live_log(&mut board, &ev.message);
        if let Some(slot) = board.entries.iter_mut().find(|e| e.state == "running") {
            slot.claimed = ev.claimed;
            if ev.total > 0 {
                slot.total = ev.total;
            }
            if let Some(add) = ev.added_minutes {
                slot.added_minutes += add;
            }
        }
    });

    let claimant = LiveClaimant::new(retry, interval)
        .with_gate(state.gate.clone())
        .with_reporter(claim_reporter);
    let mut sweep = AccountSweep::new(queue, claimant)
        .with_cancel_flag(cancel)
        .with_reporter(sweep_reporter);

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sweep.run()));
    let mut runtime = lock_runtime(&state.runtime);
    runtime.running = false;

    let result = match outcome {
        Ok(outcome) => {
            history::record_from_outcome(&outcome);
            runtime.status = format!(
                "本轮完成：执行 {}，跳过 {}，失败 {}，+{} 分钟{}",
                outcome.ran,
                outcome.skipped,
                outcome.failed,
                outcome.total_added_minutes,
                if outcome.cancelled { "（已取消）" } else { "" }
            );
            runtime.error = outcome.failed > 0;
            runtime.last = Some(outcome.clone());
            Ok(outcome)
        }
        Err(_) => {
            runtime.error = true;
            runtime.status = "领取线程异常退出，请查看日志".to_string();
            runtime.last = None;
            Err("领取线程异常退出".to_string())
        }
    };

    drop(runtime);

    if let Ok(outcome) = &result {
        let settings = AppSettings::load();
        if notify::is_configured(&settings.server_key) {
            let title = format!(
                "glacc-auto {}",
                if outcome.cancelled { "已取消" } else { "领取完成" }
            );
            let desp = format!(
                "执行 {}，跳过 {}，失败 {}，本轮 +{} 分钟",
                outcome.ran, outcome.skipped, outcome.failed, outcome.total_added_minutes
            );
            let _ = notify::send(&settings.server_key, &title, &desp);
        }
    }
    result
}

pub fn sweep_state_cn(state: SweepState) -> &'static str {
    match state {
        SweepState::Pending => "待领取",
        SweepState::Running => "领取中",
        SweepState::Done => "完成",
        SweepState::Failed => "失败",
        SweepState::Skipped => "跳过",
    }
}

// ── 首页快照 ──

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotDto {
    pub logged_in: bool,
    pub label: String,
    pub balance_minutes: f64,
    pub balance_known: bool,
    pub claimed: i32,
    pub total: i32,
    pub estimated_minutes: i32,
    pub status_text: String,
    pub status_is_error: bool,
}

fn snapshot_failure(logged_in: bool, label: &str, status: String, is_error: bool) -> SnapshotDto {
    SnapshotDto {
        logged_in,
        label: label.to_string(),
        balance_minutes: 0.0,
        balance_known: false,
        claimed: 0,
        total: 0,
        estimated_minutes: 0,
        status_text: status,
        status_is_error: is_error,
    }
}

pub fn snapshot_blocking() -> SnapshotDto {
    let Some(account) = store::list_accounts().into_iter().next() else {
        return snapshot_failure(false, "", "登录后同步今日任务".to_string(), false);
    };
    let label = mask_phone(&account.phone_digits);

    let mut cred = store::load(&account.key);
    if !cred.has_token() && !cred.has_refresh_token() {
        return snapshot_failure(
            true,
            &label,
            "本机无登录凭证，需先短信登录".to_string(),
            true,
        );
    }

    let retry = AppSettings::load().network_retry_count;
    let mut notice = String::new();
    let mut session = Session::new(&mut cred, move || retry);
    if session.jwt_needs_refresh() {
        let r = session.ensure_jwt(false);
        if !r.ok {
            return snapshot_failure(true, &label, format!("登录态不可用：{}", r.error), true);
        }
    }
    let mut game = GameClient::new(&mut session);

    let stages = {
        let mut on_retry = |attempt: i32, wait: std::time::Duration| {
            notice = format!(
                "网络波动：任务进度查询失败，{} 秒后自动重试（第 {} 次）",
                wait.as_secs(),
                attempt
            );
        };
        game.get_task_stages(None, false, Some(&mut on_retry))
    };
    let score = {
        let mut on_retry = |attempt: i32, wait: std::time::Duration| {
            notice = format!(
                "网络波动：余额查询失败，{} 秒后自动重试（第 {} 次）",
                wait.as_secs(),
                attempt
            );
        };
        game.get_wallet_score(Some(&mut on_retry))
    };

    let Some(list) = stages.value.as_ref() else {
        let text = describe_reason(stages.reason, &stages.detail);
        return snapshot_failure(true, &label, text, true);
    };
    let mut dto = snapshot_failure(true, &label, String::new(), false);
    dto.claimed = list.iter().map(|s| s.stage_current).sum();
    dto.total = list.iter().map(|s| s.stage_sum).sum();
    let total_score: i64 = list.iter().map(|s| s.total_score.max(0) as i64).sum();
    dto.estimated_minutes = score_to_minutes(total_score).round_ties_even() as i32;

    match score.value {
        Some(score) => {
            dto.balance_minutes = score_to_minutes(score);
            dto.balance_known = true;
        }
        None => {
            let text = describe_reason(score.reason, &score.detail);
            dto.status_text = text;
            dto.status_is_error = true;
            return dto;
        }
    }

    dto.status_text = if dto.total == 0 {
        "今日暂无任务".to_string()
    } else if dto.claimed >= dto.total {
        "今日任务已全部完成".to_string()
    } else {
        format!(
            "今日任务待领取，还差 {} 次{}",
            dto.total - dto.claimed,
            if notice.is_empty() {
                String::new()
            } else {
                format!("（{notice}）")
            }
        )
    };
    dto.status_is_error = false;
    dto
}
