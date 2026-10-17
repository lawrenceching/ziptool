//! Step 1 扫描:列出 input 顶层条目(按名字排序)并递归统计字节大小。
//! 大小只用于进度加权与预检, 允许有误差; 扫描进度经回调上报, 不支持取消。

use std::fs;
use std::path::Path;

use crate::progress::{Phase, Progress};
use crate::Result;

/// 一个待压缩的顶层条目(Step 1 产出, Step 2 补全 dst, Step 3 消费)
#[derive(Debug, Clone)]
pub struct Entry {
    /// 条目名(不含路径)
    pub name: String,
    pub kind: EntryKind,
    /// 递归统计的字节大小:文件为自身大小, 目录为其中所有普通文件大小之和
    pub size: u64,
    /// 压缩包基础名(不带扩展名), 由 Step 2 填充; 扫描后为空串
    pub dst: String,
    /// 压缩包格式, 当前恒为 "zip"
    pub format: String,
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
}

/// 扫描 input 的顶层条目并统计大小; 每完成一个顶层条目回调一次进度。
pub fn scan_dir(input: &Path, on_progress: &dyn Fn(&Progress)) -> Result<ScanReport> {
    // 顶层条目按名字字典序排序, 保证进度与后续产物顺序稳定、可复现
    let mut names: Vec<String> = fs::read_dir(input)?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort_by_key(|n| n.clone());

    let total = names.len();
    let mut report = ScanReport::default();

    for (i, name) in names.into_iter().enumerate() {
        let path = input.join(&name);
        let (kind, size) = match fs::symlink_metadata(&path) {
            Ok(md) => {
                if md.file_type().is_dir() {
                    // 目录: 只统计普通文件与链接自身, 不跟随目录链接(避免循环与重复计数)
                    (EntryKind::Dir, dir_size(&path, &mut report.warnings))
                } else {
                    // 文件与符号链接都按自身大小计, 链接归为 File
                    (EntryKind::File, md.len())
                }
            }
            Err(e) => {
                report.warnings.push(format!("{}: {e}", path.display()));
                (EntryKind::File, 0)
            }
        };
        report.entries.push(Entry {
            name,
            kind,
            size,
            dst: String::new(),
            format: "zip".into(),
        });
        report.total_bytes += size;

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

/// 递归累加目录下所有普通文件/链接自身的大小; 栈式迭代避免深递归。
/// 无权限等读不了的子树计 0 并记 warning, 不中断。
fn dir_size(dir: &Path, warnings: &mut Vec<String>) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
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
            let md = match fs::symlink_metadata(&path) {
                Ok(md) => md,
                Err(e) => {
                    warnings.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            if md.file_type().is_dir() {
                stack.push(path);
            } else {
                total += md.len();
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::unique_temp;
    use std::cell::RefCell;

    #[test]
    fn scan_lists_sorted_top_level_entries_with_sizes() {
        let root = unique_temp("scan");
        fs::create_dir_all(root.join("in/b_dir/nested")).unwrap();
        fs::create_dir_all(root.join("in/empty_dir")).unwrap();
        fs::write(root.join("in/a.txt"), "a").unwrap(); // 1 字节
        fs::write(root.join("in/b_dir/c.txt"), "abc").unwrap(); // 3 字节
        fs::write(root.join("in/b_dir/nested/d.txt"), "abcd").unwrap(); // 4 字节

        let seen = RefCell::new(Vec::new());
        let report = scan_dir(&root.join("in"), &|p| {
            seen.borrow_mut().push((p.phase, p.done, p.total));
        })
        .unwrap();

        // 按名字字典序, kind 与递归大小正确
        assert_eq!(report.entries.len(), 3);
        assert_eq!(report.entries[0].name, "a.txt");
        assert_eq!(report.entries[0].kind, EntryKind::File);
        assert_eq!(report.entries[0].size, 1);
        assert_eq!(report.entries[1].name, "b_dir");
        assert_eq!(report.entries[1].kind, EntryKind::Dir);
        assert_eq!(report.entries[1].size, 7, "目录大小 = 递归文件大小之和");
        assert_eq!(report.entries[2].name, "empty_dir");
        assert_eq!(report.entries[2].kind, EntryKind::Dir);
        assert_eq!(report.entries[2].size, 0);
        assert_eq!(report.total_bytes, 8);
        assert!(report.warnings.is_empty());
        // dst 由 Step 2 填充, 扫描后为空; format 恒为 zip
        assert_eq!(report.entries[0].dst, "");
        assert_eq!(report.entries[0].format, "zip");

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
