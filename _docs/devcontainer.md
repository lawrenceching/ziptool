# devcontainer

本项目的 devcontainer 只服务 **core + CLI** 的开发与测试, 不包含 Tauri 桌面应用所需的
WebKit/GTK 系统依赖。

## 使用

VS Code: 打开仓库后执行 `Dev Containers: Reopen in Container`。

命令行 (需安装 devcontainer CLI):

```
devcontainer up --workspace-folder .
devcontainer exec --workspace-folder . bash
```

首次创建会构建镜像 (安装 `zip`/`unzip`/`p7zip-full`) 并通过 Node feature 安装 Node 24,
需要联网。

## 容器内可用的命令

```
cargo build -p zip-tool-core -p zip-tool-cli
cargo run -p zip-tool-cli -- --help
cargo test -p zip-tool-core -p zip-tool-cli
cargo test -p zip-tool-e2e

cd crates/tauri-app && npm ci && npm test
cd crates/tauri-app && npm run css
```

`zip-tool-e2e` 无 crate 依赖, 只会自行编译 `zip-tool-cli`, 因此同样可以在容器内运行。
UI 单测只跑 Node, 不编译 Rust, 也不需要 Tauri 系统依赖。

## 容器内不可用的命令

```
cargo build --workspace
cargo build -p zip-tool-tauri
cargo tauri build
```

`zip-tool-tauri` 在 Linux 上编译需要 `libwebkit2gtk-4.1-dev`、`libgtk-3-dev` 等系统库,
本镜像刻意不安装。容器内请用 `-p` 显式指定包, 不要使用 `--workspace`。

## 镜像与配置

- `.devcontainer/Dockerfile`: 基于 `mcr.microsoft.com/devcontainers/rust:1-bookworm`,
  额外安装压缩后端 `zip`/`unzip`/`p7zip-full` (与 CI 的 Linux 分支一致)。
- `.devcontainer/devcontainer.json`: 通过 Node feature 安装 Node 24; `target` 使用
  命名卷挂载, 与宿主机 (尤指 Windows) 的构建产物隔离, 避免平台产物互相污染。

要扩成包含 Tauri 的全量环境, 在 Dockerfile 中补装 CI 里列出的 WebKit/GTK 依赖即可。
