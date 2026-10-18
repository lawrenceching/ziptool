//! 解压:支持单个 .zip,以及两种原生分卷
//! - `B.z01 ... B.zip`(zip -s,末卷 .zip)→ 只能 7z 系工具解
//!   (unzip 不支持分卷包; 实测各合并方案均产出坏包, 见 `zip_entry_backend`)
//! - `B.zip.001 ...`(7z -v,首卷 .001)→ 只能 7z 系工具解
//!
//! 目标规格见 _docs/extract.md(识别 → 预检 → 解压 三步 + 批量编排层):
//! 先对全部组做预检, 再逐组解压 —— 与 compress 相同的"先全量预检再产出"两段式。

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use crate::CancelFlag;
use crate::Result;
use crate::backend::{ArchiveListing, Backend, ExtractBackend, split_first_volume};
use crate::progress::{Phase, Progress};

/// .zip 入口卷的后端选择: 同目录存在 .z01 兄弟卷 = zip -s 分卷组 → 强制
/// SevenZip。unzip 不支持分卷包(unzip 6.0 自述 "true multi-part support
/// doesn't exist yet"), 且实测 Ubuntu 24.04 的 zip 3.0 上 `zip -s 0` 合并 /
/// `zip -FF` / cat 拼接都产出坏包(中央目录偏移按盘相对), 只有 7z 能直读
/// 分卷组(有/无密码均字节一致); 整包 → 平台默认后端(unzip / 7z)。
fn zip_entry_backend(entry: &Path) -> Backend {
    if split_first_volume(entry).is_some() {
        Backend::SevenZip
    } else {
        Backend::native()
    }
}

/// 解压入口(见 _docs/extract.md): input 为文件(单包)或目录(批量, 第 7 节)。
/// dest 会被创建; 默认拒绝覆盖已有文件, force = true 才覆盖(对应 CLI --force)。
/// password 与压缩时一致; 无密码传 None。
/// 进度经 on_progress 上报(Phase::Extracting)。取消两个生效点:组间由回调
/// Break 生效;组内则 kill 解压进程并回滚本组已解文件(与 compress 一致)。
pub fn extract(
    input: &Path,
    dest: &Path,
    password: Option<&str>,
    force: bool,
    cancel: &CancelFlag,
    on_progress: &dyn Fn(&Progress) -> ControlFlow<()>,
) -> Result<ExtractReport> {
    extract_with_backend(
        &crate::backend::RealExtract {
            program_override: None,
        },
        input,
        dest,
        password,
        force,
        cancel,
        on_progress,
    )
}

/// [`extract`] 的可注入版本: 生产固定 RealExtract, 编排测试注入假后端
pub(crate) fn extract_with_backend<B: ExtractBackend + ?Sized>(
    be: &B,
    input: &Path,
    dest: &Path,
    password: Option<&str>,
    force: bool,
    cancel: &CancelFlag,
    on_progress: &dyn Fn(&Progress) -> ControlFlow<()>,
) -> Result<ExtractReport> {
    let (groups, issues) = if input.is_dir() {
        // 目录 → 批量: Step B1 扫描分组
        let scan = scan_groups(input)?;
        (scan.groups, scan.issues)
    } else {
        // 文件 → 单包: Step 1 识别
        let resolved = resolve_archive(input)?;
        let size = std::fs::symlink_metadata(&resolved.path)
            .map(|m| m.len())
            .unwrap_or(0);
        (
            vec![ExtractGroup {
                resolved,
                original: None,
                size,
            }],
            Vec::new(),
        )
    };
    // 组合路径的 issues 以字符串进报告(UI 分阶段路径直接持有结构化 issues)
    let warnings = issues
        .into_iter()
        .map(|i| format!("{}: {}", i.name, i.message))
        .collect();
    extract_groups(
        be,
        &groups,
        dest,
        password,
        force,
        warnings,
        cancel,
        on_progress,
    )
}

/// 批量解压执行(独立阶段, _docs/ui-backend.md 6): 对给定的组做 B2 全量预检 + B3
/// 逐组解压。UI 分阶段入口: groups 来自 [`scan_groups`] 的缓存结果(可按勾选筛选),
/// 扫描 issues 由外壳直接持有, 不经过本函数。
pub fn extract_prepared(
    groups: &[ExtractGroup],
    dest: &Path,
    password: Option<&str>,
    force: bool,
    cancel: &CancelFlag,
    on_progress: &dyn Fn(&Progress) -> ControlFlow<()>,
) -> Result<ExtractReport> {
    extract_groups(
        &crate::backend::RealExtract {
            program_override: None,
        },
        groups,
        dest,
        password,
        force,
        Vec::new(),
        cancel,
        on_progress,
    )
}

/// Step 1 识别结果:入口卷路径 + 解压后端(见 _docs/extract.md 第 4 节)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedArchive {
    /// 入口卷路径(.zip / .7z 或分卷首卷 .zip.001 / .7z.001), Step 2/3 均以此为准
    pub path: PathBuf,
    /// 整包 .zip → 平台默认后端; 其余分卷形态(zip -s 组=旁有 .z01、.zip.001 /
    /// .7z / .7z.001)→ 强制 Backend::SevenZip(见 [`zip_entry_backend`])
    pub backend: Backend,
}

/// Step 1: 按扩展名识别包形态并归一化到入口卷(大小写不敏感)。
/// 只看扩展名不读内容: 形态契约由 compress 端保证, 外来包内容不符时后端解压阶段自然失败。
pub(crate) fn resolve_archive(input: &Path) -> Result<ResolvedArchive> {
    let ext = input
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let dir = input.parent().unwrap_or(Path::new(""));
    let vol = |name: &str| dir.join(name);
    match ext.as_str() {
        // 整包或 zip -s 末卷: 入口卷即自身; 有 .z01 兄弟卷的分卷组强制 7z 系
        "zip" => Ok(ResolvedArchive {
            path: input.to_path_buf(),
            backend: zip_entry_backend(input),
        }),
        // 7z 格式整包:只有 7z 系工具能解, 强制 SevenZip 后端
        "7z" => Ok(ResolvedArchive {
            path: input.to_path_buf(),
            backend: Backend::SevenZip,
        }),
        // 7z -v 首卷: 入口卷即自身, 且只有 7z 系能解
        "001" => Ok(ResolvedArchive {
            path: input.to_path_buf(),
            backend: Backend::SevenZip,
        }),
        // .zip.NNN / .7z.NNN(NNN>001): 找同包名首卷 .001
        e if is_digits(e) => {
            let first = vol(&format!("{}.001", stem_str(input)));
            locate_entry_volume(input, first, Backend::SevenZip)
        }
        // .zNN(zip -s 非末卷): 找末卷 .zip;末卷旁有 .z01(即本输入)= 分卷组 → 强制 7z
        e if e.len() >= 2 && e.starts_with('z') && is_digits(&e[1..]) => {
            let last = vol(&format!("{}.zip", stem_str(input)));
            locate_entry_volume(input, last, Backend::native()).map(|r| ResolvedArchive {
                backend: zip_entry_backend(&r.path),
                ..r
            })
        }
        _ => Err(crate::CoreError::UnsupportedArchive(input.to_path_buf())),
    }
}

/// 非入口卷 → 同目录定位入口卷;找不到报 EntryVolumeMissing(指明应传的入口卷)
fn locate_entry_volume(got: &Path, expected: PathBuf, backend: Backend) -> Result<ResolvedArchive> {
    if expected.is_file() {
        Ok(ResolvedArchive {
            path: expected,
            backend,
        })
    } else {
        Err(crate::CoreError::EntryVolumeMissing {
            got: got.to_path_buf(),
            expected,
        })
    }
}

/// 全部为 ASCII 数字且非空
fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// file_stem 的字符串形式(用于拼兄弟卷名)
fn stem_str(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 5.2 zip-slip 校验: 清单含绝对路径或 `..` 段即拒绝。
/// 本工具压出的包不会有, 但解压端可能面对外来包; 不依赖后端各自的防护。
fn check_zip_slip(names: &[String]) -> Result<()> {
    for name in names {
        let reason = if is_absolute_name(name) {
            "absolute path"
        } else if name.split('/').any(|seg| seg == "..") {
            "parent reference .."
        } else {
            continue;
        };
        return Err(crate::CoreError::UnsafeArchive {
            path: name.clone(),
            reason: reason.into(),
        });
    }
    Ok(())
}

/// 名字是否绝对路径(以 / 开头, 或 "C:" 盘符形式)。
/// 不依赖平台 Path 语义: Linux 上也要拒绝包里的 "C:/x"。
fn is_absolute_name(name: &str) -> bool {
    name.starts_with('/') || name.as_bytes().get(1) == Some(&b':')
}

/// 5.3 冲突检测: 清单中任一相对路径在 dest 下已存在 → ExtractConflict(最多列 10 个)。
/// 不静默跳过/覆盖: 旧半成品冒充解压结果比失败更危险, 覆盖须显式 --force。
fn check_conflicts(names: &[String], dest: &Path) -> Result<()> {
    let mut hits: Vec<String> = Vec::new();
    for name in names {
        if dest.join(name).symlink_metadata().is_ok() {
            hits.push(name.clone());
            if hits.len() >= 10 {
                break;
            }
        }
    }
    if hits.is_empty() {
        Ok(())
    } else {
        Err(crate::CoreError::ExtractConflict(hits))
    }
}

/// 一个待解压的组(单包模式恰好一个;批量模式来自扫描分组)
#[derive(Debug, Clone)]
pub struct ExtractGroup {
    pub resolved: ResolvedArchive,
    /// mapping.csv 提供的原条目名(无 mapping 时为 None), 用于进度与报告显示
    pub original: Option<String>,
    /// 组内各卷(入口卷+成员卷)大小之和, UI 表格"大小"列用
    pub size: u64,
}

/// 批量解压扫描结果(_docs/ui-backend.md 6): UI 分阶段入口 —— 外壳缓存本结果,
/// 确认时按勾选名单筛选 groups 后调 [`extract_prepared`], 不重扫。
#[derive(Debug, Default)]
pub struct ExtractScan {
    /// 包组, 已排序(mapping 行序优先, 否则按入口卷名)
    pub groups: Vec<ExtractGroup>,
    /// mapping.csv 元数据(展示用); 读取失败时为 None(问题记入 issues)
    pub meta: Option<MetaFile>,
    /// 孤儿卷 / 非包文件 / mapping 异常等非致命问题(有明确文件名, UI 渲染为告警行)
    pub issues: Vec<ScanIssue>,
}

/// mapping.csv 元数据行(只展示不参与解压)
#[derive(Debug, Clone)]
pub struct MetaFile {
    /// 实际文件名(通常 "mapping.csv")
    pub name: String,
    /// 文件大小
    pub size: u64,
    /// 数据行数(不含表头)
    pub rows: usize,
}

/// 扫描中的非致命问题
#[derive(Debug, Clone)]
pub struct ScanIssue {
    /// 关联的文件名(孤儿卷 / 非包文件 / mapping.csv)
    pub name: String,
    /// 具体原因(如 "missing b.zip")
    pub message: String,
}

/// 一组解压完成的记录
#[derive(Debug, Clone)]
pub struct ExtractedGroup {
    /// 入口卷文件名
    pub entry: String,
    /// mapping.csv 提供的原条目名(无 mapping 时为 None)
    pub original: Option<String>,
    /// 该组解出的文件数(目录不计, 来自 list)
    pub files: usize,
}

/// 解压结果(单包模式恰好一组)
#[derive(Debug, Default)]
pub struct ExtractReport {
    /// 已完成的组, 按执行顺序
    pub groups: Vec<ExtractedGroup>,
    /// 非致命信息(批量模式: 孤儿卷 / 非包文件 / mapping 异常)
    pub warnings: Vec<String>,
    /// 回调要求取消而提前结束(组间生效)
    pub cancelled: bool,
}

/// 依次解压全部组: 先对每组做预检(5.1), 全部通过后再逐组解压 ——
/// 避免解了一半才发现后面的组冲突。取消两个生效点:组间由回调 Break 生效;
/// 组内由 `cancel` 置位 kill 解压进程, 本组按清单回滚后停止(与 compress 语义一致)。
/// warnings 作为报告初值(批量扫描的分类警告原样透传)。
#[allow(clippy::too_many_arguments)]
pub(crate) fn extract_groups<B: ExtractBackend + ?Sized>(
    be: &B,
    groups: &[ExtractGroup],
    dest: &Path,
    password: Option<&str>,
    force: bool,
    warnings: Vec<String>,
    cancel: &CancelFlag,
    on_progress: &dyn Fn(&Progress) -> ControlFlow<()>,
) -> Result<ExtractReport> {
    // B2 全量预检: 入口卷存在 → dest 可用 → list → zip-slip → 冲突(--force 跳过)
    let mut listings: Vec<ArchiveListing> = Vec::with_capacity(groups.len());
    for g in groups {
        listings.push(precheck_one(be, &g.resolved, dest, force)?);
    }

    // B3 逐组解压(顺序执行: 机械盘/USB 上顺序 IO 优先)
    let total = groups.len();
    let mut report = ExtractReport {
        warnings,
        ..ExtractReport::default()
    };
    for (i, g) in groups.iter().enumerate() {
        let entry = g
            .original
            .clone()
            .unwrap_or_else(|| file_name_of(&g.resolved.path));
        let fire = |done: usize| -> ControlFlow<()> {
            on_progress(&Progress {
                phase: Phase::Extracting,
                done,
                total,
                done_bytes: 0,
                total_bytes: 0,
                entry: entry.clone(),
            })
        };
        if fire(i) == ControlFlow::Break(()) {
            report.cancelled = true;
            break;
        }
        if let Err(err) = be.extract(g.resolved.backend, &g.resolved.path, dest, password, cancel) {
            // 失败按清单清理本次解出的文件(--force 无法区分新旧, 不清理)
            if !force {
                cleanup_extracted(&listings[i].names, dest);
            }
            if matches!(err, crate::CoreError::Cancelled) {
                // 进程内取消: 本组回滚(不计入), 剩余组不再开始
                report.cancelled = true;
                break;
            }
            return Err(err);
        }
        let listing = &listings[i];
        report.groups.push(ExtractedGroup {
            entry: file_name_of(&g.resolved.path),
            original: g.original.clone(),
            files: listing.files,
        });
        if fire(i + 1) == ControlFlow::Break(()) {
            report.cancelled = true;
            break;
        }
    }
    Ok(report)
}

/// 单组的 Step 2 预检(5.1): 入口卷存在 → dest 可用 → list → zip-slip → 冲突
fn precheck_one<B: ExtractBackend + ?Sized>(
    be: &B,
    resolved: &ResolvedArchive,
    dest: &Path,
    force: bool,
) -> Result<ArchiveListing> {
    if !resolved.path.is_file() {
        return Err(crate::CoreError::ArchiveNotFound(
            resolved.path.to_path_buf(),
        ));
    }
    // dest: 存在但不是目录 → 报错; 不存在 → 创建(核心职责, 不依赖后端隐式行为)
    if dest.symlink_metadata().is_ok_and(|m| !m.is_dir()) {
        return Err(crate::CoreError::DestNotADirectory(dest.to_path_buf()));
    }
    std::fs::create_dir_all(dest)?;
    let listing = be.list(resolved.backend, &resolved.path)?;
    check_zip_slip(&listing.names)?;
    if !force {
        check_conflicts(&listing.names, dest)?;
    }
    Ok(listing)
}

/// 6.3 失败清理: 预检通过 ⇒ 清单路径解压前都不存在 ⇒ 现在存在的都是本次解出的。
/// best-effort: 单个删除失败忽略, 不吞掉原始后端错误(由调用方返回)。
fn cleanup_extracted(names: &[String], dest: &Path) {
    // 深度降序: 先删文件与深层条目, 再删祖先目录
    let mut sorted: Vec<&String> = names.iter().collect();
    sorted.sort_by_key(|n| std::cmp::Reverse(n.matches('/').count()));
    for name in sorted {
        let p = dest.join(name.trim_end_matches('/'));
        if name.ends_with('/') {
            let _ = std::fs::remove_dir_all(p);
        } else {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// 路径的文件名字符串形式(进度/报告显示用)
fn file_name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 7.3 Step B1 扫描分组: 输入目录顶层(不递归)分类出压缩包组(UI 分阶段入口)。
/// `X.zip` / `X.zip.001` / `X.7z` / `X.7z.001` 是入口卷; `.zNN` / `.zip.NNN` / `.7z.NNN` 是
/// 成员卷(入口卷存在则静默, 否则为孤儿卷记 issue); `mapping.csv` 是元数据; 其余文件记 issue。
/// 排序: 有 mapping.csv 按 mapping 行序(= 原条目顺序), 否则按入口卷名排序。
/// 每组补 size(组内各卷大小之和); 逐文件一次 stat, 顶层量级可忽略。
pub fn scan_groups(input: &Path) -> Result<ExtractScan> {
    let mut issues = Vec::new();
    let mut names: Vec<String> = std::fs::read_dir(input)?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort_by_key(|n| n.clone());

    let mut mapping: Vec<(String, String)> = Vec::new();
    let mut meta: Option<MetaFile> = None;
    let mut zip_entries: Vec<String> = Vec::new(); // X.zip(整包或 zip -s 末卷)
    let mut sev_entries: Vec<String> = Vec::new(); // X.zip.001 / X.7z / X.7z.001(强制 7z 系后端)
    let mut members: Vec<(String, String)> = Vec::new(); // (卷名, 期望的入口卷名, 小写)
    let mut others: Vec<String> = Vec::new(); // 非包文件
    // 入口卷名(小写) → 组内各卷大小之和(成员卷按 expected 归组)
    let mut sizes: std::collections::HashMap<String, u64> = std::collections::HashMap::new();

    for name in &names {
        let lower = name.to_lowercase();
        let size = std::fs::symlink_metadata(input.join(name))
            .map(|m| m.len())
            .unwrap_or(0);
        if lower == "mapping.csv" {
            match read_mapping(&input.join(name)) {
                Ok(rows) => {
                    meta = Some(MetaFile {
                        name: name.clone(),
                        size,
                        rows: rows.len(),
                    });
                    mapping = rows;
                }
                Err(e) => issues.push(ScanIssue {
                    name: name.clone(),
                    message: e,
                }),
            }
        } else if lower.ends_with(".zip") {
            zip_entries.push(name.clone());
            *sizes.entry(lower).or_default() += size;
        } else if lower.ends_with(".zip.001")
            || lower.ends_with(".7z")
            || lower.ends_with(".7z.001")
        {
            sev_entries.push(name.clone());
            *sizes.entry(lower).or_default() += size;
        } else if let Some(expected) = member_entry_volume(&lower) {
            members.push((name.clone(), expected.clone()));
            *sizes.entry(expected).or_default() += size;
        } else {
            others.push(name.clone());
        }
    }

    // 孤儿卷检查(大小写不敏感: 兄弟卷在 NTFS 上大小写可能不一致)
    let entries_lower: Vec<String> = zip_entries
        .iter()
        .chain(&sev_entries)
        .map(|n| n.to_lowercase())
        .collect();
    for (vol, expected) in &members {
        if !entries_lower.contains(expected) {
            issues.push(ScanIssue {
                name: vol.clone(),
                message: format!("missing {expected}"),
            });
        }
    }
    for f in others {
        issues.push(ScanIssue {
            name: f,
            message: "skipped non-archive file".into(),
        });
    }

    // 组装组(入口卷 = 主卷文件名, 与 mapping.csv 的 archive 列一致)
    let mut groups: Vec<ExtractGroup> = Vec::new();
    for name in zip_entries.iter().chain(&sev_entries) {
        let backend = if name.to_lowercase().ends_with(".zip") {
            zip_entry_backend(&input.join(name))
        } else {
            Backend::SevenZip
        };
        let size = sizes.remove(&name.to_lowercase()).unwrap_or(0);
        groups.push(ExtractGroup {
            resolved: ResolvedArchive {
                path: input.join(name),
                backend,
            },
            original: None,
            size,
        });
    }
    if groups.is_empty() {
        return Err(crate::CoreError::NoArchives(input.to_path_buf()));
    }

    // 排序与原名填充
    if mapping.is_empty() {
        groups.sort_by_key(|g| file_name_of(&g.resolved.path).to_lowercase());
    } else {
        let row_of = |g: &ExtractGroup| -> Option<usize> {
            let fname = file_name_of(&g.resolved.path);
            mapping
                .iter()
                .position(|(a, _)| a.eq_ignore_ascii_case(&fname))
        };
        groups.sort_by_key(|g| match row_of(g) {
            Some(i) => (0u8, i, String::new()),
            None => (1u8, 0usize, file_name_of(&g.resolved.path).to_lowercase()),
        });
        for g in &mut groups {
            let fname = file_name_of(&g.resolved.path);
            if let Some((_, orig)) = mapping.iter().find(|(a, _)| a.eq_ignore_ascii_case(&fname)) {
                g.original = Some(orig.clone());
            }
        }
        for (a, _) in &mapping {
            if !groups
                .iter()
                .any(|g| file_name_of(&g.resolved.path).eq_ignore_ascii_case(a))
            {
                issues.push(ScanIssue {
                    name: "mapping.csv".into(),
                    message: format!("archive not found: {a}"),
                });
            }
        }
    }
    Ok(ExtractScan {
        groups,
        meta,
        issues,
    })
}

/// 成员卷对应的入口卷名(小写): `x.znn` → `x.zip`; `x.zip.nnn` / `x.7z.nnn`(nnn≠001)→ `{stem}.001`;
/// 非成员卷 → None。
fn member_entry_volume(lower: &str) -> Option<String> {
    let (stem, ext) = lower.rsplit_once('.')?;
    if ext.len() >= 2 && ext.starts_with('z') && is_digits(&ext[1..]) {
        return Some(format!("{stem}.zip"));
    }
    if is_digits(ext) && ext != "001" {
        return Some(format!("{stem}.001"));
    }
    None
}

/// 读 mapping.csv(表头 `archive,original`), 返回 (主卷文件名, 原条目名) 行
fn read_mapping(path: &Path) -> std::result::Result<Vec<(String, String)>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut rows = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if i == 0 && line.trim().eq_ignore_ascii_case("archive,original") {
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let Some((a, o)) = line.split_once(',') else {
            return Err(format!("bad row {}: {line:?}", i + 1));
        };
        rows.push((a.trim().to_string(), o.trim().to_string()));
    }
    Ok(rows)
}

#[cfg(test)]
mod orch_tests {
    use super::*;
    use crate::backend::{ArchiveListing, ExtractBackend, RealExtract};
    use crate::progress::Phase;
    use crate::testutil::unique_temp;
    use std::cell::RefCell;
    use std::fs;
    use std::ops::ControlFlow;

    use crate::CoreError;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// 假后端:list 返回 canned 清单; extract 模拟解出 writes 文件后视 fail 决定成败
    struct FakeExtract {
        listing: Vec<String>,
        writes: Vec<String>,
        fail: bool,
        /// extract 时写出文件后返回 Err(Cancelled), 模拟"进程被 kill"
        cancel: bool,
        calls: RefCell<Vec<&'static str>>,
    }

    impl FakeExtract {
        fn new(listing: &[&str], writes: &[&str], fail: bool) -> Self {
            Self {
                listing: s(listing),
                writes: s(writes),
                fail,
                cancel: false,
                calls: RefCell::new(Vec::new()),
            }
        }

        fn new_cancelling(listing: &[&str], writes: &[&str]) -> Self {
            Self {
                listing: s(listing),
                writes: s(writes),
                fail: false,
                cancel: true,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl ExtractBackend for FakeExtract {
        fn program(&self, _kind: Backend) -> &'static str {
            "fake"
        }

        fn list(&self, _kind: Backend, _entry: &Path) -> Result<ArchiveListing> {
            self.calls.borrow_mut().push("list");
            let files = self.listing.iter().filter(|n| !n.ends_with('/')).count();
            Ok(ArchiveListing {
                names: self.listing.clone(),
                files,
            })
        }

        fn extract(
            &self,
            _kind: Backend,
            _entry: &Path,
            dest: &Path,
            _pw: Option<&str>,
            _cancel: &crate::CancelFlag,
        ) -> Result<()> {
            self.calls.borrow_mut().push("extract");
            for w in &self.writes {
                let p = dest.join(w);
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(&p, b"x").unwrap();
            }
            if self.cancel {
                Err(CoreError::Cancelled)
            } else if self.fail {
                Err(CoreError::BackendFailed {
                    program: "fake".into(),
                    status: "exit status: 1".into(),
                    output: "boom".into(),
                })
            } else {
                Ok(())
            }
        }
    }

    /// root/B.zip 占位文件 + 指向它的组
    fn one_group(tag: &str) -> (PathBuf, ExtractGroup) {
        let root = unique_temp(tag);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("B.zip"), b"").unwrap();
        let g = ExtractGroup {
            resolved: ResolvedArchive {
                path: root.join("B.zip"),
                backend: Backend::native(),
            },
            original: None,
            size: 0,
        };
        (root, g)
    }

    fn noop(_: &Progress) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }

    #[test]
    fn single_group_happy_path_reports_files_and_progress() {
        let (root, g) = one_group("ex-happy");
        let be = FakeExtract::new(
            &["Novel/", "Novel/book.txt", "photo.jpg"],
            &["Novel/book.txt", "photo.jpg"],
            false,
        );
        let dest = root.join("restore");
        let events = RefCell::new(Vec::new());
        let report = extract_groups(
            &be,
            &[g],
            &dest,
            None,
            false,
            Vec::new(),
            &CancelFlag::new(),
            &|p| {
                events.borrow_mut().push(p.clone());
                ControlFlow::Continue(())
            },
        )
        .unwrap();
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].entry, "B.zip");
        assert_eq!(report.groups[0].files, 2, "files 计目录外的条目");
        assert_eq!(report.groups[0].original, None);
        assert!(
            dest.join("Novel/book.txt").exists(),
            "extract 应被调用且文件解出"
        );
        assert!(report.warnings.is_empty());
        assert!(!report.cancelled);

        let ev = events.borrow();
        assert_eq!(ev.len(), 2, "每组开始/结束各一次: {ev:?}");
        assert_eq!(ev[0].phase, Phase::Extracting);
        assert_eq!((ev[0].done, ev[0].total), (0, 1));
        assert_eq!((ev[1].done, ev[1].total), (1, 1));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_archive_fails_before_any_backend_call() {
        let (root, g) = one_group("ex-missing");
        fs::remove_file(&g.resolved.path).unwrap();
        let be = FakeExtract::new(&[], &[], false);
        let err = extract_groups(
            &be,
            &[g],
            &root.join("r"),
            None,
            false,
            Vec::new(),
            &CancelFlag::new(),
            &noop,
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::ArchiveNotFound(_)));
        assert!(be.calls.borrow().is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn dest_existing_file_is_rejected_before_list() {
        let (root, g) = one_group("ex-destfile");
        let dest = root.join("dest");
        fs::write(&dest, b"f").unwrap();
        let be = FakeExtract::new(&[], &[], false);
        let err = extract_groups(
            &be,
            &[g],
            &dest,
            None,
            false,
            Vec::new(),
            &CancelFlag::new(),
            &noop,
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::DestNotADirectory(_)));
        assert!(be.calls.borrow().is_empty(), "dest 检查应在 list 之前");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn conflict_rejects_before_any_extract() {
        // 两段式: 预检阶段发现冲突时, 任何组都还没解压
        let (root, g) = one_group("ex-conflict");
        let dest = root.join("restore");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("photo.jpg"), b"old").unwrap();
        let be = FakeExtract::new(&["photo.jpg"], &["photo.jpg"], false);
        let err = extract_groups(
            &be,
            &[g],
            &dest,
            None,
            false,
            Vec::new(),
            &CancelFlag::new(),
            &noop,
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::ExtractConflict(_)));
        assert_eq!(*be.calls.borrow(), ["list"], "只应 list, 不应 extract");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn force_skips_conflict_check_and_extracts() {
        let (root, g) = one_group("ex-force");
        let dest = root.join("restore");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("photo.jpg"), b"old").unwrap();
        let be = FakeExtract::new(&["photo.jpg"], &["photo.jpg"], false);
        extract_groups(
            &be,
            &[g],
            &dest,
            None,
            true,
            Vec::new(),
            &CancelFlag::new(),
            &noop,
        )
        .unwrap();
        assert_eq!(*be.calls.borrow(), ["list", "extract"]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn zip_slip_rejects_before_any_extract() {
        let (root, g) = one_group("ex-slip");
        let be = FakeExtract::new(&["../evil.txt"], &["../evil.txt"], false);
        let err = extract_groups(
            &be,
            &[g],
            &root.join("r"),
            None,
            false,
            Vec::new(),
            &CancelFlag::new(),
            &noop,
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::UnsafeArchive { .. }));
        assert_eq!(*be.calls.borrow(), ["list"]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn backend_failure_cleans_extracted_files() {
        let (root, g) = one_group("ex-clean");
        let be = FakeExtract::new(
            &["Novel/", "Novel/book.txt", "photo.jpg"],
            &["Novel/book.txt", "photo.jpg"],
            true,
        );
        let dest = root.join("restore");
        let err = extract_groups(
            &be,
            &[g],
            &dest,
            None,
            false,
            Vec::new(),
            &CancelFlag::new(),
            &noop,
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::BackendFailed { .. }));
        assert!(
            !dest.exists() || fs::read_dir(&dest).unwrap().next().is_none(),
            "本次解出的文件应被按清单清理"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn force_failure_keeps_files_no_cleanup() {
        // --force 模式无法区分"本次解出"与"本来就存在", 不清理
        let (root, g) = one_group("ex-keep");
        let be = FakeExtract::new(&["photo.jpg"], &["photo.jpg"], true);
        let dest = root.join("restore");
        fs::create_dir_all(&dest).unwrap();
        let r = extract_groups(
            &be,
            &[g],
            &dest,
            None,
            true,
            Vec::new(),
            &CancelFlag::new(),
            &noop,
        );
        assert!(r.is_err());
        assert!(dest.join("photo.jpg").exists(), "--force 失败不应清理");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn tool_missing_reported_when_program_absent() {
        let (root, g) = one_group("ex-tool");
        let be = RealExtract {
            program_override: Some("no-such-tool-xyz-42"),
        };
        let err = extract_groups(
            &be,
            &[g],
            &root.join("r"),
            None,
            false,
            Vec::new(),
            &CancelFlag::new(),
            &noop,
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::ToolMissing(_)));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cancel_at_start_event_skips_group() {
        let (root, g) = one_group("ex-cancel-start");
        let be = FakeExtract::new(&["a.txt"], &["a.txt"], false);
        let report = extract_groups(
            &be,
            &[g],
            &root.join("r"),
            None,
            false,
            Vec::new(),
            &CancelFlag::new(),
            &|_| ControlFlow::Break(()),
        )
        .unwrap();
        assert!(report.cancelled);
        assert!(report.groups.is_empty(), "取消在组开始前生效");
        assert_eq!(*be.calls.borrow(), ["list"], "预检已发生, 解压未开始");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cancel_after_first_group_counts_it_and_stops() {
        let (root, g1) = one_group("ex-cancel-end");
        let g2 = ExtractGroup {
            resolved: ResolvedArchive {
                path: root.join("B.zip"),
                backend: Backend::SevenZip,
            },
            original: None,
            size: 0,
        };
        let be = FakeExtract::new(&["a.txt"], &["a.txt"], false);
        let seen = RefCell::new(0);
        let report = extract_groups(
            &be,
            &[g1, g2],
            &root.join("r"),
            None,
            false,
            Vec::new(),
            &CancelFlag::new(),
            &|_| {
                *seen.borrow_mut() += 1;
                if *seen.borrow() >= 2 {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            },
        )
        .unwrap();
        // 事件序列: start1(continue) → end1(break → cancelled); 第二组未开始
        assert!(report.cancelled);
        assert_eq!(report.groups.len(), 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn process_cancel_rolls_back_group_and_stops_batch() {
        let (root, g1) = one_group("ex-kill");
        let g2 = ExtractGroup {
            resolved: ResolvedArchive {
                path: root.join("B.zip"),
                backend: Backend::SevenZip,
            },
            original: None,
            size: 0,
        };
        let be = FakeExtract::new_cancelling(&["Novel/a.txt"], &["Novel/a.txt"]);
        let dest = root.join("restore");
        let report = extract_groups(
            &be,
            &[g1, g2],
            &dest,
            None,
            false,
            Vec::new(),
            &CancelFlag::new(),
            &noop,
        )
        .unwrap();

        // Ok 而非 Err: 取消是正常终态; 本组已解文件回滚, 下一组不开始
        assert!(report.cancelled);
        assert!(
            report.groups.is_empty(),
            "被杀组不应计入: {:?}",
            report.groups
        );
        assert_eq!(
            *be.calls.borrow(),
            ["list", "list", "extract"],
            "第二组不应 extract"
        );
        assert!(
            !dest.join("Novel/a.txt").exists(),
            "被杀组已解出的文件应按清单回滚"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn process_cancel_with_force_keeps_files_but_marks_cancelled() {
        let (root, g1) = one_group("ex-kill-force");
        let be = FakeExtract::new_cancelling(&["a.txt"], &["a.txt"]);
        let dest = root.join("restore");
        let report = extract_groups(
            &be,
            &[g1],
            &dest,
            None,
            true,
            Vec::new(),
            &CancelFlag::new(),
            &noop,
        )
        .unwrap();

        assert!(report.cancelled);
        assert!(report.groups.is_empty());
        assert!(dest.join("a.txt").exists(), "--force 无法区分新旧, 不回滚");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn public_entry_accepts_single_file_input() {
        let root = unique_temp("api-file");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("B.zip"), b"").unwrap();
        let be = FakeExtract::new(&["a.txt"], &["a.txt"], false);
        let report = extract_with_backend(
            &be,
            &root.join("B.zip"),
            &root.join("restore"),
            None,
            false,
            &CancelFlag::new(),
            &noop,
        )
        .unwrap();
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].entry, "B.zip");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn public_entry_dispatches_directory_to_batch_scan() {
        let root = unique_temp("api-dir");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("A.zip"), b"").unwrap();
        fs::write(root.join("B.zip"), b"").unwrap();
        fs::write(root.join("notes.txt"), b"").unwrap();
        let be = FakeExtract::new(&["a.txt"], &["a.txt"], false);
        let report = extract_with_backend(
            &be,
            &root,
            &root.join("restore"),
            None,
            false,
            &CancelFlag::new(),
            &noop,
        )
        .unwrap();
        assert_eq!(report.groups.len(), 2);
        assert_eq!(report.groups[0].entry, "A.zip");
        assert_eq!(report.groups[1].entry, "B.zip");
        assert!(
            report.warnings.iter().any(|w| w.contains("notes.txt")),
            "扫描 warning 应进报告: {:?}",
            report.warnings
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn public_entry_empty_dir_is_no_archives() {
        let root = unique_temp("api-empty");
        fs::create_dir_all(&root).unwrap();
        let be = FakeExtract::new(&[], &[], false);
        let err = extract_with_backend(
            &be,
            &root,
            &root.join("r"),
            None,
            false,
            &CancelFlag::new(),
            &noop,
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::NoArchives(_)));
        fs::remove_dir_all(&root).unwrap();
    }
}

#[cfg(test)]
mod precheck_tests {
    use super::*;
    use crate::testutil::unique_temp;
    use std::fs;

    use crate::CoreError;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn zip_slip_accepts_normal_relative_paths() {
        check_zip_slip(&names(&["Novel/", "Novel/book.txt", "photo.jpg"])).unwrap();
    }

    #[test]
    fn zip_slip_rejects_parent_references() {
        let e = check_zip_slip(&names(&["a/b.txt", "../evil.txt"])).unwrap_err();
        match e {
            CoreError::UnsafeArchive { path, reason } => {
                assert_eq!(path, "../evil.txt");
                assert!(reason.contains(".."), "{reason}");
            }
            o => panic!("应为 UnsafeArchive: {o:?}"),
        }
    }

    #[test]
    fn zip_slip_rejects_nested_parent_and_absolute_paths() {
        // 平台无关: Linux 上也要拒绝包里的 "C:/x" 这类 Windows 绝对路径
        for bad in ["a/../../x", "/etc/passwd", "C:/Windows/x"] {
            let e = check_zip_slip(&names(&[bad])).unwrap_err();
            assert!(matches!(e, CoreError::UnsafeArchive { .. }), "{bad} -> {e}");
        }
    }

    #[test]
    fn conflict_check_passes_when_dest_is_empty() {
        let d = unique_temp("conflict-empty");
        fs::create_dir_all(&d).unwrap();
        check_conflicts(&names(&["Novel/", "Novel/book.txt"]), &d).unwrap();
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn conflict_check_lists_existing_paths_only() {
        let d = unique_temp("conflict-hit");
        fs::create_dir_all(d.join("Novel")).unwrap();
        fs::write(d.join("Novel/book.txt"), "old").unwrap();

        let e =
            check_conflicts(&names(&["Novel/", "Novel/book.txt", "photo.jpg"]), &d).unwrap_err();
        match e {
            CoreError::ExtractConflict(paths) => {
                assert!(paths.contains(&"Novel/book.txt".to_string()), "{paths:?}");
                assert!(!paths.contains(&"photo.jpg".to_string()), "{paths:?}");
            }
            o => panic!("应为 ExtractConflict: {o:?}"),
        }
        fs::remove_dir_all(&d).unwrap();
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::*;
    use crate::backend::Backend;
    use crate::testutil::unique_temp;
    use std::fs;

    use crate::CoreError;

    #[test]
    fn plain_zip_resolves_to_platform_native_backend() {
        let r = resolve_archive(Path::new("somewhere/B.zip")).unwrap();
        assert_eq!(r.path, Path::new("somewhere/B.zip"));
        assert_eq!(r.backend, Backend::native());
    }

    #[test]
    fn sevenz_single_resolves_to_sevenzip_forced() {
        let r = resolve_archive(Path::new("somewhere/B.7z")).unwrap();
        assert_eq!(r.path, Path::new("somewhere/B.7z"));
        assert_eq!(r.backend, Backend::SevenZip, "7z 格式只能 7z 系解");
    }

    #[test]
    fn sevenz_extension_is_case_insensitive() {
        let r = resolve_archive(Path::new("somewhere/B.7Z")).unwrap();
        assert_eq!(r.backend, Backend::SevenZip);
    }

    #[test]
    fn sevenz_member_volume_normalizes_to_first_volume() {
        let d = unique_temp("resolve-7z");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("B.7z.001"), b"").unwrap();
        let r = resolve_archive(&d.join("B.7z.002")).unwrap();
        assert_eq!(r.path, d.join("B.7z.001"));
        assert_eq!(r.backend, Backend::SevenZip);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn sevenz_member_volume_without_first_volume_reports_missing() {
        let err = resolve_archive(Path::new("somewhere/B.7z.002")).unwrap_err();
        assert!(
            matches!(err, CoreError::EntryVolumeMissing { .. }),
            "实际: {err:?}"
        );
    }

    #[test]
    fn first_7z_volume_resolves_with_forced_sevenzip() {
        let r = resolve_archive(Path::new("somewhere/B.zip.001")).unwrap();
        assert_eq!(r.path, Path::new("somewhere/B.zip.001"));
        assert_eq!(r.backend, Backend::SevenZip);
    }

    #[test]
    fn z01_volume_resolves_to_sibling_zip() {
        let d = unique_temp("res-z01");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("B.z01"), b"").unwrap();
        fs::write(d.join("B.zip"), b"").unwrap();

        let r = resolve_archive(&d.join("B.z01")).unwrap();
        assert_eq!(r.path, d.join("B.zip"));
        // 入口卷旁有 .z01 = zip -s 分卷组 → 只能 7z 系解(见 zip_entry_backend)
        assert_eq!(r.backend, Backend::SevenZip);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn zip_with_z01_sibling_routes_to_sevenzip_even_on_zip_platform() {
        // .zip 单看扩展名是平台默认后端, 但旁有 .z01 兄弟卷即 zip -s 分卷组,
        // 两平台都强制 SevenZip(unzip 读不了分卷组)
        let d = unique_temp("res-zip-split");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("B.zip"), b"").unwrap();
        assert_eq!(
            resolve_archive(&d.join("B.zip")).unwrap().backend,
            Backend::native(),
            "无 .z01 兄弟卷 = 整包 → 平台默认"
        );
        fs::write(d.join("B.z01"), b"").unwrap();
        assert_eq!(
            resolve_archive(&d.join("B.zip")).unwrap().backend,
            Backend::SevenZip,
            "有 .z01 兄弟卷 = 分卷组 → 强制 SevenZip"
        );
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn plain_zip_with_stray_z01_still_extracts_via_sevenzip() {
        // 整包旁残留同名 .z01(非分卷组)会误判为分卷组路由 7z —— 无害:
        // 7z 能读普通 zip, 解压结果正确。固化该设计, 防止将来收紧路由时无提示破坏。
        let d = unique_temp("res-stray-z01");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("B.zip"), b"").unwrap();
        fs::write(d.join("B.z01"), b"").unwrap();
        assert_eq!(
            resolve_archive(&d.join("B.zip")).unwrap().backend,
            Backend::SevenZip,
            "误判为分卷组仍走 7z, 行为不变"
        );
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn z01_without_sibling_zip_reports_missing_entry_volume() {
        let d = unique_temp("res-z01-missing");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("B.z01"), b"").unwrap();

        let err = resolve_archive(&d.join("B.z01")).unwrap_err();
        match err {
            CoreError::EntryVolumeMissing { got, expected } => {
                assert_eq!(got, d.join("B.z01"));
                assert_eq!(expected, d.join("B.zip"));
            }
            other => panic!("应为 EntryVolumeMissing: {other:?}"),
        }
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn later_7z_volume_resolves_to_first_volume() {
        let d = unique_temp("res-002");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("B.zip.001"), b"").unwrap();
        fs::write(d.join("B.zip.002"), b"").unwrap();

        let r = resolve_archive(&d.join("B.zip.002")).unwrap();
        assert_eq!(r.path, d.join("B.zip.001"));
        assert_eq!(r.backend, Backend::SevenZip);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn unknown_extension_is_rejected() {
        let err = resolve_archive(Path::new("somewhere/B.rar")).unwrap_err();
        assert!(matches!(err, CoreError::UnsupportedArchive(_)));
    }

    #[test]
    fn extensions_are_case_insensitive() {
        let d = unique_temp("res-case");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("B.Z01"), b"").unwrap();
        fs::write(d.join("B.zip"), b"").unwrap();

        assert_eq!(
            resolve_archive(&d.join("B.ZIP")).unwrap().path,
            d.join("B.ZIP")
        );
        assert_eq!(
            resolve_archive(&d.join("B.Z01")).unwrap().path,
            d.join("B.zip")
        );
        // B.zip.001 不存在 → EntryVolumeMissing
        assert!(matches!(
            resolve_archive(&d.join("B.ZIP.002")).unwrap_err(),
            CoreError::EntryVolumeMissing { .. }
        ));
        fs::remove_dir_all(&d).unwrap();
    }
}

/// 批量模式 Step B1:扫描分组(7.3)
#[cfg(test)]
mod batch_tests {
    use super::*;
    use crate::backend::Backend;
    use crate::testutil::unique_temp;
    use std::fs;

    use crate::CoreError;

    fn touch(d: &Path, name: &str) {
        fs::write(d.join(name), b"").unwrap();
    }

    fn entry_names(groups: &[ExtractGroup]) -> Vec<String> {
        groups
            .iter()
            .map(|g| file_name_of(&g.resolved.path))
            .collect()
    }

    /// 组名(入口卷文件名)排序收集(复用既有 entry_names)
    fn sorted_group_names(scan: &ExtractScan) -> Vec<String> {
        let mut v = entry_names(&scan.groups);
        v.sort();
        v
    }

    #[test]
    fn scan_groups_classifies_7z_entries_and_mixed_dirs() {
        let d = unique_temp("sg-7z");
        fs::create_dir_all(&d).unwrap();
        touch(&d, "Novel.7z"); // 7z 整包
        touch(&d, "media.7z.001"); // 7z 分卷首卷
        touch(&d, "media.7z.002"); // 成员卷(静默)
        touch(&d, "old.zip"); // zip 组混合
        let scan = scan_groups(&d).unwrap();

        assert_eq!(
            sorted_group_names(&scan),
            ["Novel.7z", "media.7z.001", "old.zip"]
        );
        for g in &scan.groups {
            let n = file_name_of(&g.resolved.path);
            let want = if n.ends_with(".zip") {
                Backend::native()
            } else {
                Backend::SevenZip
            };
            assert_eq!(g.resolved.backend, want, "{n} 的后端");
        }
        // media 组大小 = 两个卷之和
        let media = scan
            .groups
            .iter()
            .find(|g| file_name_of(&g.resolved.path) == "media.7z.001")
            .unwrap();
        assert_eq!(media.size, 0, "touch 的文件为空, 大小 0");
        assert!(scan.issues.is_empty(), "成员卷应静默: {:?}", scan.issues);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn scan_groups_reports_orphan_7z_volume() {
        let d = unique_temp("sg-7z-orphan");
        fs::create_dir_all(&d).unwrap();
        touch(&d, "media.7z.002"); // 无 .7z.001 → 孤儿卷
        touch(&d, "Novel.7z");
        let scan = scan_groups(&d).unwrap();
        assert_eq!(sorted_group_names(&scan), ["Novel.7z"]);
        assert!(
            scan.issues
                .iter()
                .any(|i| i.name == "media.7z.002" && i.message.contains("media.7z.001")),
            "{:?}",
            scan.issues
        );
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn scan_groups_classifies_entry_volumes_and_ignores_members() {
        let d = unique_temp("batch-classify");
        fs::create_dir_all(&d).unwrap();
        touch(&d, "Novel.zip"); // zip 组
        touch(&d, "media.zip.001"); // 7z 组
        touch(&d, "media.zip.002"); // 成员卷, 静默忽略
        touch(&d, "big.z01"); // 成员卷(zip -s)
        touch(&d, "big.zip"); // zip 组(big)
        touch(&d, "mapping.csv"); // 元数据, 不算非包文件

        let scan = scan_groups(&d).unwrap();
        assert_eq!(
            entry_names(&scan.groups),
            ["big.zip", "media.zip.001", "Novel.zip"],
            "无 mapping 时按名排序(小写比较, 跨平台稳定)"
        );
        let media = scan
            .groups
            .iter()
            .find(|g| g.resolved.path.ends_with("media.zip.001"))
            .unwrap();
        assert_eq!(media.resolved.backend, Backend::SevenZip);
        let big = scan
            .groups
            .iter()
            .find(|g| g.resolved.path.ends_with("big.zip"))
            .unwrap();
        assert_eq!(
            big.resolved.backend,
            Backend::SevenZip,
            "big 旁有 .z01 成员卷 = zip -s 分卷组 → 强制 SevenZip"
        );
        let novel = scan
            .groups
            .iter()
            .find(|g| g.resolved.path.ends_with("Novel.zip"))
            .unwrap();
        assert_eq!(novel.resolved.backend, Backend::native());
        assert!(scan.issues.is_empty(), "{scan:?}");
        assert!(scan.groups.iter().all(|g| g.original.is_none()));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn scan_groups_flags_orphan_volumes_as_issues() {
        let d = unique_temp("batch-orphan");
        fs::create_dir_all(&d).unwrap();
        touch(&d, "B.z01"); // 无 B.zip → 孤儿
        touch(&d, "C.zip.003"); // 无 C.zip.001 → 孤儿
        touch(&d, "D.zip"); // 正常组

        let scan = scan_groups(&d).unwrap();
        assert_eq!(entry_names(&scan.groups), ["D.zip"]);
        assert_eq!(scan.issues.len(), 2, "{scan:?}");
        assert!(scan.issues.iter().any(|i| i.name == "B.z01"));
        assert!(scan.issues.iter().any(|i| i.name == "C.zip.003"));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn scan_groups_warns_for_non_archive_files() {
        let d = unique_temp("batch-other");
        fs::create_dir_all(&d).unwrap();
        touch(&d, "A.zip");
        touch(&d, "notes.txt");

        let scan = scan_groups(&d).unwrap();
        assert_eq!(entry_names(&scan.groups), ["A.zip"]);
        assert_eq!(scan.issues.len(), 1);
        assert_eq!(scan.issues[0].name, "notes.txt");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn scan_groups_orders_by_mapping_and_fills_originals() {
        let d = unique_temp("batch-mapping");
        fs::create_dir_all(&d).unwrap();
        touch(&d, "zt_a.zip");
        touch(&d, "zt_b.zip");
        fs::write(
            d.join("mapping.csv"),
            "archive,original\nzt_b.zip,Novel\nzt_a.zip,media\n",
        )
        .unwrap();

        let scan = scan_groups(&d).unwrap();
        assert_eq!(
            entry_names(&scan.groups),
            ["zt_b.zip", "zt_a.zip"],
            "按 mapping 行序(= 原条目顺序)"
        );
        assert_eq!(scan.groups[0].original.as_deref(), Some("Novel"));
        assert_eq!(scan.groups[1].original.as_deref(), Some("media"));
        assert!(scan.issues.is_empty(), "{scan:?}");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn scan_groups_reports_mapping_meta() {
        let d = unique_temp("batch-meta");
        fs::create_dir_all(&d).unwrap();
        touch(&d, "zt_a.zip");
        let csv = "archive,original\nzt_a.zip,Novel\n";
        fs::write(d.join("mapping.csv"), csv).unwrap();

        let scan = scan_groups(&d).unwrap();
        let meta = scan.meta.expect("有 mapping.csv 时应返回 meta");
        assert_eq!(meta.name, "mapping.csv");
        assert_eq!(meta.rows, 1);
        assert_eq!(meta.size, csv.len() as u64);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn scan_groups_mapping_read_failure_degrades_to_issue_and_name_order() {
        let d = unique_temp("batch-mapping-bad");
        fs::create_dir_all(&d).unwrap();
        touch(&d, "zt_a.zip");
        fs::write(d.join("mapping.csv"), "not-csv\n").unwrap();

        let scan = scan_groups(&d).unwrap();
        assert!(scan.meta.is_none(), "读取失败时无 meta");
        assert!(
            scan.issues.iter().any(|i| i.name == "mapping.csv"),
            "{scan:?}"
        );
        assert_eq!(entry_names(&scan.groups), ["zt_a.zip"], "降级为按名排序");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn scan_groups_sums_volume_sizes_per_group() {
        let d = unique_temp("batch-size");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("Novel.zip"), vec![0u8; 100]).unwrap();
        fs::write(d.join("Novel.z01"), vec![0u8; 50]).unwrap();
        fs::write(d.join("media.zip.001"), vec![0u8; 200]).unwrap();
        fs::write(d.join("media.zip.002"), vec![0u8; 300]).unwrap();

        let scan = scan_groups(&d).unwrap();
        let novel = scan
            .groups
            .iter()
            .find(|g| g.resolved.path.ends_with("Novel.zip"))
            .unwrap();
        assert_eq!(novel.size, 150, "zip -s 组 = 末卷 + 成员卷");
        let media = scan
            .groups
            .iter()
            .find(|g| g.resolved.path.ends_with("media.zip.001"))
            .unwrap();
        assert_eq!(media.size, 500, "7z -v 组 = 首卷 + 成员卷");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn scan_groups_warns_for_mapping_rows_without_archive() {
        let d = unique_temp("batch-mapping-miss");
        fs::create_dir_all(&d).unwrap();
        touch(&d, "zt_a.zip");
        fs::write(
            d.join("mapping.csv"),
            "archive,original\nzt_gone.zip,Novel\nzt_a.zip,media\n",
        )
        .unwrap();

        let scan = scan_groups(&d).unwrap();
        assert_eq!(entry_names(&scan.groups), ["zt_a.zip"]);
        assert!(
            scan.issues
                .iter()
                .any(|i| i.name == "mapping.csv" && i.message.contains("zt_gone.zip")),
            "{scan:?}"
        );
        assert_eq!(scan.groups[0].original.as_deref(), Some("media"));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn scan_groups_empty_dir_is_no_archives() {
        let d = unique_temp("batch-empty");
        fs::create_dir_all(&d).unwrap();
        let err = scan_groups(&d).unwrap_err();
        assert!(matches!(err, CoreError::NoArchives(_)));
        fs::remove_dir_all(&d).unwrap();
    }
}
