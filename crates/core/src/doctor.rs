//! 环境自检(供 CLI `doctor` 子命令与未来 UI 的环境状态页共用):
//! 定位当前平台涉及的全部工具(见 [`platform_tools`])的完整路径, 并试运行一次。
//! "在 PATH 里"不等于"能用"(例如损坏的 zip.exe 缺 DLL 时定位得到但起不来),
//! 所以除了找路径还要真的跑一次, 用输出首行当版本信息。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 一项环境检查结果(当前仅"后端工具"一项)
#[derive(Debug, Clone)]
pub struct ToolCheck {
    /// 工具名(zip / 7z)
    pub program: String,
    /// 定位到的完整路径;未在 PATH 中找到为 None
    pub path: Option<PathBuf>,
    /// 试运行结果:Ok 为可运行(首行输出, 无输出为 None);Err 为定位失败或起不来
    pub probe: std::result::Result<Option<String>, String>,
}

impl ToolCheck {
    /// 该项检查是否通过
    pub fn is_ok(&self) -> bool {
        self.path.is_some() && self.probe.is_ok()
    }
}

/// 当前平台压缩/解压涉及的完整工具列表:
/// Windows 压缩(zip 与 7z 格式)与解压全走 7z;Linux 压缩用 zip / 7z, 解压用 unzip / 7z。
pub fn platform_tools() -> &'static [&'static str] {
    if cfg!(windows) {
        &["7z"]
    } else {
        &["zip", "7z", "unzip"]
    }
}

/// 检查平台全部工具(从 PATH 定位 + 试运行), CLI doctor 用
pub fn check_platform_tools() -> Vec<ToolCheck> {
    let dirs: Vec<PathBuf> =
        std::env::split_paths(std::env::var_os("PATH").as_deref().unwrap_or_default()).collect();
    platform_tools().iter().map(|p| check_tool(p, &dirs)).collect()
}

/// 检查指定工具(搜索目录可注入, 便于测试)
pub fn check_tool(program: &str, dirs: &[PathBuf]) -> ToolCheck {
    let path = locate_tool(program, dirs);
    let probe = match &path {
        Some(p) => probe_tool(p),
        None => Err("not found in PATH".into()),
    };
    ToolCheck {
        program: program.into(),
        path,
        probe,
    }
}

/// 在目录列表中按顺序定位工具, 返回第一个存在的可执行文件路径。
/// Windows 下无扩展名的名字会同时尝试 ".exe"。
pub fn locate_tool(program: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter()
        .flat_map(|dir| exe_variants(program).into_iter().map(move |n| dir.join(n)))
        .find(|p| p.is_file())
}

/// 试运行工具(无参数调用, 退出码不关心), 取 stdout(其次 stderr)的第一个非空行。
/// 进程起不来(不存在、缺 DLL 导致加载失败等)返回 Err。
pub fn probe_tool(path: &Path) -> std::result::Result<Option<String>, String> {
    let out = Command::new(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot execute: {e}"))?;
    match first_line(&out.stdout).or_else(|| first_line(&out.stderr)) {
        Some(line) => Ok(Some(line)),
        // 起来了但没有任何输出:正常退出算通过, 异常退出(如加载器崩了)算失败
        None if out.status.success() => Ok(None),
        None => Err(format!("no output, exited with {}", out.status)),
    }
}

/// 第一个非空(去首尾空白后)行
fn first_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(String::from)
}

/// 工具在目录里的候选文件名:Windows 补 ".exe"(已带扩展名则原样)
fn exe_variants(program: &str) -> Vec<String> {
    if cfg!(windows) && Path::new(program).extension().is_none() {
        vec![format!("{program}.exe"), program.to_string()]
    } else {
        vec![program.to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::unique_temp;
    use std::fs;

    /// 平台约定的可执行文件名(Windows 补 .exe)
    fn exe_name(program: &str) -> String {
        if cfg!(windows) {
            format!("{program}.exe")
        } else {
            program.to_string()
        }
    }

    #[test]
    fn locate_tool_finds_first_match_in_dir_order() {
        let (a, b) = (unique_temp("doctor-a"), unique_temp("doctor-b"));
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let name = exe_name("mytool");
        fs::write(b.join(&name), b"").unwrap();
        let found = locate_tool("mytool", &[a.clone(), b.clone()]).unwrap();
        assert_eq!(found, b.join(name), "应命中更靠前的含工具目录 b");
        fs::remove_dir_all(&a).unwrap();
        fs::remove_dir_all(&b).unwrap();
    }

    #[test]
    fn locate_tool_skips_directories_and_returns_none_when_absent() {
        let d = unique_temp("doctor-dir");
        fs::create_dir_all(d.join(exe_name("mytool"))).unwrap();
        assert_eq!(
            locate_tool("mytool", &[d.clone()]),
            None,
            "同名目录不算工具"
        );
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn probe_tool_errors_for_missing_program() {
        let p = unique_temp("doctor-missing").join(exe_name("no-such-tool"));
        let r = probe_tool(&p);
        assert!(r.is_err(), "不存在的程序应报错, 实际 {r:?}");
    }

    #[test]
    fn probe_tool_returns_first_output_line_ignoring_exit_code() {
        // cargo 不带参数会打印用法且退出码非 0:试运行只关心"起得来、有输出"
        let r = probe_tool(Path::new(env!("CARGO")));
        let line = r.expect("cargo 应可运行").expect("cargo 应有输出");
        assert!(!line.trim().is_empty());
    }

    #[test]
    fn check_tool_reports_not_found_as_failed() {
        let c = check_tool("no-such-tool-xyz", &[]);
        assert_eq!(c.path, None);
        assert!(c.probe.is_err());
        assert!(!c.is_ok());
    }

    #[test]
    fn check_tool_locates_and_probes() {
        let d = unique_temp("doctor-ok");
        fs::create_dir_all(&d).unwrap();
        // 用 cargo 当"工具", 保证测试不依赖环境装了 zip/7z
        fs::copy(env!("CARGO"), d.join(exe_name("cargo"))).unwrap();
        let c = check_tool("cargo", &[d.clone()]);
        assert_eq!(c.path, Some(d.join(exe_name("cargo"))));
        assert!(c.probe.is_ok(), "试运行应成功: {:?}", c.probe);
        assert!(c.is_ok());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn platform_tools_cover_both_formats_and_extract() {
        let tools = platform_tools();
        if cfg!(windows) {
            // Windows 压缩(zip 与 7z 格式)与解压全走 7z
            assert_eq!(tools, ["7z"]);
        } else {
            // Linux: 压缩 zip 格式用 zip, 7z 格式与解 .7z 用 7z, 解 zip 用 unzip
            assert_eq!(tools, ["zip", "7z", "unzip"]);
        }
    }
}
