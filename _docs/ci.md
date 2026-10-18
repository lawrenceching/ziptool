# CI 技术文档

> 定位: CI 实现规格. 定义 `.github/workflows/ci.yml` 的 job 结构 / 平台矩阵 / 各平台依赖准备, 以及落地前的 lint 清零前置. 可直接指导 workflow 编写.
> 前置事实: 仓库 public(`lawrenceching/ziptool`), ARM 托管 runner 对 public 仓库免费; 压缩后端选择是 `cfg!(windows)` 二分(macOS 走 Linux 同款 zip/7z 路径); e2e(`test/tests/cli_e2e.rs`)解压校验直接调用系统 `zip`/`unzip`/`7z` 命令名.

## 1. 目标与非目标

### 目标

- 每次 push(所有分支)与 PR 自动触发检查: Rust 单测 + 端到端测试 + UI Node 单测 + lint(clippy / fmt)
- 覆盖 5 平台: Linux x64 / Linux arm64 / Windows x64 / Windows arm64 / macOS arm64
- 两条压缩后端路径都被真实执行: Linux 系(zip/unzip + 7z, 由 Linux 双架构与 macOS 覆盖)与 Windows(7z, 由 Windows 双架构覆盖)

### 非目标

- 不构建/上传发布产物(installer 等), 打包另行处理
- 不含覆盖率 / 性能基准
- 不在本 workflow 内做 tauri 桌面包 bundle(只编译 + 单测)

## 2. 总体结构(单 workflow, 三个并行 job)

```
.github/workflows/ci.yml  (name: CI)
    │  on: push + pull_request + workflow_dispatch
    │  concurrency: 按 ref 分组, cancel-in-progress(缓解 PR 分支 push+PR 双触发)
    ├── job fmt      ubuntu-latest            cargo fmt --all --check        (~1 分钟)
    ├── job clippy   [ubuntu-latest,          cargo clippy --workspace
    │                 windows-latest]          --all-targets -- -D warnings
    └── job test     5 平台矩阵(fail-fast: false, timeout-minutes: 45)
                       cargo test --workspace          # Rust 单测(含 tauri-app)
                       cargo test -p zip-tool-e2e      # e2e(嵌套构建 CLI 到 target/e2e, 无锁冲突)
                       npm ci && npm test              # UI Node 单测(Node 22)
```

- clippy 只跑 ubuntu + windows 两个 runner: 源码平台分支只有 `cfg!(windows)` 二分, 这两个覆盖全部 cfg 家族; 不必 5 平台各付一次 clippy 全量编译(与 test 的编译指纹不同, 无法复用)
- fmt 平台无关, 单独一个轻量 job
- `fail-fast: false`: 一个平台失败, 其余平台照常跑完出结果
- toolchain: `dtolnay/rust-toolchain@stable`(edition 2024 需 Rust 1.85+, stable 恒满足) + `Swatinem/rust-cache@v2`(按 job 自动区分缓存 key, e2e 的 `target/e2e` 在 `target/` 下随缓存一并保留)

## 3. test 矩阵各平台细节

| runner label | 后端准备 | 备注 |
|---|---|---|
| `ubuntu-latest` | `apt-get install p7zip-full zip unzip` + Tauri 依赖 | zip/unzip 本已预装, 装了无害 |
| `ubuntu-24.04-arm` | 同上 | apt 源 arm64 有同款包(含 webkit2gtk-4.1) |
| `windows-latest` | `where 7z` 防御性检查, 缺则 `choco install 7zip` | 镜像通常自带 7-Zip 且在 PATH |
| `windows-11-arm` | 同上 | 镜像较新(2026 起由 GitHub 维护), 7z/MSVC ARM64 待实跑验证 |
| `macos-latest` | `brew install sevenzip` + `ln -sf 7zz → 7z` | zip/unzip 预装; **Homebrew 的 sevenzip 只装 `7zz`, 不带 `7z` 符号链接**, 而 e2e 直接调 `7z`, 必须补链 |

Linux 的 Tauri 依赖(编译 `crates/tauri-app` 所需, 两架构一致):

```
libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev
```

UI Node 单测在全部 5 平台跑(`npm ci` + `npm test`, jsdom 纯 JS; `node_modules` 已 gitignore, `setup-node` 开 npm 缓存)。

## 4. 前置清零(进 CI 前必须完成, 否则第一天就红)

现状(2026-10-19 本地实测): `cargo fmt --all --check` 有 diff; `cargo clippy --workspace --all-targets` 共 10 处警告(cli / tauri / e2e 干净, 全在 core)。

1. `cargo fmt --all` 一次性格式化, 独立提交
2. 修复 clippy 警告(纯机械替换), 独立提交:
   - `core/src/lib.rs:15` 文档 link reference 写法
   - `core/src/backend.rs:466,468,470` `% == 0` → `is_multiple_of()`
   - `core/src/compress.rs:110` `&mut Vec<_>` → `&mut [_]`
   - `core/src/compress.rs:291` `iter().any()` → `contains()`(两处)
   - `core/src/exclude.rs:67` 可折叠 `if`
   - `core/src/doctor.rs:132,168` `clone` → `std::slice::from_ref`
3. 本地回归: `cargo test` + `cargo test -p zip-tool-e2e` + `cd crates/tauri-app && npm test` 全绿后再推 CI

## 5. 风险与兜底

| 风险 | 兜底 |
|---|---|
| `windows-11-arm` 镜像缺 7z 或 MSVC ARM64 组件不齐 | 7z 走 choco 防御安装; 若 tauri-app 在该平台编译失败, 降级为该平台只测 core/cli/e2e |
| macOS `7zz → 7z` 软链遗漏 | e2e 全红的失败信息非常直白(找不到 `7z`); 软链步骤放在 test 之前的独立 step, 日志可查 |
| ubuntu 缺 p7zip 导致 `--format 7z` 用例失败 | 显式 `apt-get install p7zip-full`, 不赌镜像预装 |
| PR 分支 push + pull_request 双触发重复消耗 | concurrency 按 ref 取消旧 run; public 仓库免费, 可接受 |

## 6. 验证方式与后续

- workflow 与清零提交推到分支后, `gh run watch` 盯完 5 平台真实绿灯才算完成; `windows-11-arm` 与 macOS 是重点观察对象
- CI 稳定后的可选后续(不在本次范围): `main` 开分支保护, 将 `CI` workflow 设为 required check, PR 不绿不能合
