//! 桌面平台集成: 用系统默认浏览器打开外部链接(不引入 opener 插件)。
//!
//! 只按平台调用系统启动器, 只负责启动、不等待浏览器退出; 启动器进程由后台线程回收。

use std::io;

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
