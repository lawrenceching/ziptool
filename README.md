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
C.jpg -> C.jpg.zip
```

包名规则: 原名 + ".zip"(追加, 不替换已有扩展名, 保证解包端能完整还原原名).

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

## 构建 CLI

```
cargo build --release -p zip-tool-cli
```

产物: `target/release/zip-tool.exe` (Linux 下为 `zip-tool`), 单文件可直接复制使用.

```
# 批量压缩: 分包 2g + 密码 + 随机包名
zip-tool compress -i D:\data -o E:\out --split 2g -p 密码 --random-name

# 解压 (支持 .zip 与 .zip.001 分卷; 传成员卷 .zNN / .zip.NNN 自动归一到入口卷)
zip-tool extract -i E:\out\zt_xxx.zip.001 -o D:\restore -p 密码

# 批量解压: 输入传目录, 恢复压缩前的整棵树 (随机包名经 mapping.csv 还原原名)
zip-tool extract -i E:\out -o D:\restore -p 密码

# 默认拒绝覆盖目标已有文件; 确认后用 --force 覆盖
zip-tool extract -i E:\out -o D:\restore --force

# 环境自检: 定位并试运行后端工具, 打印路径与可用性 (全部正常时退出码 0)
zip-tool doctor
```

## 构建桌面应用

日常调试:

```
cargo run -p zip-tool-tauri
```

UI 样式用 Tailwind 预编译成静态 `ui/app.css`(运行时离线, 无前端打包器)。改动界面类名后重新生成:

```
cd crates/tauri-app && npm install && npm run css
```

UI 逻辑有 Node 单测(命令名/参数形状/进度协议等前后端契约 + 界面行为;`window.__TAURI__` 表面用真实 `@tauri-apps/api` 包对象、只 mock IPC 传输, 因此 UI 引用不存在的全局 API 路径会在测试期直接 TypeError, 不必等真机手测):

```
cd crates/tauri-app && npm install && npm test
```

正式打包:

```
npm install -g @tauri-apps/cli      # 或: cargo install tauri-cli --version "^2"
cargo tauri build
```

产物:
- Windows 安装包: `target/release/bundle/msi/*.msi` 与 `nsis/*.exe`
- Linux 包: `target/release/bundle/deb|rpm|appimage/`
- 独立可执行文件: `target/release/zip-tool-tauri.exe` (前端资源已内嵌, 可单独分发)

## 环境要求

- 构建: Rust 1.85+ (edition 2024); Windows 需 MSVC Build Tools; 打包桌面应用需 tauri-cli
- 桌面应用运行: Windows 10/11 自带 WebView2, 无需额外安装
- 压缩/解压通过外部命令完成 (见"压缩后端"): Windows 需 7z (7-Zip 或 NanaZip 均可), Linux 需系统 zip / unzip
