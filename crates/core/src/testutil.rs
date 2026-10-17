//! 测试共用工具(仅测试构建)。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 并行测试互不干扰的唯一临时根目录
pub(crate) fn unique_temp(tag: &str) -> PathBuf {
    let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("zip-tool-{}-{}-{}", tag, std::process::id(), n))
}
