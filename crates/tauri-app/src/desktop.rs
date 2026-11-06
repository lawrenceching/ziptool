//! 桌面平台集成: 用系统默认浏览器打开外部链接、在系统文件管理器中定位文件
//! (不引入 opener 插件)。
//!
//! 只按平台调用系统启动器, 只负责启动、不等待浏览器退出; 启动器进程由后台线程回收。

use std::io;
use std::path::Path;

/// 用系统默认浏览器打开 URL; 只负责启动, 不等待浏览器退出。
#[cfg(target_os = "windows")]
pub fn open_external_url(url: &str) -> io::Result<()> {
    use std::os::windows::process::CommandExt;

    // CREATE_NO_WINDOW: GUI 程序调 cmd 时不闪控制台窗口
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let child = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    reap_in_background(child);
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn open_external_url(url: &str) -> io::Result<()> {
    let child = std::process::Command::new("open").arg(url).spawn()?;
    reap_in_background(child);
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn open_external_url(url: &str) -> io::Result<()> {
    let child = std::process::Command::new("xdg-open").arg(url).spawn()?;
    reap_in_background(child);
    Ok(())
}

#[cfg(not(any(windows, unix)))]
pub fn open_external_url(_url: &str) -> io::Result<()> {
    Err(io::Error::other("当前平台不支持打开外部链接"))
}

/// 后台线程回收子进程, 避免僵尸进程(启动器通常很快退出)。
fn reap_in_background(mut child: std::process::Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

/// 在系统文件管理器中定位文件(查重「定位」动作): Windows 资源管理器选中文件,
/// macOS Finder reveal, Linux 打开父目录(xdg-open 无「选中」语义)。
/// 只负责启动, 不等待文件管理器退出(其退出码不可靠); 子进程后台回收。
#[cfg(target_os = "windows")]
pub fn reveal_path(path: &Path) -> io::Result<()> {
    // explorer 是 GUI 程序, 不加 CREATE_NO_WINDOW
    let child = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn()?;
    reap_in_background(child);
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn reveal_path(path: &Path) -> io::Result<()> {
    let child = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn()?;
    reap_in_background(child);
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn reveal_path(path: &Path) -> io::Result<()> {
    let parent = path.parent().unwrap_or(path);
    let child = std::process::Command::new("xdg-open").arg(parent).spawn()?;
    reap_in_background(child);
    Ok(())
}

#[cfg(not(any(windows, unix)))]
pub fn reveal_path(_path: &Path) -> io::Result<()> {
    Err(io::Error::other("当前平台不支持定位文件"))
}
