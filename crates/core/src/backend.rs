//! 压缩后端抽象:都是调用外部命令,输出统一为 zip 格式。
//! - [`Backend::Zip`]:Linux 系统 `zip` 命令,分包 `-s`(B.z01..B.zip)
//! - [`Backend::SevenZip`]:`7z` 命令(Windows),分包 `-v` + `-tzip`(B.zip.001..)

use std::path::Path;
use std::process::Command;

use crate::{CoreError, Options, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Linux 系统 zip 命令
    Zip,
    /// 7z 命令(Windows 上没有系统 zip 命令)
    SevenZip,
}

impl Backend {
    /// 按当前平台选择默认后端
    pub fn native() -> Self {
        if cfg!(windows) {
            Backend::SevenZip
        } else {
            Backend::Zip
        }
    }

    fn program(&self) -> &'static str {
        match self {
            Backend::Zip => "zip",
            Backend::SevenZip => "7z",
        }
    }

    /// 执行外部命令,非零退出视为失败(7z 的 1=warning 视为失败,后续有需要再放宽)
    pub(crate) fn run(&self, cmd: &mut Command) -> Result<()> {
        let out = cmd.output()?;
        if !out.status.success() {
            let mut text = String::from_utf8_lossy(&out.stderr).into_owned();
            if text.trim().is_empty() {
                text = String::from_utf8_lossy(&out.stdout).into_owned();
            }
            return Err(CoreError::BackendFailed {
                program: self.program().into(),
                status: out.status.to_string(),
                output: last_lines(&text, 5),
            });
        }
        Ok(())
    }

    /// 各后端的分包大小约束:仅 zip -s 有 64k 下限
    pub(crate) fn check_split_size(&self, size: Option<u64>) -> Result<()> {
        match (self, size) {
            (Backend::Zip, Some(s)) => validate_split_size(s),
            _ => Ok(()),
        }
    }

    /// 构造"把 src 压缩为 out_base 压缩包"的命令。
    /// out_base 不带扩展名;产物为 out_base.zip 或其原生分卷。
    pub fn compress_command(
        &self,
        out_base: &Path,
        src: &Path,
        opts: &Options,
    ) -> Result<Command> {
        let out_zip = out_base.with_extension("zip");
        let mut cmd = Command::new(self.program());
        match self {
            Backend::Zip => {
                cmd.arg("-r");
                if let Some(size) = opts.split_size {
                    cmd.arg("-s").arg(format_split_size(size));
                }
                if let Some(pw) = &opts.password {
                    cmd.arg("-P").arg(pw);
                }
                cmd.arg(&out_zip).arg(src);
            }
            Backend::SevenZip => {
                cmd.arg("a").arg("-tzip");
                if let Some(size) = opts.split_size {
                    cmd.arg(format!("-v{}", format_split_size(size)));
                }
                if let Some(pw) = &opts.password {
                    cmd.arg(format!("-p{pw}"));
                }
                cmd.arg("--").arg(&out_zip).arg(src);
            }
        }
        Ok(cmd)
    }

    /// 构造解压命令。archive 可以是 .zip、zip -s 的末卷 .zip,或 7z -v 的首卷 .zip.001。
    pub fn extract_command(&self, archive: &Path, dest: &Path, password: Option<&str>) -> Command {
        let mut cmd = match self {
            // 解压端程序与压缩端不同:zip -> unzip
            Backend::Zip => Command::new("unzip"),
            Backend::SevenZip => Command::new("7z"),
        };
        match self {
            Backend::Zip => {
                // zip -s 的末卷就是 .zip,unzip 直接读整组分卷
                cmd.arg("-o");
                if let Some(pw) = password {
                    cmd.arg("-P").arg(pw);
                }
                cmd.arg(archive).arg("-d").arg(dest);
            }
            Backend::SevenZip => {
                cmd.arg("x").arg("-aoa");
                if let Some(pw) = password {
                    cmd.arg(format!("-p{pw}"));
                }
                cmd.arg(format!("-o{}", dest.display()));
                cmd.arg("--").arg(archive);
            }
        }
        cmd
    }
}

/// 把字节数格式化为 zip -s / 7z -v 都接受的大小串:
/// 能整除 GB 用 g,其次 MB 用 m,再次 KB 用 k,否则用原始字节数。
pub fn format_split_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB && bytes % GB == 0 {
        format!("{}g", bytes / GB)
    } else if bytes >= MB && bytes % MB == 0 {
        format!("{}m", bytes / MB)
    } else if bytes >= KB && bytes % KB == 0 {
        format!("{}k", bytes / KB)
    } else {
        format!("{bytes}")
    }
}

/// 校验分包大小(zip -s 最小 64k,官方文档约定)
pub fn validate_split_size(bytes: u64) -> Result<()> {
    if bytes < 64 * 1024 {
        Err(crate::CoreError::SplitSizeTooSmall(bytes))
    } else {
        Ok(())
    }
}

/// 取文本最后 n 行(错误输出可能很长)
fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// 解析 "2g"/"500m"/"64k"/"65536" 形式的分包大小为字节;0 或非法格式报错
pub fn parse_size(s: &str) -> std::result::Result<u64, String> {
    const BAD: &str = "expected a size like 2g / 500m / 64k / 65536";
    let s = s.trim();
    if s.is_empty() {
        return Err(BAD.into());
    }
    let (digits, mult) = match s.chars().last().unwrap() {
        'k' | 'K' => (&s[..s.len() - 1], 1024u64),
        'm' | 'M' => (&s[..s.len() - 1], 1024 * 1024),
        'g' | 'G' => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        _ => (s, 1),
    };
    let n: u64 = digits.trim().parse().map_err(|_| format!("{BAD}, got {s:?}"))?;
    if n == 0 {
        return Err("size must be > 0".into());
    }
    n.checked_mul(mult)
        .ok_or_else(|| format!("size too large: {s:?}"))
}

#[cfg(test)]
mod size_tests {
    use super::parse_size;

    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    #[test]
    fn parses_unit_suffixes_case_insensitive() {
        assert_eq!(parse_size("2g").unwrap(), 2 * GB);
        assert_eq!(parse_size("2G").unwrap(), 2 * GB);
        assert_eq!(parse_size("500m").unwrap(), 500 * MB);
        assert_eq!(parse_size("64k").unwrap(), 64 * KB);
        assert_eq!(parse_size("65536").unwrap(), 65536);
    }

    #[test]
    fn rejects_invalid_sizes() {
        assert!(parse_size("").is_err());
        assert!(parse_size("abc").is_err());
        assert!(parse_size("0").is_err());
        assert!(parse_size("0m").is_err());
        assert!(parse_size("12x").is_err());
        assert!(parse_size("1t99999999999999999999").is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(split: Option<u64>, password: Option<&str>) -> Options {
        Options {
            split_size: split,
            password: password.map(String::from),
            random_name: false,
        }
    }

    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.get_args().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn sevenzip_compress_command_basic() {
        let cmd = Backend::SevenZip
            .compress_command(Path::new("/out/A"), Path::new("/in/A"), &opts(None, None))
            .unwrap();
        assert_eq!(cmd.get_program(), "7z");
        assert_eq!(
            args_of(&cmd),
            ["a", "-tzip", "--", "/out/A.zip", "/in/A"]
        );
    }

    #[test]
    fn sevenzip_compress_command_with_password_and_split() {
        let cmd = Backend::SevenZip
            .compress_command(
                Path::new("B"),
                Path::new("D:/data/B"),
                &opts(Some(2 * 1024 * 1024 * 1024), Some("pw")),
            )
            .unwrap();
        assert_eq!(
            args_of(&cmd),
            ["a", "-tzip", "-v2g", "-ppw", "--", "B.zip", "D:/data/B"]
        );
    }

    #[test]
    fn zip_compress_command_basic() {
        let cmd = Backend::Zip
            .compress_command(Path::new("/out/A"), Path::new("/in/A"), &opts(None, None))
            .unwrap();
        assert_eq!(cmd.get_program(), "zip");
        assert_eq!(args_of(&cmd), ["-r", "/out/A.zip", "/in/A"]);
    }

    #[test]
    fn zip_extract_command_with_password() {
        let cmd = Backend::Zip.extract_command(Path::new("/out/B.zip"), Path::new("/dest"), Some("pw"));
        assert_eq!(cmd.get_program(), "unzip");
        assert_eq!(
            args_of(&cmd),
            ["-o", "-P", "pw", "/out/B.zip", "-d", "/dest"]
        );
    }

    #[test]
    fn sevenzip_extract_command_on_first_volume() {
        let cmd = Backend::SevenZip.extract_command(Path::new("B.zip.001"), Path::new("D:\\d"), None);
        assert_eq!(cmd.get_program(), "7z");
        assert_eq!(
            args_of(&cmd),
            ["x", "-aoa", "-oD:\\d", "--", "B.zip.001"]
        );
    }

    #[test]
    fn zip_compress_command_with_password_and_split() {
        let cmd = Backend::Zip
            .compress_command(
                Path::new("/out/B"),
                Path::new("/in/B"),
                &opts(Some(500 * 1024 * 1024), Some("pw")),
            )
            .unwrap();
        assert_eq!(
            args_of(&cmd),
            ["-r", "-s", "500m", "-P", "pw", "/out/B.zip", "/in/B"]
        );
    }

    #[test]
    fn split_size_too_small_for_zip_backend_is_rejected() {
        assert!(validate_split_size(64 * 1024 - 1).is_err());
        assert!(validate_split_size(64 * 1024).is_ok());
    }

    #[test]
    fn format_split_size_uses_largest_exact_unit() {
        assert_eq!(format_split_size(2 * 1024 * 1024 * 1024), "2g");
        assert_eq!(format_split_size(500 * 1024 * 1024), "500m");
        assert_eq!(format_split_size(64 * 1024), "64k");
        assert_eq!(format_split_size(100_000), "100000");
    }
}
