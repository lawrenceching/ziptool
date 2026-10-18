//! Tauri 外壳:把 `zip-tool-core` 的能力通过 `#[tauri::command]` 暴露给前端。
//! 交互规格见 _docs/ui-backend.md, 命令面 4 个:
//! - `scan`: 只扫描不执行, 结果缓存在外壳([`ScanContext`] 保鲜)
//! - `compress` / `extract_archive`: 按勾选名单筛选缓存结果, 走 core 分阶段 API
//!   (plan + compress_entries / extract_prepared), **不重复扫描**
//! - `cancel_current`: 取消当前任务(立即 kill 在跑的后端进程并清理半成品, 停止后续)
//!
//! 命令在 `spawn_blocking` 中调 core, 进度经 `tauri::ipc::Channel` 推给前端;
//! scan/compress/extract 单任务互斥, 任务结束释放忙闲标志并清空取消旗标。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri::State;
use tauri::ipc::Channel;

use zip_tool_core::{
    CancelFlag, Entry, EntryKind, ExcludeReason, ExcludeRules, ExtractGroup, ExtractScan, Format,
    Options, Phase, Progress, ScanReport,
};

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
}

/// scan 命令缓存的扫描结果(mode + src + 规则匹配才可用)
struct ScanContext {
    mode: &'static str,
    src: PathBuf,
    /// 扫描时的排除规则(compress 模式;确认时比对, 不一致 = 过期)
    rules: ExcludeRules,
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
    /// "2g"/"500m"/字节数; None = 不分包
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
struct CompressResult {
    cancelled: bool,
    entries: Vec<ArchiveInfo>,
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
    // 宁停不错: 用户模式非法即拒(与 CLI exit 2 同语义)
    let patterns = match o.excludes {
        None => Vec::new(),
        Some(v) => match zip_tool_core::validate_patterns(&v) {
            Ok(()) => v,
            Err(e) => return Err(format!("排除模式无效: {e}")),
        },
    };
    Ok(Options {
        split_size,
        password: non_empty(o.password),
        random_name: o.random_name,
        format,
        level,
        exclude: ExcludeRules {
            // UI 恒定排除隐藏(被标行显示不勾选; 勾选了也尊重用户)
            include_hidden: false,
            patterns,
        },
    })
}

fn kind_str(k: EntryKind) -> &'static str {
    match k {
        EntryKind::Dir => "dir",
        EntryKind::File => "file",
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 从缓存的压缩扫描结果中按名单严格筛选;
/// mode/src 失配、规则不一致或名单含未知名 = 扫描过期(宁停不错, 5.4)
fn select_compress(
    scan: Option<&ScanContext>,
    src: &str,
    entries: &[String],
    rules: &ExcludeRules,
) -> Result<Vec<Entry>, String> {
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
    let selected: Vec<Entry> = report
        .entries
        .iter()
        .filter(|e| entries.iter().any(|n| n == &e.name))
        .cloned()
        .collect();
    if selected.len() != entries.len() {
        return Err(EXPIRED.into());
    }
    Ok(selected)
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
        ExcludeReason::Hidden => "隐藏文件".into(),
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
    excludes: Option<Vec<String>>,
    on_progress: Channel<ProgressEvent>,
    state: State<'_, AppState>,
) -> Result<ScanResult, String> {
    let mode: &'static str = match mode.as_str() {
        "compress" => "compress",
        "extract" => "extract",
        other => return Err(format!("未知模式: {other}")),
    };
    // 宁停不错: 非法模式在占忙之前拒绝
    if let Some(pats) = &excludes
        && let Err(e) = zip_tool_core::validate_patterns(pats)
    {
        return Err(format!("排除模式无效: {e}"));
    }
    // UI 恒定排除隐藏(被标行显示不勾选)
    let rules = ExcludeRules {
        include_hidden: false,
        patterns: excludes.unwrap_or_default(),
    };
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
    *state.scan.lock().map_err(|_| POISONED.to_string())? = Some(ScanContext {
        mode,
        src: src_for_ctx,
        rules,
        data,
    });
    release_busy(&state.busy);
    Ok(dto)
}

/// 批量压缩(3.2): src 必须与最近一次 scan(compress) 的源一致,
/// entries = 勾选名单(scan 返回的原样条目名), 执行 = plan + compress_entries。
#[tauri::command]
async fn compress(
    src: String,
    dst: String,
    entries: Vec<String>,
    options: CompressOptions,
    on_progress: Channel<ProgressEvent>,
    state: State<'_, AppState>,
) -> Result<CompressResult, String> {
    let opts = to_core_options(options)?;

    acquire_busy(&state.busy, "compress")?;
    // ScanContext 校验与筛选(纯内存; 锁在 await 前释放)
    let selected = {
        let scan = state.scan.lock().map_err(|_| POISONED.to_string())?;
        select_compress(scan.as_ref(), &src, &entries, &opts.exclude)
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
    let src_path = PathBuf::from(&src);
    let dst_path = PathBuf::from(&dst);
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut selected = selected;
        let cb_flag = flag.clone();
        zip_tool_core::plan(&src_path, &dst_path, &mut selected, &opts)?;
        zip_tool_core::compress_entries(
            &src_path,
            &dst_path,
            &selected,
            &opts,
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
    Ok(CompressResult {
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
    })
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 系统文件夹选择对话框(点击源/目标输入框时弹出)
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            scan,
            compress,
            extract_archive,
            cancel_current
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

    fn entry(name: &str, size: u64, kind: EntryKind) -> Entry {
        Entry {
            name: name.into(),
            kind,
            size,
            dst: String::new(),
            format: String::new(),
            exclude: None,
            attr_hidden: Vec::new(),
        }
    }

    fn compress_ctx(src: &str) -> ScanContext {
        ScanContext {
            mode: "compress",
            src: PathBuf::from(src),
            rules: ExcludeRules::default(),
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
        let ctx = ScanContext {
            mode: "extract",
            src: PathBuf::from("C:\\s"),
            rules: ExcludeRules::default(),
            data: ScanData::Extract(ExtractScan {
                groups: vec![ExtractGroup {
                    resolved: zip_tool_core::extract::ResolvedArchive {
                        path: PathBuf::from("C:\\s\\Novel.zip"),
                        backend: zip_tool_core::backend::Backend::native(),
                    },
                    original: Some("Novel".into()),
                    size: 10,
                }],
                meta: None,
                issues: Vec::new(),
            }),
        };
        let sel = select_extract(Some(&ctx), "C:\\s", &s(&["Novel.zip"])).unwrap();
        assert_eq!(sel.len(), 1);
        assert!(select_extract(Some(&ctx), "C:\\s", &s(&["zt_x.zip"])).is_err());
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
                    path: PathBuf::from("C:\\s\\Novel.zip"),
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
        };
        let mut o = base();
        o.format = Some("7z".into());
        o.level = Some(5);
        let core = to_core_options(o).unwrap();
        assert_eq!(core.format, zip_tool_core::Format::SevenZ);
        assert_eq!(core.level, Some(5));

        // 缺省 = zip + 后端默认级别
        let core = to_core_options(base()).unwrap();
        assert_eq!(core.format, zip_tool_core::Format::Zip);
        assert!(core.level.is_none());

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
        };
        let core = to_core_options(base()).unwrap();
        assert_eq!(core.exclude.patterns.len(), 2);
        assert!(!core.exclude.include_hidden, "UI 恒定排除隐藏");

        // 非法模式拒绝
        let mut o = base();
        o.excludes = Some(vec!["a/**b".into()]);
        assert!(to_core_options(o).is_err());

        // 缺省 = 无用户模式
        let mut o = base();
        o.excludes = None;
        assert!(to_core_options(o).unwrap().exclude.patterns.is_empty());
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
    fn compress_scan_dto_marks_excluded_items() {
        let mut a = entry("A", 1, EntryKind::Dir);
        a.exclude = Some(ExcludeReason::Hidden);
        let mut b = entry("b.txt", 2, EntryKind::File);
        b.exclude = Some(ExcludeReason::Pattern("*.log".into()));
        let report = ScanReport {
            entries: vec![a, b],
            total_bytes: 0,
            warnings: Vec::new(),
            excluded: Vec::new(),
        };
        let dto = compress_scan_dto(&report);
        assert_eq!(dto.items[0].excluded.as_deref(), Some("隐藏文件"));
        assert_eq!(
            dto.items[1].excluded.as_deref(),
            Some("匹配排除模式: *.log")
        );
    }
}
