//! 随机包名生成:避免压缩包名本身泄露敏感信息。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// 生成 16 位十六进制随机包名(不引入额外依赖:
/// 时间纳秒 ^ 进程id ^ 内部计数器,经 splitmix64 扩散)
pub fn random_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let n = nanos
        ^ (std::process::id() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0xBF58_476D_1CE4_E5B9);
    // splitmix64
    let mut z = n.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    format!("{z:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_name_is_16_hex_chars() {
        let name = random_name();
        assert_eq!(name.len(), 16);
        assert!(name.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn random_names_are_unique() {
        let a = random_name();
        let b = random_name();
        assert_ne!(a, b);
    }
}
