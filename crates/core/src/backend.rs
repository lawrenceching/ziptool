//! 压缩后端抽象:都是调用外部命令,输出统一为 zip 格式。
//! - [`Backend::Zip`]:Linux 系统 `zip` 命令,分包 `-s`(B.z01..B.zip)
//! - [`Backend::SevenZip`]:`7z` 命令(Windows),分包 `-v` + `-tzip`(B.zip.001..)

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::{CancelFlag, CoreError, Options, Result};

/// 进程轮询间隔, 也是取消生效的延迟上界
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// 压缩包格式:决定外部工具、命令参数、产物扩展名与加密方式。
/// zip = Deflate(兼容性最大);7z = LZMA2(压缩率更高, 需 7z 系工具解压)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Format {
    /// zip 格式:Linux 走系统 zip 命令,Windows 走 7z -tzip
    #[default]
    Zip,
    /// 7z 格式:两平台都走 7z 命令(-t7z -m0=LZMA2)
    SevenZ,
}

impl Format {
    /// 产物扩展名(.zip / .7z)
    pub fn extension(&self) -> &'static str {
        match self {
            Format::Zip => "zip",
            Format::SevenZ => "7z",
        }
    }

    /// 格式对应的外部工具:Zip 格式用平台原生;SevenZ 格式两平台统一 7z
    pub fn backend(&self) -> Backend {
        match self {
            Format::Zip => Backend::native(),
            Format::SevenZ => Backend::SevenZip,
        }
    }
}

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

    /// 后端对应的外部命令名(zip / 7z;doctor 自检也要用)
    pub fn program(&self) -> &'static str {
        match self {
            Backend::Zip => "zip",
            Backend::SevenZip => "7z",
        }
    }

    /// 执行外部命令,非零退出视为失败(7z 的 1=warning 视为失败,后续有需要再放宽)。
    /// `cancel` 置位即 kill 子进程并返回 [`CoreError::Cancelled`](延迟上界 = 轮询间隔);
    /// 取消即作废:与"刚自然跑完"竞态时也统一按取消返回,由编排层清理半成品。
    pub(crate) fn run(&self, cmd: &mut Command, cancel: &CancelFlag) -> Result<()> {
        // spawn 默认继承父进程 stdio(与 output() 的默认管道不同), 必须显式设:
        // stdin 断开, 防 7z/unzip 的交互提示(如密码)挂住; 输出接管用于报错
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        // 读者线程排空管道: 输出撑满 64K 缓冲会让子进程写阻塞而死锁;
        // 读进 Vec<u8> 而非 String(输出可能非 UTF-8), 报错时再 lossy 转换
        let t_out = drain_pipe(child.stdout.take());
        let t_err = drain_pipe(child.stderr.take());
        loop {
            if cancel.is_cancelled() {
                // 已自行退出的进程 kill 会报错, 忽略; 统一按取消处理
                let _ = child.kill();
                // 先回收再返回: Windows 上要等子进程句柄释放, 调用方才能删除半成品
                let _ = child.wait();
                let _ = t_out.join();
                let _ = t_err.join();
                return Err(CoreError::Cancelled);
            }
            match child.try_wait()? {
                Some(status) => {
                    let out = t_out.join().unwrap_or_default();
                    let err_out = t_err.join().unwrap_or_default();
                    if !status.success() {
                        let mut text = String::from_utf8_lossy(&err_out).into_owned();
                        if text.trim().is_empty() {
                            text = String::from_utf8_lossy(&out).into_owned();
                        }
                        return Err(CoreError::BackendFailed {
                            program: self.program().into(),
                            status: status.to_string(),
                            output: last_lines(&text, 5),
                        });
                    }
                    return Ok(());
                }
                None => std::thread::sleep(POLL_INTERVAL),
            }
        }
    }

    /// 各后端的分包大小约束:仅 zip -s 有 64k 下限
    pub(crate) fn check_split_size(&self, size: Option<u64>) -> Result<()> {
        match (self, size) {
            (Backend::Zip, Some(s)) => validate_split_size(s),
            _ => Ok(()),
        }
    }

    /// 构造"把 work_dir 下名为 name 的条目压缩为 out_zip"的命令。
    /// 子进程工作目录设为 work_dir(= input), 命令行用相对条目名:
    /// 两后端的包内路径都只有条目名本身
    /// (zip 传全路径会把父目录链存进包, 7z 只存末段, 全路径会导致两平台解压布局不一致)。
    /// out_zip 必须是绝对路径(工作目录切换后相对路径失效), 形如 <out>/<dst>.zip。
    pub fn compress_command(
        &self,
        work_dir: &Path,
        name: &str,
        out_zip: &Path,
        opts: &Options,
    ) -> Result<Command> {
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
                // 级别:Info-ZIP 的级别 flag 就是数字本身(-0 仅存储 … -9 最大)
                if let Some(n) = opts.level {
                    cmd.arg(format!("-{n}"));
                }
                // zip 无 -- 分隔符, "-" 开头的名字 ./ 化防被当参数
                let src = if name.starts_with('-') {
                    format!("./{name}")
                } else {
                    name.to_string()
                };
                cmd.arg(out_zip).arg(src);
            }
            Backend::SevenZip => {
                cmd.arg("a");
                // 格式由 Options.format 决定:SevenZip 后端可产 zip 与 7z 两种格式
                match opts.format {
                    Format::Zip => cmd.arg("-tzip"),
                    // 显式 -m0=LZMA2:防旧版 p7zip 默认 LZMA1, 保证两平台算法一致
                    Format::SevenZ => cmd.arg("-t7z").arg("-m0=LZMA2"),
                };
                if let Some(size) = opts.split_size {
                    cmd.arg(format!("-v{}", format_split_size(size)));
                }
                if let Some(pw) = &opts.password {
                    cmd.arg(format!("-p{pw}"));
                }
                if let Some(n) = opts.level {
                    cmd.arg(format!("-mx={n}"));
                }
                cmd.arg("--").arg(out_zip).arg(name);
            }
        }
        cmd.current_dir(work_dir);
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

    /// 解压端程序名与压缩端不同:zip 的解压端是 unzip
    pub(crate) fn extract_program(&self) -> &'static str {
        match self {
            Backend::Zip => "unzip",
            Backend::SevenZip => "7z",
        }
    }

    /// 构造"列包内条目清单"的命令(机器可读输出, 见 extract.md 5.1):
    /// unzip 用 -Z1(zipinfo 模式, 每行一个名字), 7z 用 l -ba -slt(取 Path = 行)。
    /// zip 格式不加密文件名, 列清单不需要密码。
    pub(crate) fn list_command(&self, entry: &Path) -> Command {
        match self {
            Backend::Zip => {
                let mut c = Command::new("unzip");
                c.arg("-Z1").arg(entry);
                c
            }
            Backend::SevenZip => {
                let mut c = Command::new("7z");
                c.args(["l", "-ba", "-slt", "--"]).arg(entry);
                c
            }
        }
    }
}

/// 解压后端抽象: 生产为 [`RealExtract`](外部命令 unzip/7z), 编排测试可注入假后端。
/// 方法都带 kind 参数: 一个注入对象可同时服务 zip / 7z 形态的组。
pub(crate) trait ExtractBackend {
    /// 解压端程序名(unzip / 7z), 用于执行前定位(缺失报 ToolMissing)
    fn program(&self, kind: Backend) -> &'static str;
    /// 列入口卷内的条目清单
    fn list(&self, kind: Backend, entry: &Path) -> Result<ArchiveListing>;
    /// 把入口卷解到 dest(冲突判断在编排层, 覆盖开关恒带);
    /// cancel 透传给进程等待, 取消即 kill
    fn extract(
        &self,
        kind: Backend,
        entry: &Path,
        dest: &Path,
        password: Option<&str>,
        cancel: &crate::CancelFlag,
    ) -> Result<()>;
}

/// 生产解压后端:行为只由 kind(Backend 枚举)决定, 程序先定位后执行。
pub(crate) struct RealExtract {
    /// 测试注入:覆盖解压端程序名(生产用 None, 按 kind 取 unzip / 7z)
    pub program_override: Option<&'static str>,
}

impl ExtractBackend for RealExtract {
    fn program(&self, kind: Backend) -> &'static str {
        self.program_override
            .unwrap_or_else(|| kind.extract_program())
    }

    fn list(&self, kind: Backend, entry: &Path) -> Result<ArchiveListing> {
        let program = self.program(kind);
        ensure_tool_available(program)?;
        let out = kind
            .list_command(entry)
            .stdin(std::process::Stdio::null())
            .output()?;
        if !out.status.success() {
            let mut text = String::from_utf8_lossy(&out.stderr).into_owned();
            if text.trim().is_empty() {
                text = String::from_utf8_lossy(&out.stdout).into_owned();
            }
            return Err(crate::CoreError::BackendFailed {
                program: program.into(),
                status: out.status.to_string(),
                output: last_lines(&text, 5),
            });
        }
        Ok(parse_listing(&kind, &String::from_utf8_lossy(&out.stdout)))
    }

    fn extract(
        &self,
        kind: Backend,
        entry: &Path,
        dest: &Path,
        password: Option<&str>,
        cancel: &crate::CancelFlag,
    ) -> Result<()> {
        ensure_tool_available(self.program(kind))?;
        let mut cmd = kind.extract_command(entry, dest, password);
        kind.run(&mut cmd, cancel)
    }
}

/// 在 PATH 里定位压缩/解压程序, 缺失报 ToolMissing(而不是 spawn 失败的 os error 2)
pub(crate) fn ensure_tool_available(program: &str) -> Result<()> {
    let dirs: Vec<PathBuf> =
        std::env::split_paths(std::env::var_os("PATH").as_deref().unwrap_or_default()).collect();
    match crate::doctor::locate_tool(program, &dirs) {
        Some(_) => Ok(()),
        None => Err(crate::CoreError::ToolMissing(program.into())),
    }
}

/// 后端命令抽象: 生产为 [`Backend`](外部命令), 编排测试可注入假后端。
pub(crate) trait CommandBackend {
    fn compress_command(
        &self,
        work_dir: &Path,
        name: &str,
        out_zip: &Path,
        opts: &Options,
    ) -> Result<Command>;
    fn run(&self, cmd: &mut Command, cancel: &crate::CancelFlag) -> Result<()>;
    fn check_split_size(&self, size: Option<u64>) -> Result<()>;
}

impl CommandBackend for Backend {
    fn compress_command(
        &self,
        work_dir: &Path,
        name: &str,
        out_zip: &Path,
        opts: &Options,
    ) -> Result<Command> {
        Backend::compress_command(self, work_dir, name, out_zip, opts)
    }

    fn run(&self, cmd: &mut Command, cancel: &crate::CancelFlag) -> Result<()> {
        Backend::run(self, cmd, cancel)
    }

    fn check_split_size(&self, size: Option<u64>) -> Result<()> {
        Backend::check_split_size(self, size)
    }
}

#[cfg(test)]
mod run_tests {
    //! Backend::run 的进程级行为:取消 kill / 自然成功 / 自然失败。
    //! 用真实子进程验证(不依赖 zip/7z 在 PATH)。
    use super::*;
    use crate::CancelFlag;
    use std::time::{Duration, Instant};

    /// 平台无关的"跑 60 秒"命令
    fn long_running() -> Command {
        if cfg!(windows) {
            let mut c = Command::new("ping");
            c.args(["-n", "60", "127.0.0.1"]);
            c
        } else {
            let mut c = Command::new("sleep");
            c.arg("60");
            c
        }
    }

    /// 平台无关的立即成功命令
    fn exit_ok() -> Command {
        if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "exit 0"]);
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg("exit 0");
            c
        }
    }

    /// 平台无关的"输出一行后失败"命令
    fn echo_boom_exit_2() -> Command {
        if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "echo boom & exit 2"]);
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg("echo boom; exit 2");
            c
        }
    }

    #[test]
    fn already_cancelled_flag_returns_cancelled_without_running_to_completion() {
        let flag = CancelFlag::new();
        flag.cancel();
        let mut cmd = long_running();
        let start = Instant::now();
        let err = Backend::native().run(&mut cmd, &flag).unwrap_err();
        assert!(matches!(err, CoreError::Cancelled), "实际: {err:?}");
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "应立即返回而非跑满 60s"
        );
    }

    #[test]
    fn cancelled_flag_kills_long_running_process() {
        let flag = CancelFlag::new();
        let canceller = flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            canceller.cancel();
        });
        let mut cmd = long_running();
        let start = Instant::now();
        let err = Backend::native().run(&mut cmd, &flag).unwrap_err();
        assert!(matches!(err, CoreError::Cancelled), "实际: {err:?}");
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "60s 的命令应在取消后很快被杀"
        );
    }

    #[test]
    fn natural_success_returns_ok() {
        let flag = CancelFlag::new();
        let mut cmd = exit_ok();
        Backend::native().run(&mut cmd, &flag).unwrap();
    }

    #[test]
    fn natural_failure_reports_backend_failed_with_captured_output() {
        let flag = CancelFlag::new();
        let mut cmd = echo_boom_exit_2();
        let err = Backend::native().run(&mut cmd, &flag).unwrap_err();
        match err {
            CoreError::BackendFailed { output, .. } => {
                assert!(
                    output.contains("boom"),
                    "stderr/stdout 应被捕获: {output:?}"
                );
            }
            other => panic!("应为 BackendFailed, 实际: {other:?}"),
        }
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

/// 起读者线程把子进程管道读到 EOF(读不完会撑满缓冲死锁), 返回收集的字节
fn drain_pipe<R: Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    })
}

/// 取文本最后 n 行(错误输出可能很长)
fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// 包内条目清单(解压预检的数据源, 见 _docs/extract.md 5.1)
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveListing {
    /// 全部条目的包内相对路径(含目录), 分隔符归一为 `/`, 目录名以 `/` 结尾
    pub names: Vec<String>,
    /// 非目录条目数
    pub files: usize,
}

/// 解析 list 命令的机器可读输出:
/// - unzip `-Z1`(zipinfo 模式): 每行一个名字, 目录以 / 结尾
/// - 7z `l -ba -slt`: 空行分隔的条目块, `Path =` 行取名字, `Folder = +` 表示目录
pub(crate) fn parse_listing(backend: &Backend, text: &str) -> ArchiveListing {
    let mut names: Vec<String> = Vec::new();
    match backend {
        Backend::Zip => {
            for line in text.lines() {
                let name = line.replace('\\', "/");
                if !name.trim().is_empty() {
                    names.push(name);
                }
            }
        }
        Backend::SevenZip => {
            let mut cur: Option<(String, bool)> = None;
            let flush = |cur: &mut Option<(String, bool)>, names: &mut Vec<String>| {
                if let Some((path, is_dir)) = cur.take() {
                    names.push(if is_dir { format!("{path}/") } else { path });
                }
            };
            for line in text.lines() {
                let line = line.trim_end();
                if let Some(v) = line.strip_prefix("Path = ") {
                    flush(&mut cur, &mut names);
                    cur = Some((v.trim().replace('\\', "/"), false));
                } else if line == "Folder = +" {
                    if let Some((_, is_dir)) = &mut cur {
                        *is_dir = true;
                    }
                } else if line.is_empty() {
                    flush(&mut cur, &mut names);
                }
            }
            flush(&mut cur, &mut names);
        }
    }
    let files = names.iter().filter(|n| !n.ends_with('/')).count();
    ArchiveListing { names, files }
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
    let n: u64 = digits
        .trim()
        .parse()
        .map_err(|_| format!("{BAD}, got {s:?}"))?;
    if n == 0 {
        return Err("size must be > 0".into());
    }
    n.checked_mul(mult)
        .ok_or_else(|| format!("size too large: {s:?}"))
}

#[cfg(test)]
mod listing_tests {
    use super::*;

    #[test]
    fn parses_unzip_z1_names_with_trailing_slash_dirs() {
        let text = "Novel/\nNovel/book.txt\nmedia/\nmedia/clip.bin\nphoto.jpg\n";
        let l = parse_listing(&Backend::Zip, text);
        assert_eq!(
            l.names,
            [
                "Novel/",
                "Novel/book.txt",
                "media/",
                "media/clip.bin",
                "photo.jpg"
            ]
        );
        assert_eq!(l.files, 3);
    }

    #[test]
    fn parses_7z_slt_blocks_and_normalizes_separators() {
        let text = "Path = Novel\nFolder = +\nSize = 0\nPacked Size = 0\n\n\
                    Path = Novel\\book.txt\nFolder = -\nSize = 2500000\nPacked Size = 300000\n\n\
                    Path = photo.jpg\nFolder = -\nSize = 200000\nPacked Size = 190000";
        let l = parse_listing(&Backend::SevenZip, text);
        assert_eq!(l.names, ["Novel/", "Novel/book.txt", "photo.jpg"]);
        assert_eq!(l.files, 2);
    }

    #[test]
    fn empty_listing_text_yields_empty() {
        let l = parse_listing(&Backend::Zip, "");
        assert!(l.names.is_empty());
        assert_eq!(l.files, 0);
    }
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
            ..Options::default()
        }
    }

    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn format_maps_extension_and_backend() {
        assert_eq!(Format::Zip.extension(), "zip");
        assert_eq!(Format::SevenZ.extension(), "7z");
        // Zip 格式跟随平台原生工具(Windows 上 zip 格式也由 7z -tzip 产出);
        // SevenZ 格式两平台统一 7z
        assert_eq!(Format::Zip.backend(), Backend::native());
        assert_eq!(Format::SevenZ.backend(), Backend::SevenZip);
    }

    #[test]
    fn ensure_tool_available_reports_missing_tool() {
        let err = ensure_tool_available("no-such-tool-xyz").unwrap_err();
        assert!(
            matches!(err, CoreError::ToolMissing(ref p) if p == "no-such-tool-xyz"),
            "实际: {err:?}"
        );
    }

    fn opts_fmt(format: Format, level: Option<u8>) -> Options {
        Options {
            split_size: None,
            password: None,
            random_name: false,
            format,
            level,
        }
    }

    #[test]
    fn sevenzip_command_7z_format_emits_lzma2() {
        let cmd = Backend::SevenZip
            .compress_command(
                Path::new("D:/data"),
                "A",
                Path::new("E:/out/A.7z"),
                &opts_fmt(Format::SevenZ, None),
            )
            .unwrap();
        assert_eq!(cmd.get_program(), "7z");
        assert_eq!(
            args_of(&cmd),
            ["a", "-t7z", "-m0=LZMA2", "--", "E:/out/A.7z", "A"]
        );
        assert_eq!(cmd.get_current_dir(), Some(Path::new("D:/data")));
    }

    #[test]
    fn sevenzip_command_7z_format_with_split_password_level() {
        let mut o = opts_fmt(Format::SevenZ, Some(9));
        o.split_size = Some(2 * 1024 * 1024 * 1024);
        o.password = Some("pw".into());
        let cmd = Backend::SevenZip
            .compress_command(Path::new("D:/data"), "B", Path::new("E:/out/B.7z"), &o)
            .unwrap();
        assert_eq!(
            args_of(&cmd),
            [
                "a", "-t7z", "-m0=LZMA2", "-v2g", "-ppw", "-mx=9", "--", "E:/out/B.7z", "B"
            ]
        );
    }

    #[test]
    fn sevenzip_command_zip_format_level_uses_mx() {
        let cmd = Backend::SevenZip
            .compress_command(
                Path::new("D:/data"),
                "A",
                Path::new("E:/out/A.zip"),
                &opts_fmt(Format::Zip, Some(9)),
            )
            .unwrap();
        assert_eq!(
            args_of(&cmd),
            ["a", "-tzip", "-mx=9", "--", "E:/out/A.zip", "A"]
        );
    }

    #[test]
    fn zip_command_level_uses_digit_flag() {
        // Info-ZIP zip 的级别 flag 就是数字本身(-0 仅存储 … -9 最大)
        let cmd = Backend::Zip
            .compress_command(
                Path::new("/in"),
                "A",
                Path::new("/out/A.zip"),
                &opts_fmt(Format::Zip, Some(9)),
            )
            .unwrap();
        assert_eq!(args_of(&cmd), ["-r", "-9", "/out/A.zip", "A"]);
    }

    #[test]
    fn level_zero_means_store() {
        let cmd = Backend::Zip
            .compress_command(
                Path::new("/in"),
                "A",
                Path::new("/out/A.zip"),
                &opts_fmt(Format::Zip, Some(0)),
            )
            .unwrap();
        assert_eq!(args_of(&cmd), ["-r", "-0", "/out/A.zip", "A"]);
    }

    #[test]
    fn sevenzip_compress_command_runs_in_work_dir_with_relative_name() {
        let cmd = Backend::SevenZip
            .compress_command(
                Path::new("D:/data"),
                "A",
                Path::new("E:/out/A.zip"),
                &opts(None, None),
            )
            .unwrap();
        assert_eq!(cmd.get_program(), "7z");
        assert_eq!(args_of(&cmd), ["a", "-tzip", "--", "E:/out/A.zip", "A"]);
        assert_eq!(cmd.get_current_dir(), Some(Path::new("D:/data")));
    }

    #[test]
    fn sevenzip_compress_command_with_password_and_split() {
        let cmd = Backend::SevenZip
            .compress_command(
                Path::new("D:/data"),
                "B",
                Path::new("E:/out/B.zip"),
                &opts(Some(2 * 1024 * 1024 * 1024), Some("pw")),
            )
            .unwrap();
        assert_eq!(
            args_of(&cmd),
            ["a", "-tzip", "-v2g", "-ppw", "--", "E:/out/B.zip", "B"]
        );
    }

    #[test]
    fn zip_compress_command_runs_in_work_dir_with_relative_name() {
        let cmd = Backend::Zip
            .compress_command(
                Path::new("/in"),
                "A",
                Path::new("/out/A.zip"),
                &opts(None, None),
            )
            .unwrap();
        assert_eq!(cmd.get_program(), "zip");
        assert_eq!(args_of(&cmd), ["-r", "/out/A.zip", "A"]);
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/in")));
    }

    #[test]
    fn zip_compress_command_prefixes_dash_leading_names() {
        // zip 没有 -- 分隔符, "-" 开头的条目名要 ./ 化防被当参数
        let cmd = Backend::Zip
            .compress_command(
                Path::new("/in"),
                "-weird",
                Path::new("/out/w.zip"),
                &opts(None, None),
            )
            .unwrap();
        assert_eq!(args_of(&cmd), ["-r", "/out/w.zip", "./-weird"]);
    }

    #[test]
    fn zip_extract_command_with_password() {
        let cmd =
            Backend::Zip.extract_command(Path::new("/out/B.zip"), Path::new("/dest"), Some("pw"));
        assert_eq!(cmd.get_program(), "unzip");
        assert_eq!(
            args_of(&cmd),
            ["-o", "-P", "pw", "/out/B.zip", "-d", "/dest"]
        );
    }

    #[test]
    fn sevenzip_extract_command_on_first_volume() {
        let cmd =
            Backend::SevenZip.extract_command(Path::new("B.zip.001"), Path::new("D:\\d"), None);
        assert_eq!(cmd.get_program(), "7z");
        assert_eq!(args_of(&cmd), ["x", "-aoa", "-oD:\\d", "--", "B.zip.001"]);
    }

    #[test]
    fn zip_compress_command_with_password_and_split() {
        let cmd = Backend::Zip
            .compress_command(
                Path::new("/in"),
                "B",
                Path::new("/out/B.zip"),
                &opts(Some(500 * 1024 * 1024), Some("pw")),
            )
            .unwrap();
        assert_eq!(
            args_of(&cmd),
            ["-r", "-s", "500m", "-P", "pw", "/out/B.zip", "B"]
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
