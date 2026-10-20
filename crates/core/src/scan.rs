//! Step 1 扫描:列出 input 顶层条目(按名字排序)并递归统计字节大小。
//! 大小只用于进度加权与预检, 允许有误差; 扫描进度经回调上报, 不支持取消。

use std::fs;
use std::path::Path;

use crate::exclude::{
    AttrCollector, ExcludeReason, ExcludeRules, ExcludedEntry, is_hidden_attr, is_hidden_name,
    matches_patterns, validate_patterns,
};
use crate::progress::{Phase, Progress};
use crate::{CoreError, Result};

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
    /// Windows 属性隐藏的树内相对路径(条目名前缀, `/` 分隔, 见 _docs/exclude-rules.md §4);
    /// 非 Windows 恒为空
    pub attr_hidden: Vec<String>,
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
/// CLI 丢弃 / UI 显示不勾选), 树内被排除项不计尺寸; Windows 属性隐藏
/// 收集为 `Entry.attr_hidden` 相对路径(§4, 预算护栏超限即回退并 warning)。
pub fn scan_dir(
    input: &Path,
    rules: &ExcludeRules,
    on_progress: &dyn Fn(&Progress),
) -> Result<ScanReport> {
    // 宁停不错: 用户模式先校验(UI/CLI 入口已校验, 这里兜底库直调场景)
    validate_patterns(&rules.patterns).map_err(CoreError::InvalidExclude)?;

    // 顶层条目按名字字典序排序, 保证进度与后续产物顺序稳定、可复现
    let mut names: Vec<String> = fs::read_dir(input)?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort_by_key(|n| n.clone());

    let total = names.len();
    let mut report = ScanReport::default();
    // Windows 属性枚举仅在"启用隐藏排除"时进行(§4); 其余平台 None
    let collect_attrs = !rules.include_hidden && cfg!(windows);

    for (i, name) in names.into_iter().enumerate() {
        let path = input.join(&name);
        let mut attr_hidden = Vec::new();
        let (kind, size, entry_count) = match fs::symlink_metadata(&path) {
            Ok(md) => {
                if md.file_type().is_dir() {
                    let mut collector = AttrCollector::new(name.clone());
                    // 目录: 只统计普通文件与链接自身, 不跟随目录链接(避免循环与重复计数)
                    let (sz, count) = dir_size(
                        &path,
                        &name,
                        rules,
                        &mut report.warnings,
                        collect_attrs.then_some(&mut collector),
                    );
                    attr_hidden = collector.finish(&mut report.warnings);
                    (EntryKind::Dir, sz, count)
                } else {
                    // 文件与符号链接都按自身大小计, 链接归为 File
                    (EntryKind::File, md.len(), 1)
                }
            }
            Err(e) => {
                report.warnings.push(format!("{}: {e}", path.display()));
                (EntryKind::File, 0, 0)
            }
        };
        // 顶层打标: 隐藏(OS 原生口径: Unix 名字 / Windows 属性)优先, 其次用户模式
        let hidden = !rules.include_hidden
            && (is_hidden_name(&name)
                || collect_attrs
                    && fs::symlink_metadata(&path)
                        .map(|m| is_hidden_attr(&m))
                        .unwrap_or(false));
        let exclude = if hidden {
            Some(ExcludeReason::Hidden)
        } else {
            matches_patterns(&rules.patterns, &name).map(ExcludeReason::Pattern)
        };
        if let Some(reason) = &exclude {
            report.excluded.push(ExcludedEntry {
                name: name.clone(),
                reason: reason.clone(),
            });
        }
        report.entries.push(Entry {
            name,
            kind,
            size,
            entry_count,
            dst: String::new(),
            format: String::new(),
            exclude,
            attr_hidden,
        });
        // 被标条目不计入总量(统计口径 = 默认将压缩的内容; 条目 size 保留真实值供展示)
        if report.entries[i].exclude.is_none() {
            report.total_bytes += size;
        }

        on_progress(&Progress {
            phase: Phase::Scanning,
            done: i + 1,
            total,
            done_bytes: report.total_bytes,
            total_bytes: 0, // 总量在扫描完成前未知
            entry: report.entries[i].name.clone(),
        });
    }

    Ok(report)
}

/// 递归累加目录下所有普通文件/链接自身的大小与条目数(目录自身计 1);
/// 栈式迭代避免深递归。
/// 无权限等读不了的子树计 0 并记 warning, 不中断。
/// 排除(树内): 用户模式(条目名前缀相对路径, 与后端开关同坐标系)+ Unix 名字隐藏;
/// 命中文件不计尺寸, 命中目录整树不下钻。Windows 属性隐藏(attrs=Some 时)收集相对路径而非打标。
fn dir_size(
    dir: &Path,
    entry_name: &str,
    rules: &ExcludeRules,
    warnings: &mut Vec<String>,
    mut attrs: Option<&mut AttrCollector>,
) -> (u64, usize) {
    let mut total = 0u64;
    // 目录自身也计 1 个条目(空目录 = 1, 与 strategy::StrategyItem.entry_count 对齐)
    let mut entries = 1usize;
    let mut stack = vec![(dir.to_path_buf(), String::new())]; // (目录, 相对条目根的前缀)
    while let Some((d, prefix)) = stack.pop() {
        let rd = match fs::read_dir(&d) {
            Ok(rd) => rd,
            Err(e) => {
                warnings.push(format!("{}: {e}", d.display()));
                continue;
            }
        };
        for entry in rd {
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
            let attr_hidden = attrs.is_some() && !rules.include_hidden && is_hidden_attr(&md);
            // 模式坐标系 = 条目名前缀的相对路径(与 7z -x! 开关、属性路径一致:
            // 用户按扫描表所见路径写模式, 如 `App/cache`、`Novel/sub/*.log`)
            let key = format!("{entry_name}/{rel}");
            let excluded = attr_hidden
                || (!rules.include_hidden && is_hidden_name(&name))
                || matches_patterns(&rules.patterns, &key).is_some();
            if excluded {
                if attr_hidden {
                    // 条目名前缀 + / 分隔, 供后端逐条开关(§4)
                    attrs
                        .as_mut()
                        .expect("attr_hidden 只在 attrs=Some 时为真")
                        .add(&key);
                }
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
    (total, entries)
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
    fn scan_unix_hidden_marks_top_level_and_deducts_total() {
        if cfg!(windows) {
            return; // Windows 无名字口径, 该用例仅 Unix 语义
        }
        let root = unique_temp("scan-hidden");
        fs::create_dir_all(root.join("in/.hid")).unwrap();
        fs::write(root.join("in/.hid/x"), "xx").unwrap(); // 2 字节, 整树剔除
        fs::write(root.join("in/.file"), "fff").unwrap(); // 3 字节
        fs::write(root.join("in/keep.txt"), "k").unwrap(); // 1 字节

        let report = scan_with(&root.join("in"), &ExcludeRules::default());
        // 顶层永远返回, 但被标 Hidden
        let hid = report.entries.iter().find(|e| e.name == ".hid").unwrap();
        assert_eq!(hid.exclude, Some(ExcludeReason::Hidden));
        assert_eq!(hid.size, 2, "条目自身 size 保留真实值(展示用)");
        let f = report.entries.iter().find(|e| e.name == ".file").unwrap();
        assert_eq!(f.exclude, Some(ExcludeReason::Hidden));
        // 被标条目不计入总量
        assert_eq!(report.total_bytes, 1, "只有 keep.txt 计入");
        // excluded 名单
        let names: Vec<&str> = report.excluded.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, [".file", ".hid"]);

        // include_hidden=true: 全部保留不打标
        let rules = ExcludeRules {
            include_hidden: true,
            patterns: vec![],
        };
        let report = scan_with(&root.join("in"), &rules);
        assert!(report.entries.iter().all(|e| e.exclude.is_none()));
        assert_eq!(report.total_bytes, 6);
        assert!(report.excluded.is_empty());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scan_windows_attr_hidden_marks_and_collects_inner_paths() {
        if !cfg!(windows) {
            return; // attrib 仅 Windows
        }
        let root = unique_temp("scan-attr");
        fs::create_dir_all(root.join("in/outer/inner")).unwrap();
        fs::write(root.join("in/outer/inner/secret.txt"), "sss").unwrap(); // 3 字节
        fs::write(root.join("in/Thumbs.db"), "tt").unwrap(); // 2 字节
        fs::write(root.join("in/outer/plain.txt"), "pp").unwrap(); // 2 字节
        for p in ["in/Thumbs.db", "in/outer/inner"] {
            let st = std::process::Command::new("attrib")
                .args(["+H", root.join(p).to_str().unwrap()])
                .status()
                .unwrap();
            assert!(st.success(), "attrib +H 失败: {p}");
        }

        let report = scan_with(&root.join("in"), &ExcludeRules::default());
        // 顶层属性隐藏打标(名字口径不生效: . 开头不算)
        let t = report
            .entries
            .iter()
            .find(|e| e.name == "Thumbs.db")
            .unwrap();
        assert_eq!(t.exclude, Some(ExcludeReason::Hidden));
        // 内部隐藏目录: 整树剔除 + 收集为一条条目名前缀相对路径(/ 分隔)
        let o = report.entries.iter().find(|e| e.name == "outer").unwrap();
        assert_eq!(o.size, 2, "plain.txt 计入, inner 整树剔除");
        assert_eq!(o.attr_hidden, vec!["outer/inner".to_string()]);
        assert_eq!(report.total_bytes, 2, "隐藏的顶层与内部都不计入");

        // include_hidden=true: 不收集不打标不剔除
        let rules = ExcludeRules {
            include_hidden: true,
            patterns: vec![],
        };
        let report = scan_with(&root.join("in"), &rules);
        assert!(report.entries.iter().all(|e| e.exclude.is_none()));
        assert!(report.entries.iter().all(|e| e.attr_hidden.is_empty()));
        assert_eq!(report.total_bytes, 7);

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
}
