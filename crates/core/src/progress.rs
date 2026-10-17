//! 进度载荷:扫描与压缩两阶段共用,经 `on_progress` 回调上报给外壳(CLI / Tauri)。

/// 压缩流程的阶段
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Step 1 扫描(统计条目与大小)
    Scanning,
    /// Step 3 逐条目压缩
    Compressing,
}

/// 单条进度(条目级粒度, 见技术文档 _docs/compress.md 第 7 节)
#[derive(Debug, Clone)]
pub struct Progress {
    pub phase: Phase,
    /// 条目计数:扫描 = 已扫描条目,压缩 = 已完成条目(从 1 开始)
    pub done: usize,
    /// 条目总数
    pub total: usize,
    /// 已累计的原始字节数(压缩阶段为已完成条目的 size 之和)
    pub done_bytes: u64,
    /// 条目总字节数(扫描阶段未知,为 0;展示层应退化为 done/total)
    pub total_bytes: u64,
    /// 当前条目名
    pub entry: String,
}
