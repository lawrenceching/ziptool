//! 批量压缩编排:input 下每个顶层条目(目录或文件)各压缩为一个包。
//!
//! - 进度:每完成一个条目回调一次 `Progress`(done 从 1 递增,total 为条目总数)
//! - 取消:回调返回 `ControlFlow::Break` 后不再开始下一个条目,已完成的保留
//! - 随机包名:`zt_<16hex>.zip`,同时在 out_dir 写 `mapping.csv` 记录对应关系

use std::ffi::OsString;
use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use crate::backend::Backend;
use crate::naming::random_name;
use crate::{CoreError, Options, Result};

/// 单条目进度
#[derive(Debug, Clone)]
pub struct Progress {
    /// 已完成条目数(从 1 开始)
    pub done: usize,
    /// 条目总数
    pub total: usize,
    /// 刚完成的条目名
    pub entry: String,
}

/// 一个完成的条目:压缩包路径与原始名字(随机命名时二者不同)
#[derive(Debug, Clone)]
pub struct CompressedEntry {
    /// 压缩包主文件(.zip;分卷时 zip -s 为末卷 .zip,7z -v 为首卷 .zip.001)
    pub archive: PathBuf,
    /// 原始条目名
    pub original: String,
}

/// 批量压缩结果
#[derive(Debug, Default)]
pub struct Report {
    pub entries: Vec<CompressedEntry>,
    /// 回调要求取消而提前结束时为 true
    pub cancelled: bool,
}

/// 批量压缩:把 input 目录下的每个顶层条目各压缩为一个包,输出到 out_dir。
pub fn compress_dir(
    input: &Path,
    out_dir: &Path,
    opts: &Options,
    on_progress: &dyn Fn(&Progress) -> ControlFlow<()>,
) -> Result<Report> {
    compress_with_backend(Backend::native(), input, out_dir, opts, on_progress)
}

pub(crate) fn compress_with_backend(
    backend: Backend,
    input: &Path,
    out_dir: &Path,
    opts: &Options,
    on_progress: &dyn Fn(&Progress) -> ControlFlow<()>,
) -> Result<Report> {
    if !input.is_dir() {
        return Err(CoreError::InputNotFound(input.to_path_buf()));
    }

    let mut items: Vec<PathBuf> = fs::read_dir(input)?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|e| e.path())
        .collect();
    items.sort_by_key(|p| p.file_name().unwrap_or_default().to_owned());

    if items.is_empty() {
        return Err(CoreError::NoEntries(input.to_path_buf()));
    }

    backend.check_split_size(opts.split_size)?;
    fs::create_dir_all(out_dir)?;

    let total = items.len();
    let mut report = Report::default();

    for (i, item) in items.into_iter().enumerate() {
        let original: String = item
            .file_name()
            .unwrap_or(&OsString::new())
            .to_string_lossy()
            .into_owned();
        let base = if opts.random_name {
            format!("zt_{}", random_name())
        } else {
            original.clone()
        };

        let mut cmd = backend.compress_command(&out_dir.join(&base), &item, opts)?;
        backend.run(&mut cmd)?;

        let entry = CompressedEntry {
            archive: resolve_primary_archive(out_dir, &base),
            original,
        };
        report.entries.push(entry);

        if let ControlFlow::Break(()) = on_progress(&Progress {
            done: i + 1,
            total,
            entry: report.entries[i].original.clone(),
        }) {
            report.cancelled = true;
            break;
        }
    }

    if opts.random_name {
        write_mapping(out_dir, &report)?;
    }
    Ok(report)
}

/// 随机命名时写 mapping.csv(archive 名,原始名),供解包端还原
fn write_mapping(out_dir: &Path, report: &Report) -> Result<()> {
    let mut csv = String::from("archive,original\n");
    for e in &report.entries {
        let name = e
            .archive
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        csv.push_str(&format!("{},{}\n", name, e.original));
    }
    fs::write(out_dir.join("mapping.csv"), csv)?;
    Ok(())
}

/// 返回压缩包主文件:优先 base.zip(zip -s 的末卷 / 不分卷),
/// 否则是 7z -v 的首卷 base.zip.001。
fn resolve_primary_archive(out_dir: &Path, base: &str) -> PathBuf {
    let plain = out_dir.join(format!("{base}.zip"));
    if plain.exists() {
        return plain;
    }
    let first_volume = out_dir.join(format!("{base}.zip.001"));
    if first_volume.exists() {
        return first_volume;
    }
    plain
}

/// 随机命名时写 mapping.csv(archive 名,原始名),供解包端还原
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Options;

    fn opts(split: Option<u64>) -> Options {
        Options {
            split_size: split,
            password: None,
            random_name: false,
        }
    }

    fn noop(_: &Progress) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }

    #[test]
    fn zip_backend_rejects_split_below_64k() {
        let root = std::env::temp_dir().join(format!("zip-tool-unit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("in/x")).unwrap();
        fs::write(root.join("in/x/f.txt"), "v").unwrap();

        let err = compress_with_backend(
            Backend::Zip,
            &root.join("in"),
            &root.join("out"),
            &opts(Some(1024)),
            &noop,
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::SplitSizeTooSmall(1024)));
        fs::remove_dir_all(&root).unwrap();
    }
}
