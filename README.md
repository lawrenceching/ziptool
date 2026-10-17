# Zip Tool

本工具用于大数据盘搬运前, 批量压缩磁盘里的各个目录.

本工具支持:
1. 每个目录压缩成单独的包
2. 支持分包压缩
3. 支持随机压缩包名(避免压缩包名本身包含敏感信息, 导致信息泄露)

**为什么不整个磁盘打包成一个压缩包**
一个数T的压缩包不利于运输. 且压缩, 复制, 上传, 下载过程都不容易中断与恢复.

**为什么不分包压缩整个磁盘**
一个分包失败或者错误, 可能会导致全部数据无法解压, 导致丢失所有数据.

Zip Tool 调用的是系统本身的zip命令, 不自带压缩实现.

压缩后端 (分包使用各后端原生能力, 保持默认命名):
- Linux: 系统 zip 命令, 分包 -s (B.z01, B.z02, ..., B.zip, 最后一卷是 .zip)
- Windows: 7z 命令, 分包 -v (B.zip.001, B.zip.002, ...) (Windows 没有系统 zip 命令)

## 例子

```
A (500MB) -> A.zip
B (26GB) -> B.z01, B.z02, ..., B.zip
C.jpg -> C.zip
```

## 项目结构

Rust workspace, 核心逻辑与外壳分离, 同一套核心同时支撑命令行和 Tauri UI:

- `crates/core` — 核心库: 批量压缩 / 分包 / 密码 / 随机包名 / 进度回调与取消 (封装系统压缩命令: Linux 用 zip, Windows 用 7z)
- `crates/cli` — 命令行外壳 (clap), 二进制名 `zip-tool`
- `crates/tauri-app` — Tauri 2 桌面应用, `#[tauri::command]` 包装核心库

核心库通过回调上报进度与取消, CLI 画进度条, Tauri 用 Channel 推送到前端.

## 开发

```
cargo build                            # 构建全部 (core + cli + tauri)
cargo test                             # 测试
cargo run -p zip-tool-cli -- --help    # 命令行
cargo run -p zip-tool-tauri            # 桌面应用
```
