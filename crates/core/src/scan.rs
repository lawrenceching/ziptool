//! Step 1 扫描:列出 input 顶层条目(按名字排序)并递归统计字节大小。
//! 大小只用于进度加权与预检, 允许有误差; 扫描进度经回调上报。
//! 流式形态([`scan_dir_stream`]): 顶层条目骨架先回调(UI 可先渲染行), 统计出大小后
//! 逐条回调; 扫描可经 [`CancelFlag`] 在条目/目录边界取消(返回 [`CoreError::Cancelled`])。

use std::fs;
use std::path::Path;

use crate::CancelFlag;
use crate::exclude::{
    ExcludeReason, ExcludeRules, ExcludedEntry, effective_patterns, matches_patterns,
    validate_patterns,
};
use crate::progress::{Phase, Progress};
use crate::{CoreError, Result};

/// 目录树内每处理 N 个条目检查一次取消旗标(取消延迟 ≤ N 次 read_dir/stat)
const CANCEL_CHECK_EVERY: u32 = 512;

/// 一个待压缩的顶层条目(Step 1 产出, Step 2 补全 dst, Step 3 消费)
#[derive(Debug, Clone)]
pub struct Entry {
    /// 条目名(不含路径)
    pub name: String,
    pub kind: EntryKind,
    /// 递归统计的字节大小:文件为自身大小, 目录为其中所有普通文件大小之和
    pub size: u64,
    /// 包内条目数(普通文件 + 目录; 目录自身也计 1; 空目录 = 1)
    pub entry_count: usize,
    /// 压缩包基础名(不带扩展名), 由 Step 2 填充; 扫描后为空串
    pub dst: String,
    /// 压缩包格式("zip"/"7z"), 由 Step 2 按 Options.format 填充; 扫描后为空串
    pub format: String,
    /// 排除标记(仅顶层打标;None = 正常条目)。扫描永远返回全部条目,
    /// CLI 丢弃被标条目, UI 渲染为不勾选
    pub exclude: Option<crate::exclude::ExcludeReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
}

/// 扫描结果
#[derive(Debug, Default)]
pub struct ScanReport {
    /// 顶层条目, 按名字字典序
    pub entries: Vec<Entry>,
    /// 所有条目大小之和
    pub total_bytes: u64,
    /// 扫描中的非致命问题(如无权限子树), 不中断扫描
    pub warnings: Vec<String>,
    /// 被排除的顶层条目名单(CLI 提示用;与 entries 中带 exclude 标记者一致)
    pub excluded: Vec<crate::exclude::ExcludedEntry>,
}

/// 扫描 input 的顶层条目并统计大小; 每完成一个顶层条目回调一次进度。
/// 排除规则(rules)双轨生效: 顶层条目永远返回并打标(`Entry.exclude`,
/// CLI 丢弃 / UI 显示不勾选), 树内被排除项不计尺寸。
/// 排除只认系统元数据名单 + 用户/策略模式; 用户设置的隐藏属性不参与判定。
pub fn scan_dir(
    input: &Path,
    rules: &ExcludeRules,
    on_progress: &dyn Fn(&Progress),
) -> Result<ScanReport> {
    scan_dir_full(
        input,
        rules,
        &CancelFlag::new(),
        &|_| {},
        &|_, _| {},
        on_progress,
    )
}

/// 流式扫描: 顶层条目(排序后)一列出就回调 `on_list` —— 此时文件大小已填、
/// 目录大小仍是 0 —— 之后每完成一个顶层条目的递归统计回调 `on_entry(下标, 条目)`。
/// 下标与 `on_list` 的顺序一致; 最终返回与 [`scan_dir`] 相同的完整报告。
/// 取消经 `cancel` 生效(检查点之间最多再处理 [`CANCEL_CHECK_EVERY`] 个条目),
/// 取消时立即返回 [`CoreError::Cancelled`], 不返回部分结果。
pub fn scan_dir_stream(
    input: &Path,
    rules: &ExcludeRules,
    cancel: &CancelFlag,
    on_list: &dyn Fn(&[Entry]),
    on_entry: &dyn Fn(usize, &Entry),
) -> Result<ScanReport> {
    scan_dir_full(input, rules, cancel, on_list, on_entry, &|_| {})
}

/// 取消检查器: 目录树遍历里按固定粒度轮询旗标(避免每个条目都做原子读)
struct ScanCtl<'a> {
    cancel: &'a CancelFlag,
    ticks: u32,
}

impl<'a> ScanCtl<'a> {
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

    /// 边界级检查(每个目录 / 每个顶层条目): 每次必查
    fn check(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            Err(CoreError::Cancelled)
        } else {
            Ok(())
        }
    }
}

fn scan_dir_full(
    input: &Path,
    rules: &ExcludeRules,
    cancel: &CancelFlag,
    on_list: &dyn Fn(&[Entry]),
    on_entry: &dyn Fn(usize, &Entry),
    on_progress: &dyn Fn(&Progress),
) -> Result<ScanReport> {
    // 取消已置位的扫描不再读盘, 也不给前端发骨架(旧扫描已被新扫描取代)
    if cancel.is_cancelled() {
        return Err(CoreError::Cancelled);
    }
    // 宁停不错: 生效模式先校验(UI/CLI 入口已校验, 这里兜底库直调场景)
    let patterns = effective_patterns(rules);
    validate_patterns(&patterns).map_err(CoreError::InvalidExclude)?;

    let mut warnings: Vec<String> = Vec::new();
    // 顶层条目按名字字典序排序, 保证进度与后续产物顺序稳定、可复现
    let mut names: Vec<String> = fs::read_dir(input)?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort_by_key(|n| n.clone());

    // 第一遍: 顶层骨架(类型 / 文件大小 / 排除标记; 目录大小留到第二遍统计)
    let mut entries: Vec<Entry> = Vec::with_capacity(names.len());
    for name in names {
        let path = input.join(&name);
        let (kind, size, entry_count) = match fs::symlink_metadata(&path) {
            Ok(md) => {
                if md.file_type().is_dir() {
                    // 目录: 只统计普通文件与链接自身, 不跟随目录链接(避免循环与重复计数)
                    (EntryKind::Dir, 0, 1)
                } else {
                    // 文件与符号链接都按自身大小计, 链接归为 File
                    (EntryKind::File, md.len(), 1)
                }
            }
            Err(e) => {
                warnings.push(format!("{}: {e}", path.display()));
                (EntryKind::File, 0, 0)
            }
        };
        // 顶层打标: 只按生效模式命中; 不按任何文件属性/`.` 前缀判定。
        let exclude = matches_patterns(&patterns, &name).map(ExcludeReason::Pattern);
        entries.push(Entry {
            name,
            kind,
            size,
            entry_count,
            dst: String::new(),
            format: String::new(),
            exclude,
        });
    }
    on_list(&entries);

    // 第二遍: 逐条递归统计目录大小, 每条完成即回调(UI 据此填表)
    let total = entries.len();
    let mut ctl = ScanCtl::new(cancel);
    let mut total_bytes = 0u64;
    for (index, entry) in entries.iter_mut().enumerate() {
        ctl.check()?;
        if entry.kind == EntryKind::Dir {
            let path = input.join(&entry.name);
            let (size, entry_count) =
                dir_size(&path, &entry.name, &patterns, &mut warnings, &mut ctl)?;
            entry.size = size;
            entry.entry_count = entry_count;
        }
        // 被标条目不计入总量(统计口径 = 默认将压缩的内容; 条目 size 保留真实值供展示)
        if entry.exclude.is_none() {
            total_bytes += entry.size;
        }
        on_entry(index, entry);
        on_progress(&Progress {
            phase: Phase::Scanning,
            done: index + 1,
            total,
            done_bytes: total_bytes,
            total_bytes: 0, // 总量在扫描完成前未知
            entry: entry.name.clone(),
        });
    }

    let excluded: Vec<ExcludedEntry> = entries
        .iter()
        .filter_map(|entry| {
            entry.exclude.clone().map(|reason| ExcludedEntry {
                name: entry.name.clone(),
                reason,
            })
        })
        .collect();

    Ok(ScanReport {
        entries,
        total_bytes,
        warnings,
        excluded,
    })
}

/// 递归累加目录下所有普通文件/链接自身的大小与条目数(目录自身计 1);
/// 栈式迭代避免深递归。
/// 无权限等读不了的子树计 0 并记 warning, 不中断。
/// 排除(树内): 模式(条目名前缀相对路径, 与后端开关同坐标系);
/// 命中文件不计尺寸, 命中目录整树不下钻。
/// 不按任何文件属性 / `.` 前缀排除用户文件。
/// 取消: 每个目录边界必查, 条目按 [`CANCEL_CHECK_EVERY`] 粒度查; 命中返回
/// [`CoreError::Cancelled`](部分统计结果丢弃)。
fn dir_size(
    dir: &Path,
    entry_name: &str,
    patterns: &[String],
    warnings: &mut Vec<String>,
    ctl: &mut ScanCtl<'_>,
) -> Result<(u64, usize)> {
    let mut total = 0u64;
    // 目录自身也计 1 个条目(空目录 = 1, 与 strategy::StrategyItem.entry_count 对齐)
    let mut entries = 1usize;
    let mut stack = vec![(dir.to_path_buf(), String::new())]; // (目录, 相对条目根的前缀)
    while let Some((d, prefix)) = stack.pop() {
        ctl.check()?;
        let rd = match fs::read_dir(&d) {
            Ok(rd) => rd,
            Err(e) => {
                warnings.push(format!("{}: {e}", d.display()));
                continue;
            }
        };
        for entry in rd {
            ctl.tick()?;
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warnings.push(format!("{}: {e}", d.display()));
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
            // 模式坐标系 = 条目名前缀的相对路径(与 7z -x! 开关一致:
            // 用户按扫描表所见路径写模式, 如 `App/cache`、`Novel/sub/*.log`)
            let key = format!("{entry_name}/{rel}");
            if matches_patterns(patterns, &key).is_some() {
                continue; // 不计尺寸; 目录不入栈(整树剔除/不下钻)
            }
            entries += 1;
            if md.file_type().is_dir() {
                stack.push((path, rel));
            } else {
                total += md.len();
            }
        }
    }
    Ok((total, entries))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::unique_temp;
    use crate::{CoreError, ExcludeReason, ExcludeRules};
    use std::cell::RefCell;

    fn scan_with(root: &Path, rules: &ExcludeRules) -> ScanReport {
        scan_dir(root, rules, &|_| {}).unwrap()
    }

    #[test]
    fn scan_does_not_treat_dot_prefix_as_hidden() {
        // `.` 前缀是 Unix 命名约定, 用户可能自建 .env/.git/.config;
        // 隐藏只走 OS 属性(Windows), 不能用名字批量排除。
        let root = unique_temp("scan-dot-name");
        fs::create_dir_all(root.join("in/.hid")).unwrap();
        fs::write(root.join("in/.hid/x"), "xx").unwrap(); // 2 字节, 不再整树剔除
        fs::write(root.join("in/.file"), "fff").unwrap(); // 3 字节
        fs::write(root.join("in/.env"), "ee").unwrap(); // 2 字节
        fs::write(root.join("in/keep.txt"), "k").unwrap(); // 1 字节

        let report = scan_with(&root.join("in"), &ExcludeRules::default());
        for name in [".hid", ".file", ".env", "keep.txt"] {
            let entry = report.entries.iter().find(|e| e.name == name).unwrap();
            assert!(entry.exclude.is_none(), "{name} 不应按名字排除");
        }
        assert_eq!(report.total_bytes, 8, "全部普通文件都计入");
        assert!(report.excluded.is_empty());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scan_excludes_system_metadata_by_pattern() {
        let root = unique_temp("scan-meta");
        fs::create_dir_all(root.join("in/Novel")).unwrap();
        fs::write(root.join("in/Thumbs.db"), "tt").unwrap(); // 2 字节
        fs::write(root.join("in/Novel/.DS_Store"), "d").unwrap(); // 1 字节
        fs::write(root.join("in/Novel/keep.txt"), "kk").unwrap(); // 2 字节

        let rules = ExcludeRules {
            include_hidden: true, // 与属性无关, 只验证模式
            patterns: vec!["Thumbs.db".into(), ".DS_Store".into()],
        };
        let report = scan_with(&root.join("in"), &rules);
        let thumbs = report
            .entries
            .iter()
            .find(|e| e.name == "Thumbs.db")
            .unwrap();
        assert_eq!(
            thumbs.exclude,
            Some(ExcludeReason::Pattern("Thumbs.db".into()))
        );
        let novel = report.entries.iter().find(|e| e.name == "Novel").unwrap();
        assert_eq!(novel.size, 2, "树内 .DS_Store 被扣除");
        assert_eq!(report.total_bytes, 2, "keep.txt 计入, 元数据不计");

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scan_does_not_exclude_user_hidden_attribute() {
        let root = unique_temp("scan-user-hidden");
        fs::create_dir_all(root.join("in")).unwrap();
        fs::write(root.join("in/Thumbs.db"), "tt").unwrap(); // 2 字节(元数据)
        fs::write(root.join("in/secret.txt"), "sss").unwrap(); // 3 字节(用户隐藏)
        fs::write(root.join("in/keep.txt"), "k").unwrap(); // 1 字节
        if cfg!(windows) {
            // 用户手动隐藏的普通文件: 仅属性, 不是系统元数据
            for p in ["in/secret.txt", "in/Thumbs.db"] {
                let st = std::process::Command::new("attrib")
                    .args(["+H", root.join(p).to_str().unwrap()])
                    .status()
                    .unwrap();
                assert!(st.success(), "attrib +H 失败: {p}");
            }
        }

        let report = scan_with(&root.join("in"), &ExcludeRules::default());
        let thumbs = report
            .entries
            .iter()
            .find(|e| e.name == "Thumbs.db")
            .unwrap();
        assert_eq!(
            thumbs.exclude,
            Some(ExcludeReason::Pattern("Thumbs.db".into())),
            "系统元数据按名单排除"
        );
        let secret = report
            .entries
            .iter()
            .find(|e| e.name == "secret.txt")
            .unwrap();
        assert!(secret.exclude.is_none(), "用户隐藏属性不参与判定");
        assert_eq!(report.total_bytes, 4, "secret + keep 计入");
        assert_eq!(report.excluded.len(), 1, "只有元数据被排除");

        // include_hidden=true: 连系统元数据也不排除
        let rules = ExcludeRules {
            include_hidden: true,
            patterns: vec![],
        };
        let report = scan_with(&root.join("in"), &rules);
        assert!(report.entries.iter().all(|e| e.exclude.is_none()));
        assert_eq!(report.total_bytes, 6);

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scan_applies_user_patterns_to_top_level_and_tree() {
        let root = unique_temp("scan-pattern");
        fs::create_dir_all(root.join("in/App/cache")).unwrap();
        fs::create_dir_all(root.join("in/keep")).unwrap();
        fs::write(root.join("in/App/cache/junk.tmp"), "jj").unwrap(); // 2
        fs::write(root.join("in/App/main.bin"), "mmm").unwrap(); // 3
        fs::write(root.join("in/keep/data"), "dd").unwrap(); // 2
        fs::write(root.join("in/drop.log"), "x").unwrap(); // 1

        let rules = ExcludeRules {
            include_hidden: true,
            patterns: vec!["*.log".into(), "App/cache".into()],
        };
        let report = scan_with(&root.join("in"), &rules);
        // 顶层打标(Pattern 保留命中模式)
        let d = report
            .entries
            .iter()
            .find(|e| e.name == "drop.log")
            .unwrap();
        assert_eq!(d.exclude, Some(ExcludeReason::Pattern("*.log".into())));
        assert!(
            report
                .entries
                .iter()
                .find(|e| e.name == "keep")
                .unwrap()
                .exclude
                .is_none()
        );
        // 目录大小扣除: App 只剩 main.bin(cache 整树不下钻);drop.log 不计入总量
        assert_eq!(
            report
                .entries
                .iter()
                .find(|e| e.name == "App")
                .unwrap()
                .size,
            3
        );
        assert_eq!(report.total_bytes, 5);
        assert_eq!(report.excluded.len(), 1);
        assert_eq!(
            report.excluded[0].reason,
            ExcludeReason::Pattern("*.log".into())
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scan_rejects_invalid_patterns_as_invalid_exclude() {
        let root = unique_temp("scan-badpat");
        fs::create_dir_all(root.join("in")).unwrap();
        let rules = ExcludeRules {
            include_hidden: true,
            patterns: vec!["a/**b".into()],
        };
        let err = scan_dir(&root.join("in"), &rules, &|_| {}).unwrap_err();
        assert!(matches!(err, CoreError::InvalidExclude(_)));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scan_lists_sorted_top_level_entries_with_sizes() {
        let root = unique_temp("scan");
        fs::create_dir_all(root.join("in/b_dir/nested")).unwrap();
        fs::create_dir_all(root.join("in/empty_dir")).unwrap();
        fs::write(root.join("in/a.txt"), "a").unwrap(); // 1 字节
        fs::write(root.join("in/b_dir/c.txt"), "abc").unwrap(); // 3 字节
        fs::write(root.join("in/b_dir/nested/d.txt"), "abcd").unwrap(); // 4 字节

        let seen = RefCell::new(Vec::new());
        let report = scan_dir(&root.join("in"), &ExcludeRules::default(), &|p| {
            seen.borrow_mut().push((p.phase, p.done, p.total));
        })
        .unwrap();

        // 按名字字典序, kind 与递归大小正确
        assert_eq!(report.entries.len(), 3);
        assert_eq!(report.entries[0].name, "a.txt");
        assert_eq!(report.entries[0].kind, EntryKind::File);
        assert_eq!(report.entries[0].size, 1);
        assert_eq!(report.entries[0].entry_count, 1);
        assert_eq!(report.entries[1].name, "b_dir");
        assert_eq!(report.entries[1].kind, EntryKind::Dir);
        assert_eq!(report.entries[1].size, 7, "目录大小 = 递归文件大小之和");
        assert_eq!(
            report.entries[1].entry_count, 4,
            "b_dir 自身 + nested 目录 + 2 个文件"
        );
        assert_eq!(report.entries[2].name, "empty_dir");
        assert_eq!(report.entries[2].kind, EntryKind::Dir);
        assert_eq!(report.entries[2].size, 0);
        assert_eq!(report.entries[2].entry_count, 1, "空目录自身计 1");
        assert_eq!(report.total_bytes, 8);
        assert!(report.warnings.is_empty());
        // dst 与 format 均由 Step 2 填充, 扫描不感知格式
        assert_eq!(report.entries[0].dst, "");
        assert_eq!(report.entries[0].format, "");

        // 扫描进度:每个顶层条目回调一次, phase=Scanning, done 递增
        let s = seen.borrow();
        assert_eq!(s.len(), 3);
        assert!(s.iter().all(|(ph, _, _)| *ph == Phase::Scanning));
        assert_eq!(
            s.iter().map(|(_, d, _)| *d).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert!(s.iter().all(|(_, _, t)| *t == 3));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scan_dir_stream_lists_entries_before_sizes_then_reports_each_entry() {
        let root = unique_temp("scan-stream");
        fs::create_dir_all(root.join("in/b_dir/nested")).unwrap();
        fs::create_dir_all(root.join("in/empty_dir")).unwrap();
        fs::write(root.join("in/a.txt"), "a").unwrap(); // 1 字节
        fs::write(root.join("in/b_dir/c.txt"), "abc").unwrap(); // 3 字节

        let listed = RefCell::new(Vec::new());
        let measured = RefCell::new(Vec::new());
        let flag = CancelFlag::new();
        let report = scan_dir_stream(
            &root.join("in"),
            &ExcludeRules::default(),
            &flag,
            &|entries| {
                listed.borrow_mut().extend(
                    entries
                        .iter()
                        .map(|entry| (entry.name.clone(), entry.kind, entry.size)),
                );
            },
            &|index, entry| {
                measured
                    .borrow_mut()
                    .push((index, entry.name.clone(), entry.size));
            },
        )
        .unwrap();

        // 骨架阶段: 排序已固定; 文件大小可见, 目录大小未知(0)
        let listed = listed.borrow();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0], ("a.txt".to_string(), EntryKind::File, 1));
        assert_eq!(listed[1], ("b_dir".to_string(), EntryKind::Dir, 0));
        assert_eq!(listed[2], ("empty_dir".to_string(), EntryKind::Dir, 0));

        // 逐条回调: 下标与骨架序一致, 回调时大小已统计完成
        let measured = measured.borrow();
        assert_eq!(
            measured
                .iter()
                .map(|(index, name, _)| (*index, name.as_str()))
                .collect::<Vec<_>>(),
            vec![(0, "a.txt"), (1, "b_dir"), (2, "empty_dir")]
        );
        assert_eq!(measured[1].2, 3, "b_dir 回调时递归大小已就绪");

        assert_eq!(report.total_bytes, 4);
        assert_eq!(
            report.entries[1].entry_count, 3,
            "b_dir 自身 + nested + c.txt"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scan_dir_stream_cancelled_before_start_skips_listing_and_io() {
        let root = unique_temp("scan-stream-cancel-early");
        fs::create_dir_all(root.join("in/a")).unwrap();
        fs::write(root.join("in/a/x"), "x").unwrap();

        let flag = CancelFlag::new();
        flag.cancel();
        let listed = RefCell::new(false);
        let err = scan_dir_stream(
            &root.join("in"),
            &ExcludeRules::default(),
            &flag,
            &|_| *listed.borrow_mut() = true,
            &|_, _| panic!("取消后不应统计任何条目"),
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::Cancelled));
        assert!(!*listed.borrow(), "取消的扫描不再给前端发骨架");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scan_dir_stream_stops_after_cancel_between_top_level_entries() {
        let root = unique_temp("scan-stream-cancel-mid");
        for name in ["a", "b", "c"] {
            fs::create_dir_all(root.join("in").join(name)).unwrap();
        }
        fs::write(root.join("in/a/x"), "x").unwrap();

        let flag = CancelFlag::new();
        let cancel_after_first = flag.clone();
        let measured = RefCell::new(Vec::new());
        let err = scan_dir_stream(
            &root.join("in"),
            &ExcludeRules::default(),
            &flag,
            &|_| {},
            &|index, entry| {
                measured.borrow_mut().push(entry.name.clone());
                if index == 0 {
                    cancel_after_first.cancel(); // 第一条完成后取消
                }
            },
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::Cancelled));
        assert_eq!(*measured.borrow(), ["a"], "取消后不再统计后续条目");
        fs::remove_dir_all(&root).unwrap();
    }
}
