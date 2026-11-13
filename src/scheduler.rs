// 多条 cron 定时任务。
//
// 与原版 Windows 计划任务不同：规则住在服务进程内，不依赖 schtasks；
// 支持多条规则、IANA 时区、全账号/单账号范围、错过补跑与随机抖动。
// 规则落盘 <data_root>/schedules.json，服务重启后继续生效。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use serde::{Deserialize, Serialize};

use crate::glacc::store;
use crate::service::{lock_runtime, plan_for, run_sweep_blocking, AppState};

pub const SCHEDULE_FILE: &str = "schedules.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleRule {
    pub id: String,
    pub name: String,
    /// 5 段（分 时 日 月 周）或 6 段（秒 分 时 日 月 周）cron。
    pub cron: String,
    pub timezone: String,
    /// all | 账号 key
    pub scope: String,
    pub enabled: bool,
    /// 服务停机期间错过的最近一次，启动后补跑。
    pub catch_up: bool,
    /// 触发后随机延后 0..jitter 秒，避免整点同时打接口。
    pub jitter_secs: u64,
    pub created_at: i64,
    pub last_run_at: Option<i64>,
    pub last_status: Option<String>,
}

impl ScheduleRule {
    pub fn new(name: String, cron: String, timezone: String, scope: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            cron,
            timezone,
            scope,
            enabled: true,
            catch_up: true,
            jitter_secs: 0,
            created_at: Utc::now().timestamp(),
            last_run_at: None,
            last_status: None,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("规则名称不能为空".to_string());
        }
        parse_cron(&self.cron)?;
        self.timezone
            .parse::<Tz>()
            .map_err(|_| format!("时区无效：{}", self.timezone))?;
        if self.scope != "all" && !store::list_accounts().iter().any(|a| a.key == self.scope) {
            return Err("scope 指向的账号不存在".to_string());
        }
        if self.jitter_secs > 3600 {
            return Err("jitterSecs 不能超过 3600".to_string());
        }
        Ok(())
    }

    fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let tz: Tz = self.timezone.parse().ok()?;
        let schedule = parse_cron(&self.cron).ok()?;
        schedule
            .after(&after.with_timezone(&tz))
            .next()
            .map(|d| d.with_timezone(&Utc))
    }
}

/// `cron` crate 需要秒字段；用户按习惯写 5 段时自动补 `0 `。
fn parse_cron(raw: &str) -> Result<Schedule, String> {
    let trimmed = raw.trim();
    let fields = trimmed.split_whitespace().count();
    let normalized = match fields {
        5 => format!("0 {trimmed}"),
        6 | 7 => trimmed.to_string(),
        _ => return Err(format!("cron 需要 5 或 6 段，实际 {fields} 段")),
    };
    Schedule::from_str(&normalized).map_err(|e| format!("cron 表达式无效：{e}"))
}

pub struct Scheduler {
    state: Arc<AppState>,
    rules: Arc<Mutex<Vec<ScheduleRule>>>,
    next_run: Arc<Mutex<HashMap<String, i64>>>,
    path: Arc<PathBuf>,
}

impl Scheduler {
    pub fn load(state: Arc<AppState>) -> Result<Arc<Self>, String> {
        let path = store::data_root().join(SCHEDULE_FILE);
        let rules = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<Vec<ScheduleRule>>(&text)
                .map_err(|e| format!("schedules.json 解析失败：{e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("读取 schedules.json 失败：{e}")),
        };
        let scheduler = Arc::new(Self {
            state,
            rules: Arc::new(Mutex::new(rules)),
            next_run: Arc::new(Mutex::new(HashMap::new())),
            path: Arc::new(path),
        });
        scheduler.recompute_all();
        Ok(scheduler)
    }

    fn lock_rules(&self) -> std::sync::MutexGuard<'_, Vec<ScheduleRule>> {
        self.rules.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn save(&self) -> Result<(), String> {
        save_rules(&self.rules, &self.path)
    }

    fn recompute_all(&self) {
        let now = Utc::now();
        let mut next = self.next_run.lock().unwrap_or_else(|e| e.into_inner());
        next.clear();
        for rule in self.lock_rules().iter().filter(|r| r.enabled) {
            if let Some(t) = rule.next_after(now) {
                next.insert(rule.id.clone(), t.timestamp());
            }
        }
    }

    pub fn list(&self) -> Vec<ScheduleRule> {
        self.lock_rules().clone()
    }

    pub fn create(&self, mut rule: ScheduleRule) -> Result<ScheduleRule, String> {
        rule.id = uuid::Uuid::new_v4().to_string();
        rule.created_at = Utc::now().timestamp();
        rule.last_run_at = None;
        rule.last_status = None;
        rule.validate()?;
        {
            let mut rules = self.lock_rules();
            if rules.iter().any(|r| r.name == rule.name) {
                return Err("同名规则已存在".to_string());
            }
            rules.push(rule.clone());
        }
        self.save()?;
        self.recompute_all();
        Ok(rule)
    }

    pub fn update(&self, id: &str, patch: ScheduleRule) -> Result<ScheduleRule, String> {
        let updated = {
            let mut rules = self.lock_rules();
            let index = rules
                .iter()
                .position(|r| r.id == id)
                .ok_or_else(|| "规则不存在".to_string())?;
            let mut next = rules[index].clone();
            next.name = patch.name;
            next.cron = patch.cron;
            next.timezone = patch.timezone;
            next.scope = patch.scope;
            next.enabled = patch.enabled;
            next.catch_up = patch.catch_up;
            next.jitter_secs = patch.jitter_secs;
            next.validate()?;
            if rules
                .iter()
                .enumerate()
                .any(|(i, r)| i != index && r.name == next.name)
            {
                return Err("同名规则已存在".to_string());
            }
            rules[index] = next.clone();
            next
        };
        self.save()?;
        self.recompute_all();
        Ok(updated)
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        {
            let mut rules = self.lock_rules();
            let before = rules.len();
            rules.retain(|r| r.id != id);
            if rules.len() == before {
                return Err("规则不存在".to_string());
            }
        }
        self.next_run
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        self.save()
    }

    /// 立即执行一条规则（不改变下一次计划时间）。
    pub fn run_now(&self, id: &str) -> Result<(), String> {
        let rule = self
            .lock_rules()
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| "规则不存在".to_string())?;
        self.fire(rule);
        Ok(())
    }

    fn fire(&self, rule: ScheduleRule) {
        let state = self.state.clone();
        let queue = plan_for(&rule.scope);
        if queue.is_empty() {
            set_status(&self.rules, &self.path, &rule.id, "跳过：没有匹配账号");
            return;
        }
        {
            let runtime = lock_runtime(&state.runtime);
            if runtime.running {
                set_status(&self.rules, &self.path, &rule.id, "跳过：上一轮仍在进行");
                return;
            }
        }
        let trigger = format!("定时任务「{}」", rule.name);
        set_status(&self.rules, &self.path, &rule.id, "执行中");
        let rules = self.rules.clone();
        let path = self.path.clone();
        let id = rule.id.clone();
        std::thread::spawn(move || {
            let outcome = run_sweep_blocking(state, queue, &trigger, false);
            let status = match &outcome {
                Ok(o) => format!(
                    "完成：执行 {}，跳过 {}，失败 {}，+{} 分钟",
                    o.ran, o.skipped, o.failed, o.total_added_minutes
                ),
                Err(e) => format!("失败：{e}"),
            };
            set_status(&rules, &path, &id, &status);
        });
    }

    /// 启动监督循环：每 5 秒检查一次到期规则。
    pub fn start(self: &Arc<Self>) {
        let scheduler = self.clone();
        tokio::spawn(async move {
            scheduler.catch_up_once();
            loop {
                scheduler.tick().await;
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
    }

    fn catch_up_once(&self) {
        let now = Utc::now().timestamp();
        let due: Vec<ScheduleRule> = self
            .lock_rules()
            .iter()
            .filter(|r| r.enabled && r.catch_up)
            .filter(|r| {
                let Some(last) = r.last_run_at else {
                    return false;
                };
                if last >= now - 60 {
                    return false;
                }
                let base = DateTime::from_timestamp(last, 0).unwrap_or_else(Utc::now);
                r.next_after(base).is_some_and(|t| t.timestamp() <= now)
            })
            .cloned()
            .collect();
        for rule in due {
            self.fire(rule);
        }
    }

    async fn tick(self: &Arc<Self>) {
        let now = Utc::now().timestamp();
        let due: Vec<ScheduleRule> = {
            let rules = self.lock_rules();
            let next = self.next_run.lock().unwrap_or_else(|e| e.into_inner());
            rules
                .iter()
                .filter(|r| r.enabled)
                .filter(|r| next.get(&r.id).is_some_and(|t| *t <= now))
                .cloned()
                .collect()
        };
        for rule in due {
            // 先推进下一次触发时间，再异步执行；否则抖动期间的下一次 tick 会重复派发。
            let next = rule.next_after(Utc::now());
            {
                let mut map = self.next_run.lock().unwrap_or_else(|e| e.into_inner());
                match next {
                    Some(t) => {
                        map.insert(rule.id.clone(), t.timestamp());
                    }
                    None => {
                        map.remove(&rule.id);
                    }
                }
            }
            let this = self.clone();
            // 抖动与执行都放到独立任务：一条规则的随机延迟不能拖住其它规则。
            tokio::spawn(async move {
                if rule.jitter_secs > 0 {
                    use rand::Rng;
                    let jitter = rand::thread_rng().gen_range(0..=rule.jitter_secs);
                    if jitter > 0 {
                        tokio::time::sleep(Duration::from_secs(jitter)).await;
                    }
                }
                this.fire(rule);
            });
        }
    }
}

fn save_rules(rules: &Mutex<Vec<ScheduleRule>>, path: &Path) -> Result<(), String> {
    let json = serde_json::to_string_pretty(&*rules.lock().unwrap_or_else(|e| e.into_inner()))
        .map_err(|e| format!("序列化规则失败：{e}"))?;
    store::write_atomic(path, &json).map_err(|e| format!("写入规则失败：{e}"))
}

fn set_status(rules: &Mutex<Vec<ScheduleRule>>, path: &Path, id: &str, status: &str) {
    {
        let mut guard = rules.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(rule) = guard.iter_mut().find(|r| r.id == id) {
            rule.last_run_at = Some(Utc::now().timestamp());
            rule.last_status = Some(status.to_string());
        }
    }
    let _ = save_rules(rules, path);
}
