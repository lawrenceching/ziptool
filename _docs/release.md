# Release 技术文档

> 定位: Release 实现规格. 定义 `.github/workflows/release.yml` 的触发 / 门禁 / 构建 / 发布结构, 以及产品改名(ziptool)的范围. 可直接指导 workflow 编写.
> 前置事实: CI(`.github/workflows/ci.yml`, 8 job)已稳定全绿; 仓库 public(`lawrenceching/ziptool`); UI 为纯静态且 `tauri.conf.json` 无 `beforeBuildCommand`(`tauri build` 不需要前端构建步骤); workspace 共享根 `target` 目录(tauri bundle 输出在 `target/release/bundle/`).

## 1. 目标与非目标

### 目标

- 手动触发(`workflow_dispatch`, 输入一个已有 tag), 先校验该 commit 的 CI 全绿, 再构建并创建 GitHub Release
- 5 平台(与 CI test 矩阵一致: ubuntu-latest / ubuntu-24.04-arm / windows-latest / windows-11-arm / macos-latest)产出 **CLI 包 + Tauri 安装包**, 全部作为 Release 附件
- 产品名统一为 `ziptool`: CLI 二进制名 / Tauri `productName` / 发布包文件名

### 非目标

- 代码签名 / 公证 / 自动更新(updater)
- crate 内部名改名(`zip-tool-cli` / `zip-tool-core` / `zip-tool-tauri` 保持不变, 那是包管理层面名字)
- 版本号 bump 自动化(发版前手动改版本, 流程见 §7)

## 2. 产品改名(独立前置提交)

| 文件 | 改动 |
|---|---|
| `crates/cli/Cargo.toml` | `[[bin]] name = "zip-tool"` → `"ziptool"` |
| `crates/cli/src/main.rs` | clap `#[command(name = "zip-tool")]` → `"ziptool"` |
| `crates/tauri-app/tauri.conf.json` | `"productName": "zip-tool"` → `"ziptool"`(安装包 / deb / dmg 文件名随之变 `ziptool_*`) |
| `crates/tauri-app/tauri.conf.json`(顶层) | 增加 `"mainBinaryName": "ziptool"`(安装包内部主程序名,缺省是 crate bin 名 `zip-tool-tauri`;注意是顶层字段,不属 `bundle` 段) |
| `test/tests/cli_e2e.rs` | 二进制名 `"zip-tool.exe"` / `"zip-tool"` → `"ziptool.exe"` / `"ziptool"` |
| `README.md` | 用法示例与产物路径. 注意 `zip-tool-tauri` 作为 crate bin 名不改; 「独立可执行文件」产物行随 mainBinaryName 更新为 `ziptool.exe`(两者同路径, 后构建覆盖先构建) |

- `Cargo.lock` 不受影响(bin 名不进 lock)
- 命名注意区分三个层面: crate 名(不改)/ CLI bin 名(改 `ziptool`)/ Tauri productName(改 `ziptool`)
- 验证: `cargo test --workspace` + `cargo test -p zip-tool-e2e` + `cd crates/tauri-app && npm test` 全绿

## 3. 总体结构(单 workflow, 三个串行 job)

```
.github/workflows/release.yml  (name: Release)
    │  on: workflow_dispatch
    │    inputs.tag: 要发布的 tag(如 v0.1.0), required
    │  permissions: 顶层 contents: read; 仅 release job 提权 contents: write
    │  concurrency: release-${{ inputs.tag }}(cancel-in-progress: true, 防同一 tag 并发发布)
    ├── job verify   ubuntu-latest   tag/版本一致性校验 + 等待该 commit 的 CI 全绿
    ├── job build    5 平台矩阵      CLI 归档 + tauri bundle → upload-artifact
    └── job release  ubuntu-latest   download-artifact 合并 → gh release create
```

- 触发后所有 job 都以 `inputs.tag` 为唯一事实源: checkout 该 tag、校验、构建、发布
- verify 失败则整条链终止, 不会产出半成品 Release

## 4. verify job(门禁)

`timeout-minutes: 115`, 三步:

1. **checkout tag**: `actions/checkout@v4` with `ref: ${{ inputs.tag }}`; tag 不存在时直接失败
2. **版本三方一致性**: `${TAG#v}`(去 v 前缀)== 根 `Cargo.toml` 的 `[workspace.package] version`(`grep -m1 '^version ='` 取)== `crates/tauri-app/tauri.conf.json` 的 `version`(`jq -r .version` 取). 任一不等 → fail. 防止给旧代码打新版本号、或两处版本漂移
3. **等 CI 全绿**: `sha=$(git rev-parse HEAD)`, 用 `gh run list --workflow=CI --commit="$SHA" --json status,conclusion` 轮询(每 30s):
   - 尚无 run(索引延迟或 tag 刚推)→ 继续等, 上限 10 分钟
   - 最新 run `status != completed` → 继续等, 总上限 100 分钟(CI test job 自身 timeout 45 分钟 + 排队余量)
   - `conclusion != success` → fail
   - `conclusion == success` → 通过

环境: `GH_TOKEN: ${{ github.token }}`、`GH_REPO: ${{ github.repository }}`. 不引第三方等待 action, 纯 bash + gh.

verify job 需显式 `permissions: {contents: read, actions: read}`——顶层限定后未声明权限归 none, 而 `gh run list` 查 Actions API 需要 `actions: read`.

## 5. build job(5 平台矩阵)

`needs: verify`, `timeout-minutes: 60`, `fail-fast: false`. 每个 job:

1. checkout(`ref: inputs.tag`)+ `dtolnay/rust-toolchain@stable` + `Swatinem/rust-cache@v2`(`cache-on-failure: true`)
2. 平台依赖(照抄 ci.yml 相应步骤): Linux 装 webkit2gtk 等 Tauri 依赖; Windows / macOS 构建期无额外依赖(7z 是运行期依赖, release 构建不需要)
3. **CLI 归档**:
   - `cargo build --release -p zip-tool-cli` → `target/release/ziptool(.exe)`
   - triple: `rustc -vV | sed -n 's/^host: //p'`; 版本: `${TAG#v}`(verify 已保证与 Cargo.toml 一致)
   - unix: `tar czf dist/ziptool-cli-<ver>-<triple>.tar.gz -C target/release ziptool`
   - Windows(`shell: pwsh`): `Compress-Archive -Path target/release/ziptool.exe -DestinationPath dist/ziptool-cli-<ver>-<triple>.zip`(runner 预装, 零额外依赖)
4. **Tauri 包**: `setup-node@v4`(node 22, npm 缓存)→ `npm ci`(crates/tauri-app)→ `npx tauri build`
   - 前置: `crates/tauri-app/package.json` 增加 devDependency `@tauri-apps/cli: "^2"`, 本地 `npm install` 重生成 package-lock 后提交(npm 装平台预编译二进制, 5 平台全覆盖, 附赠本地 `npx tauri build` 能力)
   - 产物位置(workspace 根): `target/release/bundle/{deb,rpm,appimage,dmg,nsis,msi}/`
5. **归集**: 把 CLI 归档与 bundle 按 glob 拷入平铺的 `dist/`(`*.deb` `*.rpm` `*.AppImage` `*.dmg` `*-setup.exe` `*.msi`), `upload-artifact@v4` 上传, artifact 名 `bundle-<os>`(`matrix.os`, 已足够区分平台, 免去跨 step 传递 triple). macOS 只取 `.dmg`(不传 `.app` 目录)

产出物一览:

| 平台 | CLI | Tauri |
|---|---|---|
| ubuntu-latest | `ziptool-cli-0.1.0-x86_64-unknown-linux-gnu.tar.gz` | `ziptool_0.1.0_amd64.deb` / `.rpm` / `.AppImage` |
| ubuntu-24.04-arm | `…-aarch64-unknown-linux-gnu.tar.gz` | 同上, `arm64` |
| windows-latest | `ziptool-cli-0.1.0-x86_64-pc-windows-msvc.zip` | `ziptool_0.1.0_x64-setup.exe` / `_x64_en-US.msi` |
| windows-11-arm | `…-aarch64-pc-windows-msvc.zip` | 同上, `arm64` |
| macos-latest | `ziptool-cli-0.1.0-aarch64-apple-darwin.tar.gz` | `ziptool_0.1.0_aarch64.dmg` |

## 6. release job(发布)

`needs: build`, `timeout-minutes: 10`, `permissions: contents: write`.

1. `download-artifact@v4`: `pattern: bundle-*`, `merge-multiple: true`, path `dist/`
2. `gh release view "$TAG"` 存在 → `gh release upload "$TAG" dist/* --clobber`; 不存在 → `gh release create "$TAG" dist/* --title "ziptool $TAG" --generate-notes --verify-tag`(`--verify-tag` 确保远端 tag 存在)
3. non-draft 直接发布

## 7. 发版操作手册

1. 改版本号(两处必须同步): 根 `Cargo.toml` `[workspace.package] version` + `crates/tauri-app/tauri.conf.json` `version`
2. 提交并推 main, 等 CI 绿
3. `git tag v<版本> && git push origin v<版本>`
4. Actions → Release → Run workflow → 选 main, 输入 `v<版本>`
5. verify 等 CI(同一 commit 已绿则秒过)→ 5 平台构建 → Release 挂出全部附件

## 8. 风险与兜底

| 风险 | 兜底 |
|---|---|
| Linux arm64 的 AppImage 打包失败(Tauri 生态历史弱点) | 回退为该平台 `npx tauri build --bundles deb,rpm` |
| `windows-11-arm` 上 NSIS / WiX 打包失败 | 先实跑观察; 失败则该平台降级只出 CLI 归档(矩阵加 bundle 条件) |
| 版本漂移(tag ≠ Cargo.toml ≠ tauri.conf) | verify 第 2 步三方校验直接 fail, 不会带病发布 |
| 同一 tag 重复 dispatch 竞争 | `concurrency: release-<tag>` 取消旧 run; release job 的 view/upload 分支幂等 |
| tauri build 首次运行下载 NSIS / WiX 工具慢 | 计入 build timeout 60 分钟余量; rust-cache 不缓存该目录, 接受每次下载 |
| CLI 与 GUI 独立可执行文件同为 target/release/ziptool.exe, 后构建覆盖先构建 | Release 流程 CLI 归档步骤固定先于 tauri build(顺序写死在 yml 并加注释) |

## 9. 验证方式(bootstrap)

workflow 无法本地完整验证(依赖真实 runner), 沿用 CI 的分支迭代流程:

1. yml 静态检查: actionlint(本机跑一次)
2. 专用分支(如 `ci/release-bootstrap`)上: 打临时 tag(指向分支头, 版本与当时 Cargo.toml 一致, 如 `v0.1.0`)→ dispatch(选分支, tag 输入该临时 tag)→ verify 会等该 commit 的 CI(分支 push 已触发)→ 观察 5 平台构建与 Release 内容
3. 每轮失败修完 push 重跑; 全链路绿后: 删测试 Release 与临时 tag → ff-only 合回 main → main 上再跑一次确认
4. 改名部分(§2)本地全量测试绿后才进入 workflow 阶段
