//! zip-tool 核心库:批量压缩 / 分包 / 密码 / 随机包名。
//!
//! CLI(`zip-tool-cli`)与 Tauri UI(`zip-tool-tauri`)共用本 crate。
//! 核心只做逻辑,不依赖任何终端或 UI 设施:
//! - 进度通过 `on_progress` 回调上报,CLI 画进度条、Tauri 推送事件;
//! - 取消通过回调返回 `ControlFlow::Break` 实现(CLI Ctrl+C / UI 取消按钮)。
//!
//! 压缩后端按平台选择(core 内部抽象,均为调用外部命令):
//! - Linux: 系统 `zip` 命令
//! - Windows: `7z` 命令(Windows 无系统 zip 命令)
//!
//! 计划的对外 API(尚未实现):
//! - [`Options`]:压缩选项
//! - `compress_dir(src, out_dir, opts, on_progress)`:批量压缩入口,目录级隔离
//! - `extract(archive_dir, dest)`:解压/合并分包
//!
//! 分包策略:使用后端原生分包,保持各后端默认命名:
//! - Linux `zip -s`:`B.z01, ..., B.zip`(最后一卷为 `.zip`)
//! - Windows `7z -v`:`B.zip.001, ...`(`-tzip` 输出 zip 格式;实测 NanaZip 支持 zip 分卷)
//!
//! 两侧输出均为 zip 格式,解压/合并由 `extract` 承担。

pub mod backend;
pub mod compress;
pub mod doctor;
pub mod extract;
pub mod naming;
pub mod progress;
pub mod scan;

pub use backend::parse_size;
pub use compress::{CompressedEntry, Report, compress_dir};
pub use extract::extract;
pub use progress::{Phase, Progress};
pub use scan::{Entry, EntryKind, scan_dir};

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
    /// 后端命令非零退出
    BackendFailed {
        program: String,
        status: String,
        output: String,
    },
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
                write!(f, "duplicate archive names (case-insensitive): {}", names.join(", "))
            }
            CoreError::OutputExists(p) => {
                write!(f, "archive already exists, not overwriting: {}", p.display())
            }
            CoreError::OutInsideInput(p) => {
                write!(f, "output directory is inside input directory: {}", p.display())
            }
            CoreError::BackendFailed {
                program,
                status,
                output,
            } => write!(f, "{program} failed ({status}):\n{output}"),
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

/// 压缩选项(骨架占位,字段随实现调整)
#[derive(Debug, Default, Clone)]
pub struct Options {
    /// 分包大小(字节);`None` 表示不分包
    pub split_size: Option<u64>,
    /// 压缩密码;`None` 表示不加密
    pub password: Option<String>,
    /// 是否使用随机包名,避免包名本身泄露敏感信息
    pub random_name: bool,
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
    }
}
