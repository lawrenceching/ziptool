# ziptool

本工具用于大数据盘搬运前, 批量压缩磁盘里的各个目录.

本工具支持:
1. 每个目录可压缩成单独的包; 桌面 UI 可选打包策略(小条目合并 / 超大条目分包, 见下)
2. 支持分包压缩
3. 支持随机压缩包名(避免压缩包名本身包含敏感信息, 导致信息泄露)
4. 两种压缩格式: zip(兼容性最好)与 7z(LZMA2, 压缩率更高), 以及 0-9 压缩级别
5. 排除规则: 桌面 UI 默认启用「操作系统隐藏文件 + 编程语言的临时文件」两组策略, 可在设置页逐项关闭; 系统隐藏只认**系统元数据名单**(`Thumbs.db` / `.DS_Store` 等), **不按文件隐藏属性或 `.` 前缀排除用户文件**; CLI 默认同样排除系统元数据, `--exclude` 自定义模式可叠加
6. 界面支持简体中文、繁体中文(香港)、繁体中文(台湾)、英语、俄语; 启动时跟随系统语言偏好, 也可手动切换并记忆
7. 桌面 UI 的**打包策略**: 小条目合并、超大条目分包; 扫描后自动生成方案供审核, 阈值可调, 改阈值/勾选不重新扫描
8. 桌面 UI 的**设置页**(边栏 + 内容区): 文件排除策略(操作系统隐藏文件 / 编程语言临时文件, 默认都开, 可逐项关闭)与「压缩完成后关机」占位(置灰, 后续版本提供); 设置持久化到用户配置目录
9. **应用内菜单栏**(窗口顶部, 视觉规格参考 shadcn/ui Menubar): 应用壳层为 **App = MenuBar / Content / StatusBar** 三段式; `应用`(报告问题 / 退出) + `设置`(设置…, 打开设置页); 菜单文案跟随界面语言; 「报告问题」用系统默认浏览器打开 GitHub 仓库, 「退出」先终止在跑任务再退出

**为什么不整个磁盘打包成一个压缩包**
一个数T的压缩包不利于运输. 且压缩, 复制, 上传, 下载过程都不容易中断与恢复.

**为什么不分包压缩整个磁盘**
一个分包失败或者错误, 可能会导致全部数据无法解压, 导致丢失所有数据.

ziptool 调用系统已有的压缩命令, 不自带压缩实现.

压缩后端 (分包使用各后端原生能力, 保持默认命名):
- Linux: zip 格式用系统 zip 命令, 分包 -s (B.z01, B.z02, ..., B.zip, 最后一卷是 .zip); 7z 格式用 7z 命令
- Windows: 7z 命令, zip 格式加 -tzip, 分包 -v (B.zip.001, B.zip.002, ...) (Windows 没有系统 zip 命令)

7z 格式说明见 `_docs/compress-modes.md`(压缩率高, 但解压端需要 7z 系工具).

## 例子

```
A (500MB) -> A.zip
B (26GB) -> B.z01, B.z02, ..., B.zip
C.jpg -> C.jpg.zip
D (7z 格式) -> D.7z
```

包名规则: 原名 + ".zip"(7z 格式为 ".7z"; 追加, 不替换已有扩展名, 保证解包端能完整还原原名).

## 打包策略(桌面 UI)

默认策略为: 合并≤100m、分包>2g、合并包≤跟随分包、单包条目≤50000。

- 扫描完成后自动计算方案; 策略行显示预计逻辑包数与最大直接/合并包
- "查看方案"打开只读抽屉: 包类型、成员、原始大小、条目数、提醒
- 改阈值或勾选只重算方案, **不重新扫描**
- 确认时壳层会重算并校验 `planId`, 方案变化则拒绝开工
- 审核只读, 不支持手动拖拽/拆分/合并包成员
- 大小为原始大小; 压缩后大小与实际分卷数未知

阈值含义:

- `合并≤`: 小于等于此大小的顶层条目进入合并池
- `分包>`: 大于此大小的条目按此大小切卷
- `合并包≤`: 单个合并包的原始大小上限; 留空跟随 `分包>`
- `条目≤`: 单个合并包的条目数上限; 留空不限制
- 默认值为二进制单位(100 MiB / 2 GiB); 若目的地按十进制 2GB 限制, 请自行下调

## 设置与文件排除(桌面 UI)

菜单栏 `设置 > 设置…` 进入设置页, 左侧边栏为「通用」「文件排除」。

- 应用壳层为 **App = MenuBar / Content / StatusBar** 三段式; 菜单栏是应用内自绘的 HTML, 视觉规格参考 shadcn/ui Menubar: 背景与 App 一致(无边框/无浮层容器, 无下拉箭头), 纯文字入口 + 下拉 popover 卡片 + 浅阴影, 菜单项右侧显示快捷键(退出 `Ctrl+Q`/`⌘Q`, 设置 `Ctrl+S`/`⌘S`), 「退出」为 destructive 红色; 打开带淡入/缩放/下滑动画
- **应用菜单**: 「报告问题」用系统默认浏览器打开 <https://github.com/lawrenceching/ziptool>; 「退出」在终止在跑任务后退出应用
- **设置菜单**: 「设置…」打开设置页; 设置加载完成前 / 扫描与任务运行中 / 方案抽屉打开时该项禁用
- 菜单文案跟随界面语言(简中 / 繁中港 / 繁中台 / English / Русский), 切换即时生效
- 交互对齐 shadcn Menubar: 鼠标 / 方向键 / Home / End / Esc、菜单打开时 `←` `→` 切换、hover 切换、首字 typeahead、打开淡入缩放动画

- **文件排除**列表:
  - 操作系统隐藏文件(默认开): 只排除系统元数据名单(`Thumbs.db`、`desktop.ini`、`.DS_Store`、`.Spotlight-V100` 等); **不按文件隐藏属性或 `.` 前缀排除** `.git` / `.env` / `.config` 等用户文件
  - 编程语言的临时文件(默认开): `node_modules`、`__pycache__`、`.gradle`、`*.class`、`*.tmp` 等工具生成的缓存/临时物
- 被排除的顶层条目仍显示在扫描表中, 默认不勾选, 可手动勾选覆盖
- 修改策略后, 关闭设置页会按新策略重扫当前压缩源; 解压模式不受影响
- **压缩完成后关机**: 本期仅占位置灰(标注「即将支持」), 不实现关机行为; 设置字段已预留
- 设置文件: `app_config_dir()/settings.json`(Windows `%APPDATA%\com.ziptool.desktop\settings.json`); 损坏/版本过新时回退默认值
- CLI **不读**该设置, 行为与参数保持原样

完整设计见 `_docs/filtering_rules.md`; 菜单栏实现见 `_docs/menu.md`。

## 项目结构

Rust workspace, 核心逻辑与外壳分离, 同一套核心同时支撑命令行和 Tauri UI:

- `crates/core` — 核心库: 批量压缩 / 分包 / 密码 / 随机包名 / zip 与 7z 格式 / 进度回调与取消 (封装系统压缩命令: Linux 用 zip/7z, Windows 用 7z)
- `crates/cli` — 命令行外壳 (clap), 二进制名 `ziptool`
- `crates/tauri-app` — Tauri 2 桌面应用, `#[tauri::command]` 包装核心库

核心库通过回调上报进度与取消, CLI 画进度条, Tauri 用 Channel 推送到前端.

## 开发

```
cargo build                            # 构建全部 (core + cli + tauri)
cargo test                             # 测试
cargo run -p zip-tool-cli -- --help    # 命令行
cargo run -p zip-tool-tauri            # 桌面应用
```

也可以用 devcontainer (仅 core + CLI, 不含 Tauri 系统依赖): 见 `_docs/devcontainer.md`。

发版用 `cargo bump-version <x.y.z>`: 同步 `Cargo.toml` / `Cargo.lock` / `tauri.conf.json`, 并做本地提交和 tag; **不会自动 push**, 推送由你手动执行。之后在 Actions 里运行 Release, 不需要输入版本号。详见 `_docs/release.md`。

## 构建 CLI

```
cargo build --release -p zip-tool-cli
```

产物: `target/release/ziptool.exe` (Linux 下为 `ziptool`), 单文件可直接复制使用.

```
# 批量压缩: 分包 2g + 密码 + 随机包名
ziptool compress -i D:\data -o E:\out --split 2g -p 密码 --random-name

# 7z 格式 (LZMA2) + 压缩级别 9 (0=仅存储, 缺省用后端默认)
ziptool compress -i D:\data -o E:\out --format 7z --level 9

# 解压 (支持 .zip/.7z 与 .zip.001/.7z.001 分卷; 传成员卷 .zNN / .zip.NNN / .7z.NNN 自动归一到入口卷)
ziptool extract -i E:\out\zt_xxx.zip.001 -o D:\restore -p 密码

# 批量解压: 输入传目录, 恢复压缩前的整棵树 (随机包名经 mapping.csv 还原原名)
ziptool extract -i E:\out -o D:\restore -p 密码

# 默认拒绝覆盖目标已有文件; 确认后用 --force 覆盖
ziptool extract -i E:\out -o D:\restore --force

# 排除规则: --exclude 可重复(不含/ = 任意深度文件名, 含/ = 从源根锚定路径)
ziptool compress -i D:\data -o E:\out --exclude Thumbs.db --exclude "*.tmp" --exclude node_modules/**

# 默认排除系统元数据(Thumbs.db/.DS_Store 等); --include-hidden-files 关闭(兼容旧参数名)
ziptool compress -i D:\data -o E:\out --include-hidden-files

# 环境自检: 定位并试运行后端工具, 打印路径与可用性 (全部正常时退出码 0)
ziptool doctor
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

UI 多语言使用 vendored i18next (见 `ui/vendor/`)。升级 i18next 时:

```
cd crates/tauri-app
npm install --save-dev --save-exact i18next@26.5.3
npm run vendor:i18n
```

界面文案 key 与语言匹配规则见 `_docs/i18n.md`。

正式打包:

```
cd crates/tauri-app && npx tauri build     # @tauri-apps/cli 已在 devDependencies, 无需全局安装
```

产物:
- Windows 安装包: `target/release/bundle/msi/*.msi` 与 `nsis/*-setup.exe`
- Linux 包: `target/release/bundle/deb|rpm|appimage/`
- macOS 包: `target/release/bundle/dmg/*.dmg`
- 独立可执行文件: `target/release/ziptool.exe` (前端资源已内嵌, 可单独分发; tauri build 会把 crate bin `zip-tool-tauri` 改名为 `ziptool.exe`, 与 CLI 构建产物同路径, 后构建者覆盖先构建者)

## 环境要求

- 构建: Rust 1.85+ (edition 2024); Windows 需 MSVC Build Tools; 打包桌面应用需 @tauri-apps/cli (已在 crates/tauri-app devDependencies)
- 桌面应用运行: Windows 10/11 自带 WebView2, 无需额外安装
- 压缩/解压通过外部命令完成 (见"压缩后端"): Windows 需 7z (7-Zip 或 NanaZip 均可), Linux 需系统 zip / unzip, 7z 系命令 (7z 格式压缩与解 .7z 需要)
