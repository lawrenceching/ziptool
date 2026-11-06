//! 重复文件查找(只读, 见 _docs/dedup.md):
//! 阶段 1 元数据扫描(两遍遍历, 只 stat)找出候选组(「同文件名+同大小」∪「同扩展名+同大小」,
//! 并查集允许传递连接);
//! 阶段 2/3 前 1 MiB BLAKE3 前缀分桶 → 前缀即全文件直接确认 / <500 MiB 全量哈希确认 /
//! ≥500 MiB 比前 10 MiB 转待确认;
//! 阶段 4 仅对用户选中的待确认组做全量 BLAKE3(壳层在 UI 勾选后才调用)。
//! 全程单线程顺序读, 只读文件系统; 组处理原子: 取消时当前组退回原状态, 不产生半个组。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::CancelFlag;
use crate::exclude::{ExcludeRules, effective_patterns, matches_patterns, validate_patterns};
use crate::progress::{Phase, Progress};
use crate::{CoreError, Result};

/// 哈希读缓冲(固定 1 MiB; 每次比对/校验分配一份, 跨文件复用)
const READ_BUF_BYTES: usize = 1 << 20;
/// 目录树内每处理 N 个条目检查一次取消旗标(与 scan.rs 同粒度)
const CANCEL_CHECK_EVERY: u32 = 512;
/// 比对/校验进度: 每读约 16 MB 发一次
const PROGRESS_BYTES_EVERY: u64 = 16 << 20;
/// 进度时间节流(约 100 ms 一次, 避免百万文件刷爆 IPC)
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// 查重阈值; Default = 1 MiB / 500 MiB / 10 MiB(二进制 MiB, 与用户口径「MB」对应)。
/// 阈值不暴露到 UI, 但可配置以便单测用小值覆盖大文件分支。
#[derive(Debug, Clone)]
pub struct DedupOptions {
    /// 阶段 2 前缀比对读取量; 默认 1 MiB
    pub prefix_bytes: u64,
    /// 大文件阈值: ≥ 此大小走「前 large_prefix_bytes 相同 → 待确认」分支; 默认 500 MiB
    pub large_threshold: u64,
    /// 大文件第二前缀读取量; 默认 10 MiB
    pub large_prefix_bytes: u64,
}

impl Default for DedupOptions {
    fn default() -> Self {
        Self {
            prefix_bytes: 1 << 20,
            large_threshold: 500 << 20,
            large_prefix_bytes: 10 << 20,
        }
    }
}

/// 组状态(输出排序优先级 Confirmed < LargePrefixMatched < Candidate)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupGroupState {
    /// 阶段 1 产出, 等待内容比对
    Candidate,
    /// 全量 BLAKE3 一致
    Confirmed,
    /// ≥ large_threshold 且前 large_prefix_bytes 一致, 待全量校验
    LargePrefixMatched,
}

/// 组内一个文件(相对源根路径, '/' 分隔)
#[derive(Debug, Clone)]
pub struct DedupFile {
    pub rel_path: String,
    pub size: u64,
}

/// 候选/结果组; 组内文件按 rel_path 字典序; id 在输出排序后重新分配 1..n
#[derive(Debug, Clone)]
pub struct DedupGroup {
    pub id: u64,
    pub state: DedupGroupState,
    /// 组内每个文件的大小(候选分组保证同大小)
    pub size: u64,
    pub files: Vec<DedupFile>,
    /// (files.len() - 1) × size: 全量去重后理论上可释放的字节
    pub wasted_bytes: u64,
}

/// 阶段 1 报告
#[derive(Debug)]
pub struct DedupScanReport {
    /// 全部 state = Candidate
    pub groups: Vec<DedupGroup>,
    pub warnings: Vec<String>,
    /// 参与统计的文件数(不含被排除 / 符号链接 / 0 字节)
    pub files_scanned: u64,
    /// Σ min(size, prefix_bytes): 「开始查重」前的预计读盘量
    pub estimated_first_pass_bytes: u64,
}

/// 阶段 2/3 报告
#[derive(Debug)]
pub struct DedupCompareReport {
    /// Confirmed / LargePrefixMatched / 未处理 Candidate 的完整组列表(已重编号)
    pub groups: Vec<DedupGroup>,
    pub warnings: Vec<String>,
    pub cancelled: bool,
    /// 已处理完且未产出任何结果组的候选组数
    pub rejected_groups: usize,
}

/// 阶段 4 报告
#[derive(Debug)]
pub struct DedupVerifyReport {
    pub groups: Vec<DedupGroup>,
    pub warnings: Vec<String>,
    pub cancelled: bool,
}

/// 阶段 1: 元数据扫描与候选分组(见模块注释)。取消返回 [`CoreError::Cancelled`],
/// 不返回部分候选。
pub fn scan_candidates(
    input: &Path,
    rules: &ExcludeRules,
    opts: &DedupOptions,
    cancel: &CancelFlag,
    on_progress: &dyn Fn(&Progress),
) -> Result<DedupScanReport> {
    // 取消已置位的扫描不再读盘(旧扫描已被新扫描取代)
    if cancel.is_cancelled() {
        return Err(CoreError::Cancelled);
    }
    if !input.is_dir() {
        return Err(CoreError::InputNotFound(input.to_path_buf()));
    }
    // 宁停不错: 生效模式先校验(UI 入口已校验, 这里兜底库直调场景)
    let patterns = effective_patterns(rules);
    validate_patterns(&patterns).map_err(CoreError::InvalidExclude)?;

    // 第一遍: 只把普通文件 size 推入向量(8 字节/文件), 同时节流上报进度
    let mut sizes: Vec<u64> = Vec::new();
    let mut files_seen = 0u64;
    let mut throttle = TimeThrottle::new();
    let mut warnings = walk_files(input, &patterns, cancel, &mut |rel, size| {
        files_seen += 1;
        sizes.push(size);
        if throttle.due() {
            on_progress(&Progress {
                phase: Phase::DedupScanning,
                done: files_seen as usize,
                total: 0, // 总数未知
                done_bytes: 0,
                total_bytes: 0,
                entry: rel.to_string(),
            });
        }
        Ok(())
    })?;

    // 出现次数 ≥2 的 size 才可能成组
    sizes.sort_unstable();
    let mut candidate_sizes: HashSet<u64> = HashSet::new();
    let mut i = 0;
    while i < sizes.len() {
        let mut j = i;
        while j < sizes.len() && sizes[j] == sizes[i] {
            j += 1;
        }
        if j - i >= 2 {
            candidate_sizes.insert(sizes[i]);
        }
        i = j;
    }
    if candidate_sizes.is_empty() {
        return Ok(DedupScanReport {
            groups: Vec::new(),
            warnings,
            files_scanned: files_seen,
            estimated_first_pass_bytes: 0,
        });
    }

    // 第二遍: 只收集 size 在候选集合中的文件(路径按字典序, 保证组内顺序稳定)
    let mut files: Vec<(String, u64)> = Vec::new();
    let more = walk_files(input, &patterns, cancel, &mut |rel, size| {
        if candidate_sizes.contains(&size) {
            files.push((rel.to_string(), size));
        }
        Ok(())
    })?;
    warnings.extend(more);
    files.sort();

    let groups = group_by_size(files);
    let estimated_first_pass_bytes = groups
        .iter()
        .flat_map(|g| g.files.iter())
        .map(|f| f.size.min(opts.prefix_bytes))
        .sum();
    Ok(DedupScanReport {
        groups,
        warnings,
        files_scanned: files_seen,
        estimated_first_pass_bytes,
    })
}

/// 阶段 2/3: 对每个候选组做前缀比对与哈希(见模块注释)。
/// 只处理 Candidate 组, 其它状态原样保留; 返回完整组列表并重新编号。
pub fn compare_candidates(
    input: &Path,
    groups: Vec<DedupGroup>,
    opts: &DedupOptions,
    cancel: &CancelFlag,
    on_progress: &dyn Fn(&Progress),
) -> Result<DedupCompareReport> {
    if cancel.is_cancelled() {
        return Err(CoreError::Cancelled);
    }
    let total = groups
        .iter()
        .filter(|g| g.state == DedupGroupState::Candidate)
        .count();
    let mut done = 0usize;
    let mut done_bytes = 0u64;
    let mut warnings: Vec<String> = Vec::new();
    let mut cancelled = false;
    let mut rejected = 0usize;
    let mut out: Vec<DedupGroup> = Vec::new();
    let mut buf = vec![0u8; READ_BUF_BYTES];
    let mut throttle = ByteThrottle::new();

    for group in groups {
        if group.state != DedupGroupState::Candidate {
            out.push(group);
            continue;
        }
        // 取消: 当前组退回候选, 未处理组保持候选(组处理原子, 不产生半个组)
        if cancel.is_cancelled() {
            cancelled = true;
            out.push(group);
            continue;
        }
        let mut emit = |n: u64, rel: &str| {
            done_bytes += n;
            if throttle.due(n) {
                on_progress(&Progress {
                    phase: Phase::DedupComparing,
                    done,
                    total,
                    done_bytes,
                    total_bytes: 0,
                    entry: rel.to_string(),
                });
            }
        };
        match compare_one(
            input,
            &group,
            opts,
            cancel,
            &mut buf,
            &mut warnings,
            &mut emit,
        ) {
            None => {
                cancelled = true;
                out.push(group);
            }
            Some(results) => {
                done += 1;
                if results.is_empty() {
                    rejected += 1;
                }
                out.extend(results);
            }
        }
        // 组边界强制发一次进度(UI 据此显示「已完成 X/Y 组」)
        on_progress(&Progress {
            phase: Phase::DedupComparing,
            done,
            total,
            done_bytes,
            total_bytes: 0,
            entry: String::new(),
        });
    }

    sort_and_renumber(&mut out);
    Ok(DedupCompareReport {
        groups: out,
        warnings,
        cancelled,
        rejected_groups: rejected,
    })
}

/// 阶段 4: 仅处理选中的 LargePrefixMatched 组, 全量 BLAKE3 后转 Confirmed 分桶。
/// 按传入 groups 顺序处理选中项, 保证结果确定性。
pub fn verify_large_groups(
    input: &Path,
    groups: Vec<DedupGroup>,
    selected_ids: &[u64],
    cancel: &CancelFlag,
    on_progress: &dyn Fn(&Progress),
) -> Result<DedupVerifyReport> {
    if cancel.is_cancelled() {
        return Err(CoreError::Cancelled);
    }
    let total = groups
        .iter()
        .filter(|g| g.state == DedupGroupState::LargePrefixMatched && selected_ids.contains(&g.id))
        .count();
    let mut done = 0usize;
    let mut done_bytes = 0u64;
    let mut warnings: Vec<String> = Vec::new();
    let mut cancelled = false;
    let mut out: Vec<DedupGroup> = Vec::new();
    let mut buf = vec![0u8; READ_BUF_BYTES];
    let mut throttle = ByteThrottle::new();

    for group in groups {
        let selected =
            group.state == DedupGroupState::LargePrefixMatched && selected_ids.contains(&group.id);
        if !selected {
            out.push(group);
            continue;
        }
        // 取消: 当前组保持待确认, 未处理组保持待确认
        if cancel.is_cancelled() {
            cancelled = true;
            out.push(group);
            continue;
        }
        let mut emit = |n: u64, rel: &str| {
            done_bytes += n;
            if throttle.due(n) {
                on_progress(&Progress {
                    phase: Phase::DedupVerifying,
                    done,
                    total,
                    done_bytes,
                    total_bytes: 0,
                    entry: rel.to_string(),
                });
            }
        };
        match hash_files(
            input,
            group.files.clone(),
            None,
            cancel,
            &mut buf,
            &mut warnings,
            &mut emit,
        ) {
            None => {
                cancelled = true;
                out.push(group);
            }
            Some(hashed) => {
                done += 1;
                for files in hash_buckets(hashed) {
                    out.push(make_group(files, group.size, DedupGroupState::Confirmed));
                }
            }
        }
        // 组边界强制发一次进度
        on_progress(&Progress {
            phase: Phase::DedupVerifying,
            done,
            total,
            done_bytes,
            total_bytes: 0,
            entry: String::new(),
        });
    }

    sort_and_renumber(&mut out);
    Ok(DedupVerifyReport {
        groups: out,
        warnings,
        cancelled,
    })
}

/// 单个候选组的阶段 2/3; 返回 None = 组内取消(中间结果丢弃, 组退回原状态)。
#[allow(clippy::too_many_arguments)]
fn compare_one(
    input: &Path,
    group: &DedupGroup,
    opts: &DedupOptions,
    cancel: &CancelFlag,
    buf: &mut Vec<u8>,
    warnings: &mut Vec<String>,
    on_bytes: &mut dyn FnMut(u64, &str),
) -> Option<Vec<DedupGroup>> {
    let size = group.size;
    let prefix = size.min(opts.prefix_bytes);
    // 阶段 2: 前缀哈希分桶, 只保留 ≥2 个文件的桶
    let matched = hash_buckets(hash_files(
        input,
        group.files.clone(),
        Some(prefix),
        cancel,
        buf,
        warnings,
        on_bytes,
    )?);
    let mut results: Vec<DedupGroup> = Vec::new();
    for part in matched {
        if size <= prefix {
            // 前缀已覆盖全文件 → 直接确认
            results.push(make_group(part, size, DedupGroupState::Confirmed));
        } else if size < opts.large_threshold {
            // 阶段 3: 小文件全量 BLAKE3 → Confirmed 分桶
            let confirmed = hash_buckets(hash_files(
                input, part, None, cancel, buf, warnings, on_bytes,
            )?);
            results.extend(
                confirmed
                    .into_iter()
                    .map(|files| make_group(files, size, DedupGroupState::Confirmed)),
            );
        } else {
            // ≥ large_threshold: 前 Q 字节 BLAKE3 → LargePrefixMatched 分桶
            let q = size.min(opts.large_prefix_bytes);
            let pending = hash_buckets(hash_files(
                input,
                part,
                Some(q),
                cancel,
                buf,
                warnings,
                on_bytes,
            )?);
            results.extend(
                pending
                    .into_iter()
                    .map(|files| make_group(files, size, DedupGroupState::LargePrefixMatched)),
            );
        }
    }
    Some(results)
}

/// 流式读取并计算 BLAKE3; limit = None 表示读到 EOF。
/// 每读取一个 chunk 前检查一次取消旗标; 读取字节数经 on_bytes 上报(实际发送由外层节流)。
/// buf 由调用方复用(固定 1 MiB, 避免每个文件重复分配)。
fn hash_reader(
    path: &Path,
    limit: Option<u64>,
    buf: &mut [u8],
    cancel: &CancelFlag,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<blake3::Hash> {
    let mut file = fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut remaining = limit;
    loop {
        if cancel.is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        let want = match remaining {
            Some(r) => r.min(buf.len() as u64) as usize,
            None => buf.len(),
        };
        let n = file.read(&mut buf[..want])?;
        if n == 0 {
            break; // EOF(文件可能已缩小, 以实际内容为准)
        }
        hasher.update(&buf[..n]);
        on_bytes(n as u64);
        if let Some(r) = remaining.as_mut() {
            *r -= n as u64;
            if *r == 0 {
                break;
            }
        }
    }
    Ok(hasher.finalize())
}

/// 组内一批文件哈希; 读失败记 warning 并移出该文件(不中断整批)。
/// 返回 None = 取消(调用方把当前组退回原状态)。
#[allow(clippy::too_many_arguments)]
fn hash_files(
    input: &Path,
    files: Vec<DedupFile>,
    limit: Option<u64>,
    cancel: &CancelFlag,
    buf: &mut Vec<u8>,
    warnings: &mut Vec<String>,
    on_bytes: &mut dyn FnMut(u64, &str),
) -> Option<Vec<(DedupFile, blake3::Hash)>> {
    let mut out = Vec::with_capacity(files.len());
    for file in files {
        let path = input.join(&file.rel_path);
        let rel = file.rel_path.clone();
        let h = match hash_reader(&path, limit, buf, cancel, &mut |n| on_bytes(n, &rel)) {
            Ok(h) => h,
            Err(CoreError::Cancelled) => return None,
            Err(e) => {
                warnings.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        out.push((file, h));
    }
    Some(out)
}

/// 按哈希分桶, 只保留 ≥2 个文件的桶
fn hash_buckets(hashed: Vec<(DedupFile, blake3::Hash)>) -> Vec<Vec<DedupFile>> {
    let mut buckets: HashMap<blake3::Hash, Vec<DedupFile>> = HashMap::new();
    for (file, h) in hashed {
        buckets.entry(h).or_default().push(file);
    }
    buckets.into_values().filter(|v| v.len() >= 2).collect()
}

fn make_group(files: Vec<DedupFile>, size: u64, state: DedupGroupState) -> DedupGroup {
    let wasted_bytes = (files.len() as u64 - 1) * size;
    DedupGroup {
        id: 0, // 排序后统一分配
        state,
        size,
        files,
        wasted_bytes,
    }
}

/// 输出排序: 状态优先级 Confirmed < LargePrefixMatched < Candidate,
/// 同状态按 wasted_bytes 降序、首路径字典序升序; 排序后 id = 1..n
fn sort_and_renumber(groups: &mut [DedupGroup]) {
    groups.sort_by(|a, b| {
        state_rank(&a.state)
            .cmp(&state_rank(&b.state))
            .then(b.wasted_bytes.cmp(&a.wasted_bytes))
            .then(a.files[0].rel_path.cmp(&b.files[0].rel_path))
    });
    for (i, g) in groups.iter_mut().enumerate() {
        g.id = (i + 1) as u64;
    }
}

fn state_rank(s: &DedupGroupState) -> u8 {
    match s {
        DedupGroupState::Confirmed => 0,
        DedupGroupState::LargePrefixMatched => 1,
        DedupGroupState::Candidate => 2,
    }
}

/// 同大小桶内并查集分组(规则 A 同文件名 ∪ 规则 B 同扩展名, 传递连接);
/// 取连通分量 ≥2 的作为候选组。输入已按 rel_path 排序(HashMap 遍历顺序无妨:
/// 输出前统一排序)。
fn group_by_size(files: Vec<(String, u64)>) -> Vec<DedupGroup> {
    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, (_, size)) in files.iter().enumerate() {
        by_size.entry(*size).or_default().push(i);
    }
    let mut groups: Vec<DedupGroup> = Vec::new();
    for indexes in by_size.into_values() {
        let n = indexes.len();
        if n < 2 {
            continue;
        }
        let mut parent: Vec<usize> = (0..n).collect();
        let mut by_name: HashMap<String, usize> = HashMap::new();
        let mut by_ext: HashMap<String, usize> = HashMap::new();
        for (k, &i) in indexes.iter().enumerate() {
            let rel = &files[i].0;
            // 规则 A: 同完整文件名(小写) —— 传递连接靠并查集(A 同名 B, B 同扩展名 C → 同组)
            let name = file_stem_name(rel);
            match by_name.get(&name) {
                Some(&first) => uf_union(&mut parent, first, k),
                None => {
                    by_name.insert(name, k);
                }
            }
            // 规则 B: 同扩展名(小写; 无扩展名 = 空串)
            let ext = extension_lower(rel);
            match by_ext.get(&ext) {
                Some(&first) => uf_union(&mut parent, first, k),
                None => {
                    by_ext.insert(ext, k);
                }
            }
        }
        let mut components: HashMap<usize, Vec<usize>> = HashMap::new();
        for k in 0..n {
            components
                .entry(uf_find(&mut parent, k))
                .or_default()
                .push(k);
        }
        for members in components.into_values() {
            if members.len() < 2 {
                continue;
            }
            // k 按升序收集 → 组内文件继承 rel_path 字典序
            let group_files: Vec<DedupFile> = members
                .into_iter()
                .map(|k| {
                    let (rel, size) = &files[indexes[k]];
                    DedupFile {
                        rel_path: rel.clone(),
                        size: *size,
                    }
                })
                .collect();
            let size = group_files[0].size;
            groups.push(make_group(group_files, size, DedupGroupState::Candidate));
        }
    }
    sort_and_renumber(&mut groups);
    groups
}

/// 并查集查找(路径减半)
fn uf_find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

fn uf_union(parent: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (uf_find(parent, a), uf_find(parent, b));
    if ra != rb {
        parent[ra] = rb;
    }
}

/// 文件名(含扩展名)小写: 分组规则 A 的 key
fn file_stem_name(rel: &str) -> String {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    base.to_lowercase()
}

/// 扩展名小写(Path::extension 语义); 无扩展名 = 空串
fn extension_lower(rel: &str) -> String {
    Path::new(rel)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// 时间节流(约 100 ms 一次)
struct TimeThrottle {
    last: Instant,
}

impl TimeThrottle {
    fn new() -> Self {
        Self {
            last: Instant::now(),
        }
    }
    fn due(&mut self) -> bool {
        if self.last.elapsed() >= PROGRESS_INTERVAL {
            self.last = Instant::now();
            true
        } else {
            false
        }
    }
}

/// 比对/校验进度节流: 每读约 16 MB 或每 100 ms 发一次(取先到者)
struct ByteThrottle {
    last: Instant,
    since_bytes: u64,
}

impl ByteThrottle {
    fn new() -> Self {
        Self {
            last: Instant::now(),
            since_bytes: 0,
        }
    }
    fn due(&mut self, bytes: u64) -> bool {
        self.since_bytes += bytes;
        if self.since_bytes >= PROGRESS_BYTES_EVERY || self.last.elapsed() >= PROGRESS_INTERVAL {
            self.since_bytes = 0;
            self.last = Instant::now();
            true
        } else {
            false
        }
    }
}

/// 取消检查器: 目录树遍历里按固定粒度轮询旗标(与 scan.rs::ScanCtl 同思路)
struct DedupCtl<'a> {
    cancel: &'a CancelFlag,
    ticks: u32,
}

impl<'a> DedupCtl<'a> {
    fn new(cancel: &'a CancelFlag) -> Self {
        Self { cancel, ticks: 0 }
    }

    /// 条目级检查: 累计到粒度才查一次旗标
    fn tick(&mut self) -> Result<()> {
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks.is_multiple_of(CANCEL_CHECK_EVERY) && self.cancel.is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        Ok(())
    }

    /// 边界级检查(每个目录): 每次必查
    fn check(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            Err(CoreError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// 遍历源目录树, 对每个参与查重的普通文件回调 visit(相对路径, 大小)。
/// - `fs::symlink_metadata`; 文件符号链接跳过; Windows reparse 目录(junction)不下钻
/// - 0 字节文件跳过
/// - 排除坐标系 = 相对源根的路径(与 scan.rs::dir_size 同口径): 命中文件跳过, 命中目录整树不下钻
/// - 读目录 / stat 失败记 warning, 跳过该子树, 不中断
/// - 取消: 目录边界必查, 条目每 CANCEL_CHECK_EVERY 查一次; visit 返回 Err 直接上抛
fn walk_files(
    input: &Path,
    patterns: &[String],
    cancel: &CancelFlag,
    visit: &mut dyn FnMut(&str, u64) -> Result<()>,
) -> Result<Vec<String>> {
    let mut warnings: Vec<String> = Vec::new();
    let mut ctl = DedupCtl::new(cancel);
    // (目录, 相对源根前缀); 显式栈避免深递归
    let mut stack = vec![(input.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        ctl.check()?;
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) => {
                warnings.push(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        for entry in rd {
            ctl.tick()?;
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warnings.push(format!("{}: {e}", dir.display()));
                    continue;
                }
            };
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let md = match fs::symlink_metadata(&path) {
                Ok(md) => md,
                Err(e) => {
                    warnings.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if matches_patterns(patterns, &rel).is_some() {
                continue; // 文件跳过 / 目录不入栈(整树剔除)
            }
            let ft = md.file_type();
            if ft.is_dir() {
                if is_reparse_dir(&md) {
                    continue; // junction / 目录链接: 不跟随, 避免循环
                }
                stack.push((path, rel));
                continue;
            }
            if ft.is_symlink() || md.len() == 0 {
                continue; // 链接不参与(避免链接与目标算重复); 0 字节不占空间
            }
            visit(&rel, md.len())?;
        }
    }
    Ok(warnings)
}

/// Windows 目录联接 / 符号目录带 reparse 属性, 不下钻
#[cfg(windows)]
fn is_reparse_dir(md: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_dir(_md: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::unique_temp;
    use std::path::Path;

    /// 小阈值覆盖大文件分支: 前缀 64 B / 大文件阈值 1 KB / 大文件第二前缀 256 B
    fn small_opts() -> DedupOptions {
        DedupOptions {
            prefix_bytes: 64,
            large_threshold: 1024,
            large_prefix_bytes: 256,
        }
    }

    fn write(root: &Path, rel: &str, content: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn scan(root: &Path) -> DedupScanReport {
        scan_candidates(
            root,
            &ExcludeRules::default(),
            &small_opts(),
            &CancelFlag::new(),
            &|_| {},
        )
        .unwrap()
    }

    fn compare(root: &Path, groups: Vec<DedupGroup>) -> DedupCompareReport {
        compare_candidates(root, groups, &small_opts(), &CancelFlag::new(), &|_| {}).unwrap()
    }

    fn verify(root: &Path, groups: Vec<DedupGroup>, ids: &[u64]) -> DedupVerifyReport {
        verify_large_groups(root, groups, ids, &CancelFlag::new(), &|_| {}).unwrap()
    }

    fn rels(g: &DedupGroup) -> Vec<String> {
        g.files.iter().map(|f| f.rel_path.clone()).collect()
    }

    // ---------- 阶段 1: 候选分组 ----------

    #[test]
    fn same_name_same_size_are_candidates() {
        let root = unique_temp("dedup-name-size");
        write(&root, "a/a.txt", b"hello");
        write(&root, "b/b.txt", b"hello");
        write(&root, "lone.txt", b"other!"); // 6 字节, 不与 5 字节组撞大小
        let rep = scan(&root);
        assert_eq!(rep.groups.len(), 1, "{rep:?}");
        let g = &rep.groups[0];
        assert_eq!(g.state, DedupGroupState::Candidate);
        assert_eq!(g.size, 5);
        assert_eq!(g.wasted_bytes, 5);
        assert_eq!(
            rels(g),
            vec!["a/a.txt".to_string(), "b/b.txt".to_string()],
            "组内按 rel_path 字典序"
        );
        assert_eq!(g.id, 1, "输出重编号 1..n");
        assert_eq!(rep.files_scanned, 3);
        assert_eq!(
            rep.estimated_first_pass_bytes,
            2 * 5.min(64),
            "Σ min(size, prefix)"
        );
    }

    #[test]
    fn same_ext_same_size_are_candidates() {
        let root = unique_temp("dedup-ext-size");
        write(&root, "x.bin", [0; 10].as_slice());
        write(&root, "y.bin", [1; 10].as_slice());
        let rep = scan(&root);
        assert_eq!(rep.groups.len(), 1);
        assert_eq!(rels(&rep.groups[0]), vec!["x.bin", "y.bin"]);
    }

    #[test]
    fn same_name_diff_size_not_candidate() {
        let root = unique_temp("dedup-name-diff-size");
        write(&root, "a/a.txt", b"12345");
        write(&root, "b/a.txt", b"123456");
        let rep = scan(&root);
        assert!(rep.groups.is_empty(), "{rep:?}");
    }

    #[test]
    fn same_size_diff_name_diff_ext_not_candidate() {
        let root = unique_temp("dedup-no-rule");
        write(&root, "a.aaa", [0; 8].as_slice());
        write(&root, "b.bbb", [0; 8].as_slice());
        let rep = scan(&root);
        assert!(rep.groups.is_empty(), "并集规则之外不候选");
    }

    #[test]
    fn no_extension_same_size_are_candidates() {
        let root = unique_temp("dedup-no-ext");
        write(&root, "aa", [0; 6].as_slice());
        write(&root, "bb", [0; 6].as_slice());
        let rep = scan(&root);
        assert_eq!(rep.groups.len(), 1, "无扩展名视为空串, 命中规则 B");
    }

    #[test]
    fn name_and_ext_case_insensitive() {
        let root = unique_temp("dedup-case");
        write(&root, "p/Photo.JPG", b"abc");
        write(&root, "q/photo.jpg", b"abc");
        write(&root, "m/x.dat", b"zzz");
        write(&root, "n/y.DAT", b"yyy");
        let rep = scan(&root);
        assert_eq!(rep.groups.len(), 2, "大小写只影响候选不影响正确性: {rep:?}");
    }

    #[test]
    fn zero_byte_files_skipped() {
        let root = unique_temp("dedup-zero");
        write(&root, "a/empty.txt", b"");
        write(&root, "b/empty.txt", b"");
        let rep = scan(&root);
        assert!(rep.groups.is_empty());
        assert_eq!(rep.files_scanned, 0);
    }

    #[test]
    fn single_file_not_candidate() {
        let root = unique_temp("dedup-single");
        write(&root, "only.txt", b"solo");
        let rep = scan(&root);
        assert!(rep.groups.is_empty());
    }

    #[test]
    fn excluded_file_and_dir_skipped() {
        let root = unique_temp("dedup-exclude");
        write(&root, "keep/a.bin", [0; 4].as_slice());
        write(&root, "keep/b.bin", [0; 4].as_slice());
        write(&root, "skip.log", [0; 4].as_slice());
        write(&root, "skip2.log", [0; 4].as_slice());
        write(&root, "cache/c.bin", [0; 4].as_slice());
        write(&root, "cache/d.bin", [0; 4].as_slice());
        let rules = ExcludeRules {
            include_hidden: true,
            patterns: vec!["skip.log".into(), "skip2.log".into(), "cache/**".into()],
        };
        let rep =
            scan_candidates(&root, &rules, &small_opts(), &CancelFlag::new(), &|_| {}).unwrap();
        assert_eq!(rep.groups.len(), 1, "{rep:?}");
        assert_eq!(rels(&rep.groups[0]), vec!["keep/a.bin", "keep/b.bin"]);
        assert_eq!(rep.files_scanned, 2);
    }

    #[test]
    fn os_metadata_default_excluded() {
        // 默认策略含系统元数据名单: Thumbs.db 不参与
        let root = unique_temp("dedup-os-meta");
        write(&root, "a/Thumbs.db", [0; 4].as_slice());
        write(&root, "b/Thumbs.db", [0; 4].as_slice());
        let rep = scan(&root);
        assert!(rep.groups.is_empty());
    }

    #[test]
    fn symlink_not_participating() {
        let root = unique_temp("dedup-symlink");
        write(&root, "real.txt", b"target");
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("real.txt"), root.join("real.txt.lnk")).unwrap();
        let rep = scan(&root);
        assert!(rep.groups.is_empty(), "链接与其目标不算重复: {rep:?}");
    }

    #[test]
    fn transitive_connection_one_group() {
        // A 与 B 同文件名, B 与 C 同扩展名 → 三者同组(并查集传递)
        let root = unique_temp("dedup-transitive");
        write(&root, "x/rep.bin", [0; 9].as_slice());
        write(&root, "y/rep.bin", [0; 9].as_slice());
        write(&root, "z/other.bin", [0; 9].as_slice());
        let rep = scan(&root);
        assert_eq!(rep.groups.len(), 1, "{rep:?}");
        assert_eq!(rep.groups[0].files.len(), 3);
        assert_eq!(rep.groups[0].wasted_bytes, 18);
    }

    #[test]
    fn scan_rejects_missing_dir() {
        let root = unique_temp("dedup-missing");
        let err = scan_candidates(
            &root.join("nope"),
            &ExcludeRules::default(),
            &small_opts(),
            &CancelFlag::new(),
            &|_| {},
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::InputNotFound(_)));
    }

    // ---------- 阶段 2/3: 前缀比对与哈希 ----------

    #[test]
    fn same_prefix_diff_tail_rejected() {
        // <500 MB(阈值 1 KB)分支: 全量哈希裁决, 前 64 B 相同尾部不同 → 淘汰
        let root = unique_temp("dedup-prefix-tail");
        let mut a = vec![0u8; 200];
        a[64..].fill(1);
        let mut b = vec![0u8; 200];
        b[64..].fill(2);
        write(&root, "a.bin", &a);
        write(&root, "b.bin", &b);
        let scan = scan(&root);
        assert_eq!(scan.groups.len(), 1);
        let rep = compare(&root, scan.groups);
        assert!(!rep.cancelled);
        assert_eq!(rep.rejected_groups, 1);
        assert!(rep.groups.is_empty());
    }

    #[test]
    fn small_file_within_prefix_confirmed_directly() {
        // 32 B ≤ 前缀 64 B: 阶段 2 即确认
        let root = unique_temp("dedup-small-confirm");
        write(&root, "a.txt", b"identical-content-32-bytes!!!!");
        write(&root, "sub/a.txt", b"identical-content-32-bytes!!!!");
        let scan = scan(&root);
        assert_eq!(scan.groups.len(), 1);
        let rep = compare(&root, scan.groups);
        assert_eq!(rep.groups.len(), 1);
        assert_eq!(rep.groups[0].state, DedupGroupState::Confirmed);
        assert_eq!(rep.rejected_groups, 0);
    }

    #[test]
    fn small_file_content_diff_rejected() {
        let root = unique_temp("dedup-small-diff");
        write(&root, "a.txt", b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        write(&root, "b.txt", b"BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB");
        let scan = scan(&root);
        let rep = compare(&root, scan.groups);
        assert!(rep.groups.is_empty());
        assert_eq!(rep.rejected_groups, 1);
    }

    #[test]
    fn large_prefix_matched_then_verify_confirmed() {
        // ≥1 KB: 前 256 B 相同 → LargePrefixMatched; 全量相同 → 阶段 4 转 Confirmed
        let root = unique_temp("dedup-large-confirm");
        let content: Vec<u8> = (0..2000).map(|i| (i % 251) as u8).collect();
        write(&root, "v/a.mkv", &content);
        write(&root, "v/b.mkv", &content);
        let scan = scan(&root);
        let rep = compare(&root, scan.groups);
        assert_eq!(rep.groups.len(), 1);
        assert_eq!(rep.groups[0].state, DedupGroupState::LargePrefixMatched);
        assert_eq!(rep.groups[0].id, 1);

        let rep2 = verify(&root, rep.groups, &[1]);
        assert!(!rep2.cancelled);
        assert_eq!(rep2.groups.len(), 1);
        assert_eq!(rep2.groups[0].state, DedupGroupState::Confirmed);
        assert_eq!(rep2.groups[0].id, 1, "重编号从 1 开始");
    }

    #[test]
    fn large_verify_diff_tail_dropped() {
        // 前 256 B 相同但尾部不同 → 阶段 4 全量哈希淘汰
        let root = unique_temp("dedup-large-tail");
        let mut a = vec![7u8; 2000];
        a[256..].fill(1);
        let mut b = vec![7u8; 2000];
        b[256..].fill(2);
        write(&root, "a.mkv", &a);
        write(&root, "b.mkv", &b);
        let scan = scan(&root);
        let rep = compare(&root, scan.groups);
        assert_eq!(rep.groups.len(), 1);
        assert_eq!(rep.groups[0].state, DedupGroupState::LargePrefixMatched);
        let pending_id = rep.groups[0].id;
        let rep2 = verify(&root, rep.groups, &[pending_id]);
        assert!(rep2.groups.is_empty(), "全量不同 → 组移除");
    }

    #[test]
    fn large_prefix_diff_rejected_at_compare() {
        // ≥阈值且前 256 B 内不同 → 阶段 2/3 直接淘汰, 不进待确认
        let root = unique_temp("dedup-large-prefix-diff");
        let mut a = vec![0u8; 2000];
        a[0] = 1;
        let mut b = vec![0u8; 2000];
        b[0] = 2;
        write(&root, "a.mkv", &a);
        write(&root, "b.mkv", &b);
        let scan = scan(&root);
        let rep = compare(&root, scan.groups);
        assert!(rep.groups.is_empty());
        assert_eq!(rep.rejected_groups, 1);
    }

    #[test]
    fn candidate_group_splits_into_two_confirmed() {
        // 4 个 2000 B 文件: 前 256 B 全同(一个待确认组), 尾部 2+2 两组不同 → 阶段 4 分裂为 2 个确认组
        let root = unique_temp("dedup-split");
        let mut c1 = vec![0u8; 2000];
        for (i, b) in c1[256..].iter_mut().enumerate() {
            *b = (i % 7) as u8;
        }
        let mut c2 = vec![0u8; 2000];
        for (i, b) in c2[256..].iter_mut().enumerate() {
            *b = (i % 11) as u8;
        }
        write(&root, "1.bin", &c1);
        write(&root, "2.bin", &c1);
        write(&root, "3.bin", &c2);
        write(&root, "4.bin", &c2);
        let scan = scan(&root);
        let rep = compare(&root, scan.groups);
        assert_eq!(rep.groups.len(), 1, "同前缀先并为一组待确认: {rep:?}");
        assert_eq!(rep.groups[0].files.len(), 4);
        let rep2 = verify(&root, rep.groups, &[1]);
        assert_eq!(rep2.groups.len(), 2, "{rep2:?}");
        assert!(
            rep2.groups
                .iter()
                .all(|g| g.state == DedupGroupState::Confirmed && g.files.len() == 2)
        );
    }

    // ---------- 取消与读失败 ----------

    #[test]
    fn cancelled_scan_returns_error() {
        let root = unique_temp("dedup-cancel-scan");
        write(&root, "a.txt", b"x");
        let flag = CancelFlag::new();
        flag.cancel();
        let err = scan_candidates(
            &root,
            &ExcludeRules::default(),
            &small_opts(),
            &flag,
            &|_| {},
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::Cancelled));
    }

    #[test]
    fn compare_cancel_keeps_done_and_pending_candidate() {
        // 组 1 处理完成后取消(组边界回调触发): 已完成组保留结论, 未处理组仍为候选
        let root = unique_temp("dedup-cancel-compare");
        write(&root, "g1/a.txt", b"same");
        write(&root, "g1/b.txt", b"same");
        write(&root, "g2/a.txt", b"other");
        write(&root, "g2/b.txt", b"other");
        let scan = scan(&root);
        assert_eq!(scan.groups.len(), 2);
        let flag = CancelFlag::new();
        let rep = compare_candidates(&root, scan.groups, &small_opts(), &flag, &|p: &Progress| {
            if p.phase == Phase::DedupComparing && p.done >= 1 {
                flag.cancel();
            }
        })
        .unwrap();
        assert!(rep.cancelled);
        assert_eq!(rep.groups.len(), 2, "{rep:?}");
        assert_eq!(
            rep.groups[0].state,
            DedupGroupState::Confirmed,
            "已完成组保留结论"
        );
        assert_eq!(
            rep.groups[1].state,
            DedupGroupState::Candidate,
            "未处理组仍候选"
        );
    }

    #[test]
    fn verify_cancel_keeps_remaining_pending() {
        // 组 1 全量校验完成后取消: 组 1 → Confirmed, 组 2 保持 LargePrefixMatched
        let root = unique_temp("dedup-cancel-verify");
        let content = vec![9u8; 2000];
        write(&root, "p1/a", &content);
        write(&root, "p1/b", &content);
        let bigger = vec![9u8; 2010];
        write(&root, "p2/a", &bigger);
        write(&root, "p2/b", &bigger);
        let scan = scan(&root);
        let rep = compare(&root, scan.groups);
        assert_eq!(rep.groups.len(), 2, "两个大文件组(不同大小区分): {rep:?}");
        assert_eq!(rep.groups[0].id, 1);
        assert_eq!(rep.groups[1].id, 2);
        let flag = CancelFlag::new();
        let rep2 = verify_large_groups(&root, rep.groups, &[1, 2], &flag, &|p: &Progress| {
            if p.phase == Phase::DedupVerifying && p.done >= 1 {
                flag.cancel();
            }
        })
        .unwrap();
        assert!(rep2.cancelled);
        assert_eq!(rep2.groups[0].state, DedupGroupState::Confirmed);
        assert_eq!(rep2.groups[1].state, DedupGroupState::LargePrefixMatched);
    }

    #[test]
    fn missing_file_warns_and_group_drops() {
        let root = unique_temp("dedup-missing-file");
        write(&root, "a.bin", [0; 4].as_slice());
        write(&root, "b.bin", [0; 4].as_slice());
        let scan = scan(&root);
        fs::remove_file(root.join("b.bin")).unwrap();
        let rep = compare(&root, scan.groups);
        assert!(rep.groups.is_empty(), "组不足 2 个文件 → 整组移除");
        assert_eq!(rep.rejected_groups, 1);
        assert!(!rep.warnings.is_empty(), "读失败记 warning");
    }

    // ---------- 排序 / 确定性 / 进度 ----------

    #[test]
    fn output_sorted_by_state_then_wasted_then_path() {
        let root = unique_temp("dedup-sort");
        let c: Vec<u8> = (0..2000).map(|i| (i % 13) as u8).collect();
        write(&root, "big1", &c);
        write(&root, "big2", &c);
        write(&root, "mid1", b"this is exactly 25 bytes!");
        write(&root, "mid2", b"this is exactly 25 bytes!");
        write(&root, "s1", b"12345678");
        write(&root, "s2", b"12345678");
        let scan = scan(&root);
        assert_eq!(scan.groups.len(), 3, "{scan:?}");
        let rep = compare(&root, scan.groups);
        let states: Vec<(DedupGroupState, u64, String)> = rep
            .groups
            .iter()
            .map(|g| (g.state, g.wasted_bytes, g.files[0].rel_path.clone()))
            .collect();
        assert_eq!(
            states,
            vec![
                (DedupGroupState::Confirmed, 25, "mid1".into()),
                (DedupGroupState::Confirmed, 8, "s1".into()),
                (DedupGroupState::LargePrefixMatched, 2000, "big1".into()),
            ],
            "Confirmed < LargePrefixMatched; 同状态 wasted 降序"
        );
        for (i, g) in rep.groups.iter().enumerate() {
            assert_eq!(g.id, (i + 1) as u64);
        }
    }

    #[test]
    fn scan_is_deterministic() {
        let root = unique_temp("dedup-deterministic");
        write(&root, "a/x.bin", [0; 10].as_slice());
        write(&root, "b/x.bin", [0; 10].as_slice());
        write(&root, "y.bin", [1; 10].as_slice());
        write(&root, "z.bin", [2; 10].as_slice());
        let r1 = scan(&root);
        let r2 = scan(&root);
        let summary = |r: &DedupScanReport| -> Vec<(u64, u64, u64, Vec<String>)> {
            r.groups
                .iter()
                .map(|g| (g.id, g.size, g.wasted_bytes, rels(g)))
                .collect()
        };
        assert_eq!(summary(&r1), summary(&r2), "两次扫描结果一致");
        // 3 个同大小文件按规则两两连接 → 单组 3 文件
        assert_eq!(r1.groups.len(), 1);
        assert_eq!(
            r1.groups[0].files.len(),
            4,
            "y.bin/z.bin 同扩展名也并入: {r1:?}"
        );
    }

    #[test]
    fn compare_emits_boundary_progress() {
        let root = unique_temp("dedup-progress");
        write(&root, "a.txt", b"same");
        write(&root, "b.txt", b"same");
        write(&root, "c.txt", b"same2");
        write(&root, "d.txt", b"same2");
        let scan = scan(&root);
        let events: std::sync::Mutex<Vec<Progress>> = std::sync::Mutex::new(Vec::new());
        let rep = compare_candidates(
            &root,
            scan.groups,
            &small_opts(),
            &CancelFlag::new(),
            &|p: &Progress| events.lock().unwrap().push(p.clone()),
        );
        assert!(!rep.unwrap().cancelled);
        let ev = events.into_inner().unwrap();
        let boundary: Vec<&Progress> = ev
            .iter()
            .filter(|p| p.done > 0 && p.entry.is_empty())
            .collect();
        assert_eq!(boundary.len(), 2, "每处理完一组强制发一次: {ev:?}");
        assert_eq!(boundary[0].done, 1);
        assert_eq!(boundary[1].done, 2);
        assert!(ev.iter().all(|p| p.phase == Phase::DedupComparing));
    }

    #[test]
    fn verify_rejects_unselected_or_wrong_state_ids() {
        // 未选中的待确认组原样保留; 无关 ID 无效果
        let root = unique_temp("dedup-verify-select");
        let content = vec![5u8; 2000];
        write(&root, "a.mkv", &content);
        write(&root, "b.mkv", &content);
        let scan = scan(&root);
        let rep = compare(&root, scan.groups);
        let pending_id = rep.groups[0].id;
        let rep2 = verify(&root, rep.groups, &[999]); // 无关 ID
        assert_eq!(rep2.groups.len(), 1);
        assert_eq!(rep2.groups[0].state, DedupGroupState::LargePrefixMatched);
        assert_eq!(rep2.groups[0].id, pending_id, "重编号后仍是 1");
    }
}
