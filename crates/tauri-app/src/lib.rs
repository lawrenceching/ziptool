//! Tauri 外壳:把 `zip-tool-core` 的能力通过 `#[tauri::command]` 暴露给前端。
//! 交互规格见 _docs/ui-backend.md, 命令面:
//! - `scan`: 只扫描不执行, 结果缓存在外壳([`ScanContext`] 保鲜)
//! - `plan_packages` / `compress` / `extract_archive`: 打包方案与按名单执行
//! - `validate_dir`: 拖拽/选择路径校验(必须是已存在的文件夹, 不改变 core 预检职责)
//! - `cancel_current`: 取消当前任务(立即 kill 在跑的后端进程并清理半成品, 停止后续)
//! - `load_settings` / `save_settings`: `settings.json` 读写(见 _docs/filtering_rules.md)
//!
//! 命令在 `spawn_blocking` 中调 core, 进度经 `tauri::ipc::Channel` 推给前端;
//! scan/compress/extract 单任务互斥, 任务结束释放忙闲标志并清空取消旗标。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri::State;
use tauri::ipc::Channel;

mod settings;

use tauri::AppHandle;

use zip_tool_core::strategy::{self, PackageKind, StrategyItem, StrategyParams};
use zip_tool_core::{
    CancelFlag, Entry, EntryKind, ExcludeReason, ExcludeRules, ExtractGroup, ExtractScan, Format,
    Options, Phase, Progress, ScanReport,
};

use settings::SettingsLoadResult;

const POISONED: &str = "state poisoned";
const EXPIRED: &str = "扫描结果已过期, 请重新选择源文件夹";

/// 全局状态:当前任务取消旗标 + 任务互斥 + 扫描结果缓存
#[derive(Default)]
struct AppState {
    /// 运行中任务(压缩/解压)的取消旗标; 任务结束清空
    current: Mutex<Option<CancelFlag>>,
    /// 任务互斥(scan/compress/extract 互斥): Some(kind) = 忙
    busy: Mutex<Option<&'static str>>,
    /// 最近一次 scan 的结果(保鲜: 确认时校验 mode + src, 名单严格匹配)
    scan: Mutex<Option<ScanContext>>,
    /// 扫描代数: 每次成功 scan 递增, 用于 planId 失效旧方案
    scan_generation: AtomicU64,
}

/// scan 命令缓存的扫描结果(mode + src + 规则匹配才可用)
struct ScanContext {
    mode: &'static str,
    src: PathBuf,
    /// 扫描时的排除规则(compress 模式;确认时比对, 不一致 = 过期)
    rules: ExcludeRules,
    /// 成功扫描的代数; planId 的一部分
    generation: u64,
    data: ScanData,
}

enum ScanData {
    Compress(ScanReport),
    Extract(ExtractScan),
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent {
    /// "scanning" / "compressing" / "extracting"
    phase: String,
    done: usize,
    total: usize,
    done_bytes: u64,
    total_bytes: u64,
    entry: String,
}

impl From<&Progress> for ProgressEvent {
    fn from(p: &Progress) -> Self {
        Self {
            phase: match p.phase {
                Phase::Scanning => "scanning".into(),
                Phase::Compressing => "compressing".into(),
                Phase::Extracting => "extracting".into(),
            },
            done: p.done,
            total: p.total,
            done_bytes: p.done_bytes,
            total_bytes: p.total_bytes,
            entry: p.entry.clone(),
        }
    }
}

/// scan 返回的一个表格行(状态语义: pending 可勾选 / meta 元数据 / warning 告警)
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanItem {
    name: String,
    /// "file" | "dir"
    kind: String,
    size: u64,
    status: String,
    /// extract: mapping.csv 原名; compress: None
    original: Option<String>,
    /// warning 原因 / meta 附注; 展示文案由前端决定
    detail: Option<String>,
    /// compress: 排除原因(「隐藏文件」/「匹配排除模式: <p>」);行显示但默认不勾选
    excluded: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanResult {
    mode: String,
    items: Vec<ScanItem>,
    /// compress 子树级告警(状态栏汇总); extract 的告警走 items(warning 行)
    warnings: Vec<String>,
    total_bytes: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompressOptions {
    /// "2g"/"500m"/字节数; None = 不分包(旧路径兼容字段)
    split_size: Option<String>,
    /// 空串按 None(不加密)
    password: Option<String>,
    random_name: bool,
    /// "zip" | "7z"; None = zip(最大兼容)
    format: Option<String>,
    /// 0-9; None = 后端默认
    level: Option<u8>,
    /// 排除模式(高级区输入);None = 无用户模式
    excludes: Option<Vec<String>>,
    /// 排除策略; None = 默认策略(两种都开)
    filters: Option<settings::FilterSettingsDto>,
    /// 压缩成功后结构校验;缺省 true(旧客户端不带该字段也保持默认开启)
    #[serde(default = "default_true")]
    verify: bool,
    /// 打包策略; None = 旧逐条目路径(保持兼容)
    packaging: Option<PackagingOptions>,
}

/// 打包策略参数。空字符串/空值 = 关闭对应维度。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackagingOptions {
    /// "100m"/字节; None = 不合并
    merge_below: Option<String>,
    /// "2g"/字节; None = 不分包
    split_above: Option<String>,
    /// "2g"/字节; None = 跟随 split_above
    bundle_max: Option<String>,
    /// 单个合并包条目上限; None = 不限制
    max_entries: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EffectivePackagingDto {
    merge_below: Option<u64>,
    split_above: Option<u64>,
    bundle_max: Option<u64>,
    max_entries: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanPackageDto {
    id: String,
    /// "direct" | "split" | "bundle"
    kind: String,
    /// 指向 scan items 的行下标
    member_indexes: Vec<usize>,
    input_bytes: u64,
    entry_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanSummaryDto {
    total_packages: usize,
    direct: usize,
    split: usize,
    bundle: usize,
    selected_bytes: u64,
    max_direct_or_bundle_bytes: u64,
    max_bundle_entries: usize,
    effective: EffectivePackagingDto,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanWarningDto {
    /// "tooManyEntries"
    code: String,
    member_index: usize,
    entry_count: usize,
    limit: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanResult {
    plan_id: String,
    packages: Vec<PlanPackageDto>,
    summary: PlanSummaryDto,
    warnings: Vec<PlanWarningDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExtractOptionsDto {
    /// 空串按 None(不加密)
    password: Option<String>,
    force: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveInfo {
    archive: String,
    original: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CompressedPackageInfo {
    id: String,
    /// "direct" | "split" | "bundle"
    kind: String,
    archive: String,
    members: Vec<String>,
    input_bytes: u64,
    entry_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CompressResult {
    cancelled: bool,
    /// 旧客户端字段; 打包路径为空, 新 UI 用 packages
    entries: Vec<ArchiveInfo>,
    packages: Vec<CompressedPackageInfo>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExtractedGroupInfo {
    entry: String,
    original: Option<String>,
    files: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExtractResult {
    cancelled: bool,
    groups: Vec<ExtractedGroupInfo>,
    warnings: Vec<String>,
}

/// 任务互斥: 已有任务在跑则拒绝, 否则占住 kind(3.5 忙闲守卫)
fn acquire_busy(busy: &Mutex<Option<&'static str>>, kind: &'static str) -> Result<(), String> {
    let mut b = busy.lock().map_err(|_| POISONED.to_string())?;
    if b.is_some() {
        return Err("已有任务在运行".into());
    }
    *b = Some(kind);
    Ok(())
}

fn release_busy(busy: &Mutex<Option<&'static str>>) {
    if let Ok(mut b) = busy.lock() {
        *b = None;
    }
}

/// 任务收尾: 释放忙闲标志并清空取消旗标(成功/失败/取消都要走)
fn finish_job(state: &AppState) {
    release_busy(&state.busy);
    if let Ok(mut c) = state.current.lock() {
        *c = None;
    }
}

/// 空串(或纯空白)按 None 处理
fn non_empty(p: Option<String>) -> Option<String> {
    p.filter(|s| !s.trim().is_empty())
}

/// serde 缺省值:压缩后校验默认开启
fn default_true() -> bool {
    true
}

/// UI DTO → core Options;format/level 的取值校验在这里(宁停不错)
fn to_core_options(o: CompressOptions) -> Result<Options, String> {
    let format = match o.format.as_deref() {
        None | Some("zip") => Format::Zip,
        Some("7z") => Format::SevenZ,
        Some(other) => return Err(format!("未知压缩格式: {other}")),
    };
    let level = match o.level {
        None => None,
        Some(n) if n <= 9 => Some(n),
        Some(n) => return Err(format!("压缩级别超出 0-9: {n}")),
    };
    let split_size = match o.split_size.as_deref().map(zip_tool_core::parse_size) {
        Some(Ok(v)) => Some(v),
        Some(Err(e)) => return Err(e),
        None => None,
    };
    // 策略 + 用户模式 → 既有 ExcludeRules; 非法模式在此拒绝(与 CLI exit 2 同语义)
    let filters = o.filters.unwrap_or_default();
    let rules = zip_tool_core::effective_rules(&filters.to_core(), &o.excludes.unwrap_or_default())
        .map_err(|e| format!("排除模式无效: {e}"))?;
    Ok(Options {
        split_size,
        password: non_empty(o.password),
        random_name: o.random_name,
        format,
        level,
        exclude: rules,
        verify: o.verify,
    })
}

fn kind_str(k: EntryKind) -> &'static str {
    match k {
        EntryKind::Dir => "dir",
        EntryKind::File => "file",
    }
}

fn package_kind_str(kind: PackageKind) -> &'static str {
    match kind {
        PackageKind::Direct => "direct",
        PackageKind::Split => "split",
        PackageKind::Bundle => "bundle",
    }
}

/// 打包阈值文本 → 字节; 空串/None = 关闭该维度
fn parse_packaging_size(value: &Option<String>, field: &str) -> Result<Option<u64>, String> {
    match value.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(s) => zip_tool_core::parse_size(s)
            .map(Some)
            .map_err(|e| format!("{field}: {e}")),
    }
}

/// UI packaging DTO → strategy 参数(含自洽性校验)
fn to_strategy_params(o: &PackagingOptions) -> Result<StrategyParams, String> {
    let params = StrategyParams {
        merge_below: parse_packaging_size(&o.merge_below, "合并阈值")?,
        split_above: parse_packaging_size(&o.split_above, "分包阈值")?,
        bundle_max: parse_packaging_size(&o.bundle_max, "合并包上限")?,
        max_entries: o.max_entries,
    };
    params.validate().map_err(|e| e.to_string())?;
    Ok(params)
}

/// planId = 扫描代数 + 勾选名单(扫描序) + 生效的 packaging 参数。
/// 新扫描、改勾选、改阈值都会变; 格式/密码/目标目录不影响。
fn plan_signature(generation: u64, names: &[&str], params: &StrategyParams) -> String {
    let mut hasher = DefaultHasher::new();
    generation.hash(&mut hasher);
    names.len().hash(&mut hasher);
    for name in names {
        name.hash(&mut hasher);
    }
    params.merge_below.hash(&mut hasher);
    params.split_above.hash(&mut hasher);
    params.effective_bundle_max().hash(&mut hasher);
    params.max_entries.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// 构建 UI 的可审核方案。`rows` 是 (scan 行下标, Entry), 保持扫描序。
fn build_plan_result(
    generation: u64,
    rows: &[(usize, Entry)],
    params: &StrategyParams,
) -> Result<PlanResult, String> {
    let items: Vec<StrategyItem> = rows
        .iter()
        .map(|(_, entry)| StrategyItem::new(entry.name.clone(), entry.size, entry.entry_count))
        .collect();
    let strategy_plan = strategy::plan(&items, params).map_err(|e| e.to_string())?;

    let name_to_index: std::collections::HashMap<&str, usize> = rows
        .iter()
        .map(|(index, entry)| (entry.name.as_str(), *index))
        .collect();

    let mut packages = Vec::with_capacity(strategy_plan.packages.len());
    for (index, package) in strategy_plan.packages.iter().enumerate() {
        let member_indexes = package
            .members
            .iter()
            .map(|name| {
                name_to_index
                    .get(name.as_str())
                    .copied()
                    .ok_or_else(|| "内部错误: 方案成员不在扫描结果中".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        packages.push(PlanPackageDto {
            id: format!("p{index}"),
            kind: package_kind_str(package.kind).into(),
            member_indexes,
            input_bytes: package.input_bytes,
            entry_count: package.entry_count,
        });
    }

    let selected_bytes: u64 = rows.iter().map(|(_, entry)| entry.size).sum();
    let max_direct_or_bundle_bytes = packages
        .iter()
        .filter(|package| package.kind != "split")
        .map(|package| package.input_bytes)
        .max()
        .unwrap_or(0);
    let max_bundle_entries = packages
        .iter()
        .filter(|package| package.kind == "bundle")
        .map(|package| package.entry_count)
        .max()
        .unwrap_or(0);
    let warnings = strategy_plan
        .warnings
        .iter()
        .filter_map(|warning| {
            let index = *name_to_index.get(warning.item.as_str())?;
            match &warning.reason {
                strategy::WarningReason::TooManyEntries { entry_count, limit } => {
                    Some(PlanWarningDto {
                        code: "tooManyEntries".into(),
                        member_index: index,
                        entry_count: *entry_count,
                        limit: *limit,
                    })
                }
            }
        })
        .collect();

    let names: Vec<&str> = rows.iter().map(|(_, entry)| entry.name.as_str()).collect();
    Ok(PlanResult {
        plan_id: plan_signature(generation, &names, params),
        packages,
        summary: PlanSummaryDto {
            total_packages: strategy_plan.packages.len(),
            direct: strategy_plan
                .packages
                .iter()
                .filter(|package| package.kind == PackageKind::Direct)
                .count(),
            split: strategy_plan
                .packages
                .iter()
                .filter(|package| package.kind == PackageKind::Split)
                .count(),
            bundle: strategy_plan
                .packages
                .iter()
                .filter(|package| package.kind == PackageKind::Bundle)
                .count(),
            selected_bytes,
            max_direct_or_bundle_bytes,
            max_bundle_entries,
            effective: EffectivePackagingDto {
                merge_below: params.merge_below,
                split_above: params.split_above,
                bundle_max: params.effective_bundle_max(),
                max_entries: params.max_entries,
            },
        },
        warnings,
    })
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 从缓存的压缩扫描结果中按名单严格筛选, 返回 (扫描代数, [(行下标, Entry)])。
/// mode/src 失配、规则不一致或名单含未知名 = 扫描过期(宁停不错, 5.4)
fn select_compress_rows(
    scan: Option<&ScanContext>,
    src: &str,
    entries: &[String],
    rules: &ExcludeRules,
) -> Result<(u64, Vec<(usize, Entry)>), String> {
    let ctx = match scan {
        Some(c) if c.mode == "compress" && c.src == Path::new(src) && rules_eq(&c.rules, rules) => {
            c
        }
        _ => return Err(EXPIRED.into()),
    };
    let report = match &ctx.data {
        ScanData::Compress(r) => r,
        _ => return Err(EXPIRED.into()),
    };
    let selected = select_from_report(report, entries)?;
    Ok((ctx.generation, selected))
}

fn select_from_report(
    report: &ScanReport,
    entries: &[String],
) -> Result<Vec<(usize, Entry)>, String> {
    let wanted: std::collections::HashSet<&str> = entries.iter().map(String::as_str).collect();
    if wanted.len() != entries.len() {
        return Err(EXPIRED.into());
    }
    let selected: Vec<(usize, Entry)> = report
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| wanted.contains(entry.name.as_str()))
        .map(|(index, entry)| (index, entry.clone()))
        .collect();
    if selected.len() != wanted.len() {
        return Err(EXPIRED.into());
    }
    Ok(selected)
}

#[cfg(test)]
fn select_compress(
    scan: Option<&ScanContext>,
    src: &str,
    entries: &[String],
    rules: &ExcludeRules,
) -> Result<Vec<Entry>, String> {
    let (_, rows) = select_compress_rows(scan, src, entries, rules)?;
    Ok(rows.into_iter().map(|(_, entry)| entry).collect())
}

/// 从缓存的解压扫描结果中按名单严格筛选(名单 = 入口卷文件名)
fn select_extract(
    scan: Option<&ScanContext>,
    src: &str,
    entries: &[String],
) -> Result<Vec<ExtractGroup>, String> {
    let ctx = match scan {
        Some(c) if c.mode == "extract" && c.src == Path::new(src) => c,
        _ => return Err(EXPIRED.into()),
    };
    let s = match &ctx.data {
        ScanData::Extract(s) => s,
        _ => return Err(EXPIRED.into()),
    };
    let selected: Vec<ExtractGroup> = s
        .groups
        .iter()
        .filter(|g| entries.iter().any(|n| n == &file_name(&g.resolved.path)))
        .cloned()
        .collect();
    if selected.len() != entries.len() {
        return Err(EXPIRED.into());
    }
    Ok(selected)
}

/// 规则等价: 开关 + 模式逐条相等(顺序敏感, UI 透传顺序即扫描顺序)
fn rules_eq(a: &ExcludeRules, b: &ExcludeRules) -> bool {
    a.include_hidden == b.include_hidden && a.patterns == b.patterns
}

/// 排除原因的前端文案(ScanItem.excluded)
fn excluded_text(r: &ExcludeReason) -> String {
    match r {
        ExcludeReason::Pattern(p) => format!("匹配排除模式: {p}"),
    }
}

fn compress_scan_dto(r: &ScanReport) -> ScanResult {
    ScanResult {
        mode: "compress".into(),
        total_bytes: r.total_bytes,
        warnings: r.warnings.clone(),
        items: r
            .entries
            .iter()
            .map(|e| ScanItem {
                name: e.name.clone(),
                kind: kind_str(e.kind).into(),
                size: e.size,
                status: "pending".into(),
                original: None,
                detail: None,
                excluded: e.exclude.as_ref().map(excluded_text),
            })
            .collect(),
    }
}

fn extract_scan_dto(s: &ExtractScan) -> ScanResult {
    let mut items: Vec<ScanItem> = s
        .groups
        .iter()
        .map(|g| ScanItem {
            name: file_name(&g.resolved.path),
            kind: "file".into(),
            size: g.size,
            status: "pending".into(),
            original: g.original.clone(),
            detail: None,
            excluded: None,
        })
        .collect();
    if let Some(m) = &s.meta {
        items.push(ScanItem {
            name: m.name.clone(),
            kind: "file".into(),
            size: m.size,
            status: "meta".into(),
            original: None,
            detail: Some(format!("{} 行", m.rows)),
            excluded: None,
        });
    }
    for i in &s.issues {
        items.push(ScanItem {
            name: i.name.clone(),
            kind: "file".into(),
            size: 0,
            status: "warning".into(),
            original: None,
            detail: Some(i.message.clone()),
            excluded: None,
        });
    }
    ScanResult {
        mode: "extract".into(),
        total_bytes: s.groups.iter().map(|g| g.size).sum(),
        warnings: Vec::new(),
        items,
    }
}

/// 只扫描不执行(3.1): 选源后渲染表格。compress 递归统计大小(Scanning 进度),
/// extract 顶层分组(量小, 无进度)。结果缓存在外壳, 供确认命令筛选使用。
#[tauri::command]
async fn scan(
    mode: String,
    path: String,
    filters: Option<settings::FilterSettingsDto>,
    excludes: Option<Vec<String>>,
    on_progress: Channel<ProgressEvent>,
    state: State<'_, AppState>,
) -> Result<ScanResult, String> {
    let mode: &'static str = match mode.as_str() {
        "compress" => "compress",
        "extract" => "extract",
        other => return Err(format!("未知模式: {other}")),
    };
    // 宁停不错: 策略 + 用户模式在占忙之前展开并校验
    let filters = filters.unwrap_or_default();
    let rules = zip_tool_core::effective_rules(&filters.to_core(), &excludes.unwrap_or_default())
        .map_err(|e| format!("排除模式无效: {e}"))?;
    let p = PathBuf::from(&path);
    let src_for_ctx = p.clone();
    acquire_busy(&state.busy, "scan")?;

    // rules 既要进 move 闭包又要进缓存, 先克隆一份给闭包
    let rules_for_scan = rules.clone();
    let result = tauri::async_runtime::spawn_blocking(move || match mode {
        "compress" => zip_tool_core::scan_dir(&p, &rules_for_scan, &|prog| {
            let _ = on_progress.send(ProgressEvent::from(prog));
        })
        .map(ScanData::Compress),
        _ => zip_tool_core::scan_groups(&p).map(ScanData::Extract),
    })
    .await
    .map_err(|e| e.to_string());

    let data = match result {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => {
            release_busy(&state.busy);
            return Err(e.to_string());
        }
        Err(e) => {
            release_busy(&state.busy);
            return Err(e);
        }
    };

    let dto = match &data {
        ScanData::Compress(r) => compress_scan_dto(r),
        ScanData::Extract(s) => extract_scan_dto(s),
    };
    // 扫描成功才覆盖缓存(失败保留旧值, 但确认命令会因 src/mode 校验拒绝误用)
    let generation = state.scan_generation.fetch_add(1, Ordering::Relaxed) + 1;
    *state.scan.lock().map_err(|_| POISONED.to_string())? = Some(ScanContext {
        mode,
        src: src_for_ctx,
        rules,
        generation,
        data,
    });
    release_busy(&state.busy);
    Ok(dto)
}

/// 计算可审核的打包方案(纯内存, 不读盘/不压缩/不占忙锁)。
/// 结果基于最近一次 compress scan; 方案内容只由勾选名单 + packaging 参数决定。
#[tauri::command]
fn plan_packages(
    src: String,
    entries: Vec<String>,
    packaging: PackagingOptions,
    state: State<'_, AppState>,
) -> Result<PlanResult, String> {
    let params = to_strategy_params(&packaging)?;
    let scan = state.scan.lock().map_err(|_| POISONED.to_string())?;
    let ctx = scan.as_ref().ok_or_else(|| EXPIRED.to_string())?;
    if ctx.mode != "compress" || ctx.src != Path::new(&src) {
        return Err(EXPIRED.into());
    }
    let report = match &ctx.data {
        ScanData::Compress(report) => report,
        ScanData::Extract(_) => return Err(EXPIRED.into()),
    };
    let rows = select_from_report(report, &entries)?;
    build_plan_result(ctx.generation, &rows, &params)
}

/// 批量压缩(3.2): src 必须与最近一次 scan(compress) 的源一致,
/// entries = 勾选名单(scan 返回的原样条目名)。
/// 无 packaging = 旧逐条目路径; 有 packaging = 包级策略路径(必须先审核 planId)。
#[tauri::command]
async fn compress(
    src: String,
    dst: String,
    entries: Vec<String>,
    mut options: CompressOptions,
    plan_id: Option<String>,
    on_progress: Channel<ProgressEvent>,
    state: State<'_, AppState>,
) -> Result<CompressResult, String> {
    let packaging = options.packaging.clone();
    // 包级路径以 packaging.splitAbove 为准; 旧 splitSize 仅用于无 packaging 的旧路径
    if packaging.is_some() {
        options.split_size = None;
    }
    let mut opts = to_core_options(options)?;
    let strategy_params = match packaging.as_ref() {
        Some(packaging) => Some(to_strategy_params(packaging)?),
        None => None,
    };
    if let Some(params) = &strategy_params
        && params.split_above.is_some()
    {
        opts.split_size = params.split_above;
    }

    acquire_busy(&state.busy, "compress")?;
    // ScanContext 校验与筛选(纯内存; 锁在 await 前释放)
    let selected = {
        let scan = state.scan.lock().map_err(|_| POISONED.to_string())?;
        select_compress_rows(scan.as_ref(), &src, &entries, &opts.exclude)
    };
    let (generation, selected_rows) = match selected {
        Ok(v) => v,
        Err(e) => {
            release_busy(&state.busy);
            return Err(e);
        }
    };
    let selected: Vec<Entry> = selected_rows.into_iter().map(|(_, entry)| entry).collect();

    if let Some(params) = &strategy_params {
        let names: Vec<&str> = selected.iter().map(|entry| entry.name.as_str()).collect();
        let expected = plan_signature(generation, &names, params);
        let Some(got) = plan_id else {
            release_busy(&state.busy);
            return Err("缺少打包方案 id, 请先审核打包方案".into());
        };
        if got != expected {
            release_busy(&state.busy);
            return Err("打包方案已变化, 请重新审核".into());
        }
    }

    let flag = CancelFlag::new();
    if let Ok(mut c) = state.current.lock() {
        *c = Some(flag.clone());
    }
    let src_path = PathBuf::from(&src);
    let dst_path = PathBuf::from(&dst);
    let result =
        tauri::async_runtime::spawn_blocking(move || -> zip_tool_core::Result<CompressOutcome> {
            let cb_flag = flag.clone();
            if let Some(params) = strategy_params {
                let set =
                    zip_tool_core::plan_packages(&src_path, &dst_path, &selected, &params, &opts)?;
                let report = zip_tool_core::compress_packages(
                    &src_path,
                    &dst_path,
                    &set.packages,
                    &opts,
                    params.split_above,
                    &flag,
                    &cb_flag.into_callback(|p| {
                        let _ = on_progress.send(ProgressEvent::from(p));
                    }),
                )?;
                return Ok(CompressOutcome::Packaged(report));
            }

            let mut selected = selected;
            zip_tool_core::plan(&src_path, &dst_path, &mut selected, &opts)?;
            let report = zip_tool_core::compress_entries(
                &src_path,
                &dst_path,
                &selected,
                &opts,
                &flag,
                &cb_flag.into_callback(|p| {
                    let _ = on_progress.send(ProgressEvent::from(p));
                }),
            )?;
            Ok(CompressOutcome::Legacy(report))
        })
        .await
        .map_err(|e| e.to_string());

    finish_job(&state);
    match result?.map_err(|e| e.to_string())? {
        CompressOutcome::Legacy(report) => Ok(CompressResult {
            cancelled: report.cancelled,
            warnings: report.warnings,
            entries: report
                .entries
                .into_iter()
                .map(|e| ArchiveInfo {
                    archive: e.archive.display().to_string(),
                    original: e.original,
                })
                .collect(),
            packages: Vec::new(),
        }),
        CompressOutcome::Packaged(report) => Ok(CompressResult {
            cancelled: report.cancelled,
            warnings: report.warnings,
            entries: Vec::new(),
            packages: report
                .packages
                .into_iter()
                .map(|package| CompressedPackageInfo {
                    id: package.id,
                    kind: package_kind_str(package.kind).into(),
                    archive: package.archive.display().to_string(),
                    members: package.members,
                    input_bytes: package.input_bytes,
                    entry_count: package.entry_count,
                })
                .collect(),
        }),
    }
}

enum CompressOutcome {
    Legacy(zip_tool_core::Report),
    Packaged(zip_tool_core::PackageReport),
}

/// 批量解压(3.3): src 必须与最近一次 scan(extract) 的源一致,
/// entries = 勾选名单(scan 返回的原样入口卷名), 执行 = extract_prepared(B2 预检 + B3)。
#[tauri::command]
async fn extract_archive(
    src: String,
    dst: String,
    entries: Vec<String>,
    options: ExtractOptionsDto,
    on_progress: Channel<ProgressEvent>,
    state: State<'_, AppState>,
) -> Result<ExtractResult, String> {
    let password = non_empty(options.password);
    let force = options.force;
    acquire_busy(&state.busy, "extract")?;
    let selected = {
        let scan = state.scan.lock().map_err(|_| POISONED.to_string())?;
        select_extract(scan.as_ref(), &src, &entries)
    };
    let selected = match selected {
        Ok(v) => v,
        Err(e) => {
            release_busy(&state.busy);
            return Err(e);
        }
    };

    let flag = CancelFlag::new();
    if let Ok(mut c) = state.current.lock() {
        *c = Some(flag.clone());
    }
    let dst_path = PathBuf::from(&dst);
    let result = tauri::async_runtime::spawn_blocking(move || {
        let cb_flag = flag.clone();
        zip_tool_core::extract_prepared(
            &selected,
            &dst_path,
            password.as_deref(),
            force,
            &flag,
            &cb_flag.into_callback(|p| {
                let _ = on_progress.send(ProgressEvent::from(p));
            }),
        )
    })
    .await
    .map_err(|e| e.to_string());

    finish_job(&state);
    let report = result?.map_err(|e| e.to_string())?;
    Ok(ExtractResult {
        cancelled: report.cancelled,
        warnings: report.warnings,
        groups: report
            .groups
            .into_iter()
            .map(|g| ExtractedGroupInfo {
                entry: g.entry,
                original: g.original,
                files: g.files,
            })
            .collect(),
    })
}

/// 拖拽/选择路径校验: 必须是已存在的文件夹。
/// 用 spawn_blocking 避免网络盘 stat 阻塞 UI; 只做只读检查, 不改变 core 的预检职责。
#[tauri::command]
async fn validate_dir(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || match std::fs::metadata(&path) {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err("只能拖入文件夹".to_string()),
        Err(e) => Err(format!("无法读取路径: {e}")),
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 取消当前任务(立即 kill 在跑的后端进程并清理半成品, 停止后续); 无任务时为 no-op
#[tauri::command]
fn cancel_current(state: State<'_, AppState>) {
    if let Ok(current) = state.current.lock()
        && let Some(flag) = current.as_ref()
    {
        flag.cancel();
    }
}

/// 阻塞等待任务收尾: busy 释放(或超时)后返回是否空闲。
/// 退出前给"kill 子进程 + 清理半成品"留时间(任务收尾在 spawn_blocking 线程, 无死锁)。
fn wait_until_idle(busy: &Mutex<Option<&'static str>>, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match busy.lock() {
            Ok(b) if b.is_none() => return true,
            // 锁中毒: 无法确认状态, 按失败处理放行退出
            Err(_) => return false,
            Ok(_) => {}
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 读取设置: `app_config_dir()/settings.json`; 失败回退默认值并带警告。
#[tauri::command]
async fn load_settings(app: AppHandle) -> Result<SettingsLoadResult, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let (file, warnings) = crate::settings::load_from(&dir);
        SettingsLoadResult {
            settings: file.to_dto(),
            warnings,
        }
    })
    .await
    .map_err(|e| e.to_string())
}

/// 保存设置: 归一化后落盘, 返回 canonical DTO(未知策略 id 被丢弃)。
#[tauri::command]
async fn save_settings(
    app: AppHandle,
    settings: crate::settings::SettingsDto,
) -> Result<crate::settings::SettingsDto, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let canonical = crate::settings::canonicalize(settings);
        crate::settings::save_to(&dir, &canonical).map_err(|e| e.to_string())?;
        Ok::<_, String>(canonical.to_dto())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 系统文件夹选择对话框(点击源/目标输入框时弹出)
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            scan,
            plan_packages,
            compress,
            extract_archive,
            validate_dir,
            cancel_current,
            load_settings,
            save_settings
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // 窗口全部关闭(或 app.exit)时: 取消在跑的任务 —— core 会 kill 在跑的
            // 后端进程并清理半成品 —— 等其收尾后再退出, 避免孤儿进程占用压缩包。
            // scan 在跑时 current 为 None(扫描无外部进程, 随进程退出自然终止)。
            if let tauri::RunEvent::ExitRequested { .. } = event
                && let Some(state) = app.try_state::<AppState>()
            {
                if let Ok(current) = state.current.lock()
                    && let Some(flag) = current.as_ref()
                {
                    flag.cancel();
                }
                // 上限兜底: 正常 kill+清理在数百 ms 内完成, 超时放行退出
                wait_until_idle(&state.busy, Duration::from_secs(5));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;

    fn entry(name: &str, size: u64, kind: EntryKind) -> Entry {
        Entry {
            name: name.into(),
            kind,
            size,
            entry_count: 1,
            dst: String::new(),
            format: String::new(),
            exclude: None,
        }
    }

    fn compress_ctx(src: &str) -> ScanContext {
        ScanContext {
            mode: "compress",
            src: PathBuf::from(src),
            rules: ExcludeRules::default(),
            generation: 1,
            data: ScanData::Compress(ScanReport {
                entries: vec![
                    entry("A", 1, EntryKind::Dir),
                    entry("b.txt", 2, EntryKind::File),
                ],
                total_bytes: 3,
                warnings: vec!["warn1".into()],
                excluded: Vec::new(),
            }),
        }
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn progress_event_maps_phase_and_bytes() {
        let p = Progress {
            phase: Phase::Compressing,
            done: 2,
            total: 5,
            done_bytes: 30,
            total_bytes: 100,
            entry: "B".into(),
        };
        let e = ProgressEvent::from(&p);
        assert_eq!(e.phase, "compressing");
        assert_eq!(e.done_bytes, 30);
        assert_eq!(e.total_bytes, 100);
    }

    #[test]
    fn select_compress_filters_by_names_and_rejects_mismatch() {
        let ctx = compress_ctx("C:\\s");
        // 正常筛选
        let sel =
            select_compress(Some(&ctx), "C:\\s", &s(&["A"]), &ExcludeRules::default()).unwrap();
        assert_eq!(sel.len(), 1);
        assert_eq!(sel[0].name, "A");
        // 未知名 = 过期
        assert!(
            select_compress(Some(&ctx), "C:\\s", &s(&["X"]), &ExcludeRules::default()).is_err()
        );
        // 重复名 = 过期(严格长度校验)
        assert!(
            select_compress(
                Some(&ctx),
                "C:\\s",
                &s(&["A", "A"]),
                &ExcludeRules::default()
            )
            .is_err()
        );
        // src 失配 / 模式失配 / 无缓存
        assert!(
            select_compress(
                Some(&ctx),
                "C:\\other",
                &s(&["A"]),
                &ExcludeRules::default()
            )
            .is_err()
        );
        assert!(select_extract(Some(&ctx), "C:\\s", &s(&["A"])).is_err());
        assert!(select_compress(None, "C:\\s", &s(&["A"]), &ExcludeRules::default()).is_err());
    }

    #[test]
    fn select_extract_filters_by_entry_volume_names() {
        // 路径用 join 构造而非硬编码分隔符: unix 上反斜杠不是分隔符,
        // file_name 提取会拿整串, 过滤断言就失真了
        let ctx = ScanContext {
            mode: "extract",
            src: PathBuf::from("s"),
            rules: ExcludeRules::default(),
            generation: 1,
            data: ScanData::Extract(ExtractScan {
                groups: vec![ExtractGroup {
                    resolved: zip_tool_core::extract::ResolvedArchive {
                        path: PathBuf::from("s").join("Novel.zip"),
                        backend: zip_tool_core::backend::Backend::native(),
                    },
                    original: Some("Novel".into()),
                    size: 10,
                }],
                meta: None,
                issues: Vec::new(),
            }),
        };
        let sel = select_extract(Some(&ctx), "s", &s(&["Novel.zip"])).unwrap();
        assert_eq!(sel.len(), 1);
        assert!(select_extract(Some(&ctx), "s", &s(&["zt_x.zip"])).is_err());
    }

    #[test]
    fn busy_mutex_is_exclusive() {
        let busy: Mutex<Option<&'static str>> = Mutex::new(None);
        acquire_busy(&busy, "scan").unwrap();
        assert!(acquire_busy(&busy, "compress").is_err(), "忙时应拒绝");
        release_busy(&busy);
        acquire_busy(&busy, "compress").unwrap();
    }

    #[test]
    fn compress_scan_dto_maps_items_and_warnings() {
        let report = ScanReport {
            entries: vec![
                entry("Novel", 7, EntryKind::Dir),
                entry("a.jpg", 1, EntryKind::File),
            ],
            total_bytes: 8,
            warnings: vec!["no permission".into()],
            excluded: Vec::new(),
        };
        let dto = compress_scan_dto(&report);
        assert_eq!(dto.mode, "compress");
        assert_eq!(dto.total_bytes, 8);
        assert_eq!(dto.warnings, ["no permission"]);
        assert_eq!(dto.items.len(), 2);
        assert_eq!(dto.items[0].kind, "dir");
        assert_eq!(dto.items[0].status, "pending");
        assert_eq!(dto.items[1].kind, "file");
    }

    #[test]
    fn extract_scan_dto_maps_meta_and_issues() {
        let scan = ExtractScan {
            groups: vec![ExtractGroup {
                resolved: zip_tool_core::extract::ResolvedArchive {
                    // join 构造: 保证两平台 file_name 都是 "Novel.zip"
                    path: PathBuf::from("s").join("Novel.zip"),
                    backend: zip_tool_core::backend::Backend::native(),
                },
                original: Some("Novel".into()),
                size: 150,
            }],
            meta: Some(zip_tool_core::MetaFile {
                name: "mapping.csv".into(),
                size: 30,
                rows: 1,
            }),
            issues: vec![zip_tool_core::ScanIssue {
                name: "notes.txt".into(),
                message: "skipped non-archive file".into(),
            }],
        };
        let dto = extract_scan_dto(&scan);
        assert_eq!(dto.mode, "extract");
        assert_eq!(dto.total_bytes, 150);
        assert!(dto.warnings.is_empty(), "extract 告警走 warning 行");
        assert_eq!(dto.items.len(), 3);
        // 组行: 显示名信息在 original, name = 入口卷
        assert_eq!(dto.items[0].name, "Novel.zip");
        assert_eq!(dto.items[0].original.as_deref(), Some("Novel"));
        assert_eq!(dto.items[0].size, 150);
        // 元数据行
        assert_eq!(dto.items[1].status, "meta");
        assert_eq!(dto.items[1].name, "mapping.csv");
        assert_eq!(dto.items[1].size, 30);
        assert!(dto.items[1].detail.as_deref().unwrap().contains('1'));
        // 告警行
        assert_eq!(dto.items[2].status, "warning");
        assert_eq!(dto.items[2].name, "notes.txt");
        assert!(
            dto.items[2]
                .detail
                .as_deref()
                .unwrap()
                .contains("non-archive")
        );
    }

    #[test]
    fn non_empty_treats_blank_as_none() {
        assert_eq!(non_empty(None), None);
        assert_eq!(non_empty(Some("  ".into())), None);
        assert_eq!(non_empty(Some("pw".into())), Some("pw".to_string()));
    }

    /// 枚举两分支各持自己的扫描类型(保鲜上下文按分支取数)
    #[test]
    fn scan_data_enum_holds_both_kinds() {
        let ctx = compress_ctx("x");
        assert!(matches!(ctx.data, ScanData::Compress(_)));
    }

    #[test]
    fn wait_until_idle_returns_immediately_when_idle() {
        let busy: Mutex<Option<&'static str>> = Mutex::new(None);
        let start = std::time::Instant::now();
        assert!(wait_until_idle(&busy, Duration::from_secs(1)));
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "空闲应立即返回"
        );
    }

    #[test]
    fn wait_until_idle_times_out_when_busy() {
        let busy: Mutex<Option<&'static str>> = Mutex::new(Some("compress"));
        let start = std::time::Instant::now();
        assert!(!wait_until_idle(&busy, Duration::from_millis(150)));
        assert!(
            start.elapsed() >= Duration::from_millis(150),
            "忙时应等满超时再放行"
        );
    }

    #[test]
    fn wait_until_idle_returns_once_job_releases() {
        let busy: std::sync::Arc<Mutex<Option<&'static str>>> =
            std::sync::Arc::new(Mutex::new(Some("compress")));
        let b = busy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            *b.lock().unwrap() = None;
        });
        assert!(wait_until_idle(&busy, Duration::from_secs(5)));
    }

    #[test]
    fn to_core_options_maps_format_and_level() {
        let base = || CompressOptions {
            split_size: None,
            password: None,
            random_name: false,
            format: None,
            level: None,
            excludes: None,
            filters: None,
            verify: true,
            packaging: None,
        };
        let mut o = base();
        o.format = Some("7z".into());
        o.level = Some(5);
        let core = to_core_options(o).unwrap();
        assert_eq!(core.format, zip_tool_core::Format::SevenZ);
        assert_eq!(core.level, Some(5));
        assert!(core.verify, "默认开启校验");

        // 缺省 = zip + 后端默认级别
        let core = to_core_options(base()).unwrap();
        assert_eq!(core.format, zip_tool_core::Format::Zip);
        assert!(core.level.is_none());
        assert!(core.verify);

        // 显式关闭 → 透传 false
        let mut o = base();
        o.verify = false;
        assert!(!to_core_options(o).unwrap().verify);

        // 非法值拒绝
        let mut o = base();
        o.format = Some("rar".into());
        assert!(to_core_options(o).is_err());
        let mut o = base();
        o.level = Some(10);
        assert!(to_core_options(o).is_err());
    }

    #[test]
    fn to_core_options_validates_and_carries_excludes() {
        let base = || CompressOptions {
            split_size: None,
            password: None,
            random_name: false,
            format: None,
            level: None,
            excludes: Some(vec!["Thumbs.db".into(), "sub/*.log".into()]),
            filters: None,
            verify: true,
            packaging: None,
        };
        let core = to_core_options(base()).unwrap();
        assert!(core.exclude.patterns.contains(&"sub/*.log".to_string()));
        assert!(core.exclude.patterns.contains(&"node_modules".to_string()));
        assert!(!core.exclude.include_hidden, "默认 os-hidden 开");

        // 非法模式拒绝
        let mut o = base();
        o.excludes = Some(vec!["a/**b".into()]);
        assert!(to_core_options(o).is_err());

        // 缺省用户模式 = 只含默认策略展开的模式
        let mut o = base();
        o.excludes = None;
        let core = to_core_options(o).unwrap();
        assert!(!core.exclude.patterns.contains(&"sub/*.log".to_string()));
        assert!(core.exclude.patterns.contains(&"node_modules".to_string()));
    }

    #[test]
    fn to_core_options_maps_strategy_filters() {
        let base = |enabled: &[&str]| CompressOptions {
            split_size: None,
            password: None,
            random_name: false,
            format: None,
            level: None,
            excludes: None,
            filters: Some(settings::FilterSettingsDto {
                enabled_policies: enabled.iter().map(|s| (*s).to_string()).collect(),
            }),
            verify: true,
            packaging: None,
        };

        let os_only = to_core_options(base(&["os-hidden"])).unwrap();
        assert!(!os_only.exclude.include_hidden);
        assert!(
            os_only.exclude.patterns.is_empty(),
            "os-hidden 由 core 加系统元数据"
        );

        let lang_only = to_core_options(base(&["lang-temp"])).unwrap();
        assert!(
            lang_only.exclude.include_hidden,
            "os-hidden 关 = 不加系统元数据"
        );
        assert!(
            lang_only
                .exclude
                .patterns
                .contains(&"node_modules".to_string())
        );
        assert!(
            !lang_only
                .exclude
                .patterns
                .contains(&"Thumbs.db".to_string())
        );

        let none = to_core_options(base(&[])).unwrap();
        assert!(none.exclude.include_hidden);
        assert!(none.exclude.patterns.is_empty());

        // 未知 id 被忽略 → 等同于全关
        let unknown = to_core_options(base(&["nope"])).unwrap();
        assert!(unknown.exclude.include_hidden);
        assert!(unknown.exclude.patterns.is_empty());
    }

    #[test]
    fn select_compress_rejects_rule_mismatch_as_expired() {
        let ctx = compress_ctx("C:\\s");
        let rules = ExcludeRules::default();
        // 规则一致 → 正常
        assert!(select_compress(Some(&ctx), "C:\\s", &s(&["A"]), &rules).is_ok());
        // 规则不一致 = 扫描过期(5.4 语义扩展)
        let other = ExcludeRules {
            include_hidden: false,
            patterns: vec!["x".into()],
        };
        let err = select_compress(Some(&ctx), "C:\\s", &s(&["A"]), &other).unwrap_err();
        assert!(err.contains("过期"), "{err}");
    }

    #[test]
    fn packaging_options_parse_and_validate() {
        let base = || PackagingOptions {
            merge_below: Some("100m".into()),
            split_above: Some("2g".into()),
            bundle_max: None,
            max_entries: Some(50_000),
        };

        let params = to_strategy_params(&base()).unwrap();
        assert_eq!(params.merge_below, Some(100 * MIB));
        assert_eq!(params.split_above, Some(2 * GIB));
        assert_eq!(params.effective_bundle_max(), Some(2 * GIB));
        assert_eq!(params.max_entries, Some(50_000));

        // 空串 = 关闭
        let mut o = base();
        o.merge_below = Some("  ".into());
        o.split_above = None;
        let params = to_strategy_params(&o).unwrap();
        assert_eq!(params.merge_below, None);
        assert_eq!(params.split_above, None);
        assert_eq!(params.effective_bundle_max(), None);

        // 不自洽 / 非法值
        let mut o = base();
        o.merge_below = Some("200m".into());
        o.split_above = Some("100m".into());
        assert!(to_strategy_params(&o).is_err());
        let mut o = base();
        o.max_entries = Some(0);
        assert!(to_strategy_params(&o).is_err());
        let mut o = base();
        o.split_above = Some("abc".into());
        assert!(to_strategy_params(&o).is_err());
    }

    #[test]
    fn plan_signature_changes_with_generation_names_and_params() {
        let params = to_strategy_params(&PackagingOptions {
            merge_below: Some("100m".into()),
            split_above: Some("2g".into()),
            bundle_max: None,
            max_entries: Some(50_000),
        })
        .unwrap();
        let names = ["A", "B"];
        let base = plan_signature(1, &names, &params);
        assert_eq!(base, plan_signature(1, &names, &params));
        assert_ne!(base, plan_signature(2, &names, &params));
        assert_ne!(base, plan_signature(1, &["B", "A"], &params));

        let mut changed = params.clone();
        changed.merge_below = Some(50 * MIB);
        assert_ne!(base, plan_signature(1, &names, &changed));
    }

    #[test]
    fn build_plan_result_maps_members_indexes_and_summary() {
        let params = to_strategy_params(&PackagingOptions {
            merge_below: Some("100m".into()),
            split_above: Some("2g".into()),
            bundle_max: None,
            max_entries: Some(50_000),
        })
        .unwrap();
        let rows = vec![
            (0, entry("A", 3 * GIB, EntryKind::Dir)),
            (1, entry("b.txt", 2 * MIB, EntryKind::File)),
            (2, entry("C", MIB, EntryKind::File)),
        ];
        let result = build_plan_result(7, &rows, &params).unwrap();

        assert_eq!(
            result.plan_id,
            plan_signature(7, &["A", "b.txt", "C"], &params)
        );
        assert_eq!(result.summary.total_packages, 2);
        assert_eq!(result.summary.direct, 0);
        assert_eq!(result.summary.split, 1);
        assert_eq!(result.summary.bundle, 1);
        assert_eq!(result.summary.selected_bytes, 3 * GIB + 3 * MIB);
        assert_eq!(result.summary.max_direct_or_bundle_bytes, 3 * MIB);
        assert_eq!(result.summary.max_bundle_entries, 2);
        assert_eq!(result.summary.effective.bundle_max, Some(2 * GIB));

        assert_eq!(result.packages[0].kind, "split");
        assert_eq!(result.packages[0].member_indexes, vec![0]);
        assert_eq!(result.packages[1].kind, "bundle");
        assert_eq!(result.packages[1].member_indexes, vec![1, 2]);
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn build_plan_result_preserves_entry_limit_warning() {
        let params = to_strategy_params(&PackagingOptions {
            merge_below: Some("100m".into()),
            split_above: Some("2g".into()),
            bundle_max: None,
            max_entries: Some(1),
        })
        .unwrap();
        let mut big = entry("bigdir", 10 * MIB, EntryKind::Dir);
        big.entry_count = 2;
        let rows = vec![(4, big), (5, entry("small", 10 * MIB, EntryKind::File))];
        let result = build_plan_result(1, &rows, &params).unwrap();

        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.warnings[0].code, "tooManyEntries");
        assert_eq!(result.warnings[0].member_index, 4);
        assert_eq!(result.warnings[0].entry_count, 2);
        assert_eq!(result.warnings[0].limit, 1);
        assert!(
            result
                .packages
                .iter()
                .any(|package| { package.kind == "direct" && package.member_indexes == vec![4] })
        );
    }

    #[test]
    fn select_from_report_rejects_unknown_names() {
        let report = ScanReport {
            entries: vec![entry("A", 1, EntryKind::Dir)],
            total_bytes: 1,
            warnings: Vec::new(),
            excluded: Vec::new(),
        };
        assert!(select_from_report(&report, &s(&["A"])).is_ok());
        assert!(select_from_report(&report, &s(&["missing"])).is_err());
    }

    #[test]
    fn compress_scan_dto_marks_excluded_items() {
        let mut a = entry("A", 1, EntryKind::Dir);
        a.exclude = Some(ExcludeReason::Pattern("node_modules".into()));
        let mut b = entry("b.txt", 2, EntryKind::File);
        b.exclude = Some(ExcludeReason::Pattern("*.log".into()));
        let report = ScanReport {
            entries: vec![a, b],
            total_bytes: 0,
            warnings: Vec::new(),
            excluded: Vec::new(),
        };
        let dto = compress_scan_dto(&report);
        assert_eq!(
            dto.items[0].excluded.as_deref(),
            Some("匹配排除模式: node_modules")
        );
        assert_eq!(
            dto.items[1].excluded.as_deref(),
            Some("匹配排除模式: *.log")
        );
    }
}
