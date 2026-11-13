// 跨进程领取互斥（Linux flock）。
//
// 进程内的 SerialGate 只能挡住同一进程里的重复调用；定时任务与手动领取可能来自
// 不同进程/不同容器副本，必须有一把文件锁。锁的生命周期 = 文件句柄存在期，
// 进程崩溃时内核自动回收，不会留下僵尸锁。

use std::fs::{File, OpenOptions};
use std::path::PathBuf;

use fs2::FileExt;

use crate::glacc::store;

/// 锁文件名（位于数据根目录）。
pub const CLAIM_LOCK_FILE: &str = "claim.lock";

pub struct ClaimLock {
    file: File,
}

impl ClaimLock {
    /// 尝试取得领取锁。None = 另一个进程/实例正在领取。
    pub fn acquire() -> Option<Self> {
        Self::acquire_at(&lock_path())
    }

    /// 指定路径（测试用）。
    pub fn acquire_at(path: &PathBuf) -> Option<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .ok()?;
        match file.try_lock_exclusive() {
            Ok(()) => {
                // 写入 pid 便于诊断；失败不影响锁语义。
                use std::io::Write;
                let _ = writeln!(&file, "{}", std::process::id());
                Some(Self { file })
            }
            Err(_) => None,
        }
    }
}

impl Drop for ClaimLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub fn lock_path() -> PathBuf {
    store::data_root().join(CLAIM_LOCK_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_until_released() {
        let path = std::env::temp_dir().join(format!(
            "glacc-claim-lock-{}-{}.lock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first = ClaimLock::acquire_at(&path).expect("首次获取应成功");
        assert!(ClaimLock::acquire_at(&path).is_none(), "持锁期间不得再次获取");
        drop(first);
        let again = ClaimLock::acquire_at(&path).expect("释放后应可再次获取");
        drop(again);
        let _ = std::fs::remove_file(path);
    }
}
