//! zip-tool 核心库:批量压缩 / 分包 / 密码 / 随机包名。
//!
//! CLI(`zip-tool-cli`)与 Tauri UI(`zip-tool-tauri`)共用本 crate。
//! 核心只做逻辑,不依赖任何终端或 UI 设施:
//! - 进度通过 `on_progress` 回调上报,CLI 画进度条、Tauri 推送事件;
//! - 取消经 [`CancelFlag`] 触发:条目/组边界由回调返回 `ControlFlow::Break`,
//!   条目/组内部则终止在跑的后端命令进程并清理半成品(`CoreError::Cancelled`)。
//!
//! 压缩后端按平台选择(core 内部抽象,均为调用外部命令):
//! - Linux: 系统 `zip` 命令
//! - Windows: `7z` 命令(Windows 无系统 zip 命令)
//! - 格式维度:zip(最大兼容)与 7z(LZMA2),见 [`backend::Format`] 与 _docs/compress-modes.md
//!
//! 计划的对外 API(尚未实现):
//! - 压缩选项: [`Options`]
//! - `compress_dir(src, out_dir, opts, on_progress)`:批量压缩入口,目录级隔离
//! - `extract(archive_dir, dest)`:解压/合并分包
//!
//! 分包策略:使用后端原生分包,保持各后端默认命名:
//! - Linux `zip -s`:`B.z01, ..., B.zip`(最后一卷为 `.zip`)
//! - Windows `7z -v`:`B.zip.001, ...`(`-tzip` 输出 zip 格式;实测 NanaZip 支持 zip 分卷)
//!
//! 输出格式由 `Options.format` 决定(zip / 7z),解压/合并由 `extract` 承担。

pub mod backend;
pub mod cancel;
pub mod compress;
pub mod doctor;
pub mod exclude;
pub mod extract;
pub mod naming;
pub mod progress;
pub mod scan;

pub use backend::{Format, parse_size};
pub use cancel::CancelFlag;
pub use compress::{CompressedEntry, Report, compress_dir, compress_entries, plan};
pub use exclude::{ExcludeReason, ExcludeRules, ExcludedEntry, validate_patterns};
pub use extract::{
    ExtractGroup, ExtractReport, ExtractScan, MetaFile, ScanIssue, extract, extract_prepared,
    scan_groups,
};
pub use progress::{Phase, Progress};
pub use scan::{Entry, EntryKind, ScanReport, scan_dir};

#[cfg(test)]
mod testutil;

use std::fmt;
use std::path::PathBuf;

/// 核心库错误(变体随测试驱动逐步扩充)
#[derive(Debug)]
pub enum CoreError {
    /// zip -s 要求分包至少 64k(官方文档约定)
    SplitSizeTooSmall(u64),
    /// 输入目录不存在
    InputNotFound(PathBuf),
    /// 输入目录里没有可压缩的条目
    NoEntries(PathBuf),
    /// 非随机命名下, 条目名在大小写不敏感比较后重复(输出盘可能是 NTFS)
    DuplicateDst(Vec<String>),
    /// 输出目录已有同名压缩包, 不覆盖
    OutputExists(PathBuf),
    /// 输出目录位于输入目录内(或与输入相同)
    OutInsideInput(PathBuf),
    /// 解压:传了非入口卷且同目录找不到对应入口卷(如只有 .z01 没有 .zip)
    EntryVolumeMissing {
        /// 传入的路径
        got: PathBuf,
        /// 应存在的入口卷路径
        expected: PathBuf,
    },
    /// 解压:扩展名不是支持的包形态(zip 系四种 / 7z 系三种)
    UnsupportedArchive(PathBuf),
    /// 解压:入口卷不存在
    ArchiveNotFound(PathBuf),
    /// 解压:目标路径已存在但是普通文件
    DestNotADirectory(PathBuf),
    /// 解压:包内清单含路径逃逸条目(绝对路径或 .. 段)
    UnsafeArchive {
        /// 违规条目名
        path: String,
        /// 违规原因
        reason: String,
    },
    /// 解压:目标目录已有同名文件, 不覆盖(提示 --force)
    ExtractConflict(Vec<String>),
    /// 压缩/解压:所需工具(zip / 7z / unzip)不在 PATH
    ToolMissing(String),
    /// 压缩:用户排除模式非法(空串 / 空段 / `**` 位置错误)
    InvalidExclude(String),
    /// 解压:输入目录里没有任何压缩包组
    NoArchives(PathBuf),
    /// 压缩:条目在扫描之后、执行之前消失(分阶段 API 的存在性复核, 提示重新扫描)
    EntryMissing {
        /// 消失的条目名
        name: String,
    },
    /// 后端命令非零退出
    BackendFailed {
        program: String,
        status: String,
        output: String,
    },
    /// 压缩后结构校验失败:产物无法被解压端正常打开(不完整/损坏)
    VerifyFailed {
        /// 入口卷路径
        archive: PathBuf,
        status: String,
        output: String,
    },
    /// 任务被取消:在跑的后端命令进程已被终止并等待回收,半成品由编排层清理
    Cancelled,
    /// 文件系统 IO 错误
    Io(std::io::Error),
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoreError::SplitSizeTooSmall(n) => {
                write!(f, "split size {n} bytes is below zip's 64k minimum")
            }
            CoreError::InputNotFound(p) => write!(f, "input directory not found: {}", p.display()),
            CoreError::NoEntries(p) => write!(f, "no entries to compress in: {}", p.display()),
            CoreError::DuplicateDst(names) => {
                write!(
                    f,
                    "duplicate archive names (case-insensitive): {}",
                    names.join(", ")
                )
            }
            CoreError::OutputExists(p) => {
                write!(
                    f,
                    "archive already exists, not overwriting: {}",
                    p.display()
                )
            }
            CoreError::OutInsideInput(p) => {
                write!(
                    f,
                    "output directory is inside input directory: {}",
                    p.display()
                )
            }
            CoreError::EntryVolumeMissing { got, expected } => write!(
                f,
                "missing entry volume: {} (needed by {})",
                expected.display(),
                got.display()
            ),
            CoreError::UnsupportedArchive(p) => write!(
                f,
                "unsupported archive type: {} (expected .zip / .zNN / .zip.001 / .zip.NNN / .7z / .7z.001 / .7z.NNN)",
                p.display()
            ),
            CoreError::ArchiveNotFound(p) => write!(f, "archive not found: {}", p.display()),
            CoreError::DestNotADirectory(p) => write!(
                f,
                "destination exists but is not a directory: {}",
                p.display()
            ),
            CoreError::UnsafeArchive { path, reason } => {
                write!(f, "unsafe archive entry {path:?}: {reason}")
            }
            CoreError::ExtractConflict(paths) => write!(
                f,
                "destination already has files, not overwriting (use --force): {}",
                paths.join(", ")
            ),
            CoreError::ToolMissing(program) => {
                write!(f, "required tool {program:?} not found in PATH")
            }
            CoreError::InvalidExclude(msg) => write!(f, "invalid exclude pattern: {msg}"),
            CoreError::NoArchives(p) => write!(f, "no archives found in: {}", p.display()),
            CoreError::EntryMissing { name } => write!(
                f,
                "entry disappeared after scan: {name} (rescan the source folder)"
            ),
            CoreError::BackendFailed {
                program,
                status,
                output,
            } => write!(f, "{program} failed ({status}):\n{output}"),
            CoreError::VerifyFailed {
                archive,
                status,
                output,
            } => {
                write!(
                    f,
                    "archive verification failed ({status}): {} (the archive may be incomplete or corrupted)",
                    archive.display()
                )?;
                if !output.trim().is_empty() {
                    write!(f, "\n{output}")?;
                }
                Ok(())
            }
            CoreError::Cancelled => write!(f, "operation cancelled"),
            CoreError::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for CoreError {}

impl From<std::io::Error> for CoreError {
    fn from(e: std::io::Error) -> Self {
        CoreError::Io(e)
    }
}

/// 核心库统一 Result
pub type Result<T> = std::result::Result<T, CoreError>;

/// 压缩选项
#[derive(Debug, Clone)]
pub struct Options {
    /// 分包大小(字节);`None` 表示不分包
    pub split_size: Option<u64>,
    /// 压缩密码;`None` 表示不加密
    pub password: Option<String>,
    /// 是否使用随机包名,避免包名本身泄露敏感信息
    pub random_name: bool,
    /// 压缩包格式,默认 zip(最大兼容)
    pub format: Format,
    /// 压缩级别 0-9(0=仅存储);`None` = 不传级别参数,用后端默认
    pub level: Option<u8>,
    /// 排除规则:内建隐藏默认集 + 用户模式;Default = 排除隐藏、无用户模式
    pub exclude: ExcludeRules,
    /// 压缩成功后做结构校验(默认开启;UI 勾选框可关闭)
    pub verify: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            split_size: None,
            password: None,
            random_name: false,
            format: Format::default(),
            level: None,
            exclude: ExcludeRules::default(),
            verify: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options_do_not_split_or_encrypt() {
        let opts = Options::default();
        assert!(opts.split_size.is_none());
        assert!(opts.password.is_none());
        assert!(!opts.random_name);
        assert_eq!(opts.format, Format::Zip, "默认 zip(最大兼容)");
        assert!(opts.level.is_none(), "默认不传级别, 用后端默认");
        assert!(opts.exclude.patterns.is_empty(), "默认无用户模式");
        assert!(!opts.exclude.include_hidden, "默认排除隐藏文件");
        assert!(opts.verify, "默认压缩后做结构校验");
    }

    #[test]
    fn verify_failed_display_names_archive_and_hints_corruption() {
        let err = CoreError::VerifyFailed {
            archive: PathBuf::from("out/A.zip"),
            status: "exit code: 2".into(),
            output: String::new(),
        };
        let text = err.to_string();
        assert!(text.contains("out/A.zip"), "{text}");
        assert!(text.contains("exit code: 2"), "{text}");
        assert!(text.contains("incomplete or corrupted"), "{text}");
    }
}
