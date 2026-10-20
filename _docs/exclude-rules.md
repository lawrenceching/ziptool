# 排除规则(系统隐藏 + 自定义排除模式)技术文档

> 2026-10-20 修订: 只排**系统元数据名单**; 不再使用 Unix `.` 前缀, 也不再使用 Windows 隐藏属性。用户设置的隐藏文件/文件夹一律保留。
>
> 定位: 目标实现规格. 在 _docs/compress.md 基础上引入**排除维度**: 扫描统计与后端命令两层同步生效.
>
> **核心决策(2026-10 修订)**:
> - 底层(core/CLI)通过**排除模式**实现, 支持多个模式叠加生效
> - 系统隐藏 = **系统元数据名单**(`Thumbs.db` / `.DS_Store` / `lost+found` 等), 三平台一致
> - **不使用任何文件隐藏属性**: Windows `attrib +H`、macOS 隐藏位、Unix `.` 前缀都不参与判定
> - 用户自己设置的隐藏文件/文件夹默认保留; 需要排除时用 `--exclude` / 高级区自定义模式
> - CLI 默认排除系统元数据, `--include-hidden-files` 关闭(兼容旧参数名)
> - UI 的 `os-hidden` 策略语义与 CLI 一致; 被标行显示但默认不勾选
> - UI 设置页 `os-hidden` 默认开; 关闭后不再排除系统元数据

## 1. 目标与非目标

### 目标

- 压缩时按规则排除文件, 避免垃圾文件进包(Thumbs.db / .DS_Store / desktop.ini / 临时文件等)
- 策略层(设置页「操作系统隐藏文件 / 编程语言的临时文件」)见 [filtering_rules.md](filtering_rules.md): 策略 id 由 `filter::effective_rules` 展开为本文件的 `ExcludeRules`, 不新增第二条排除管线
- 规则双轨生效(满足 todo 关键约束):
  - **扫描统计层**: 被排除文件的尺寸从 `total_bytes`/条目 size 中扣除, 进度权重与预检不偏
  - **后端命令层**: 压缩命令带排除开关, 被排除文件确实不进包
- **不变式: 统计口径 ≡ 后端实压内容**(任何护栏回退都以"加回统计"保持一致)
- 顶层条目: 扫描永远返回全部并打标; CLI 丢弃被标条目, UI 渲染为不勾选

### 非目标

- 解压侧不动(extract.md 不受影响; mapping.csv 在 out_dir, 与排除无关)
- 无否定模式(`!foo` 例外规则); 无按大小/时间的排除
- 不改"逐条目独立压缩包"编排与事件协议
- zip 后端的目录段边界语义缝隙不修补(接受过度匹配, 见 §3)

## 2. 参数面(core)

> 策略层与自定义模式的组合由 `filter::effective_rules` 统一展开; 展开后仍是下面这个结构。

```rust
/// 排除规则: 系统元数据模式 + 用户自定义模式, 叠加(并集)生效
pub struct ExcludeRules {
    /// false(默认) = 加入系统元数据默认集; true = 不加入。
    /// 不涉及任何文件隐藏属性/`.` 前缀。
    pub include_hidden: bool,
    /// 系统元数据 + 用户模式, 多条叠加; 无否定语义
    pub patterns: Vec<String>,
}

pub struct Options {
    // ...split_size / password / random_name / format / level 不变...
    pub exclude: ExcludeRules,   // Default = 加入系统元数据默认集、无用户模式
}

/// 顶层条目的排除标记(仅顶层; 内部文件不逐个上报)
pub enum ExcludeReason {
    /// 模式命中(系统元数据默认集或用户/策略模式; 保留命中模式供 UI 展示)
    Pattern(String),
}
```

- `Entry` 增加 `exclude: Option<ExcludeReason>`; `ScanReport` 增加 `excluded: Vec<ExcludedEntry>`(name + reason, CLI 提示用)
- 分层职责:
  - **顶层**: `scan_dir` 永远返回全部条目并打标(生效模式匹配); CLI 默认丢弃被标条目(`include_hidden=true` 时无默认元数据模式), UI 渲染为不勾选(既有勾选名单机制消化)
  - **内部**: `dir_size` 遍历时跳过被排除文件/子树(不下钻被排除目录), 尺寸扣除
- 模式命中目录 = 整棵子树排除(统计 + 后端一致)
- 排除开关的位置**按后端可执行文件**(Info-ZIP 与 7z 语法不同):
  - `zip`(Linux zip 格式): `-x` 开关必须在**条目名之后**(Info-ZIP 语法 `zip -r out.zip A -x pat...`)
  - `7z`(Windows zip 格式 + 全部 7z 格式): `-x!` 开关在 `--` 之前、条目名之前(既有 flags → `--` → 条目名次序不变)

## 3. 统一模式语法与归一化

### 语法

- 匹配对象: 不含 `/` 的模式 → **任意深度的文件/目录名**(basename, 如 `Thumbs.db`、`*.tmp`); 含 `/` 的模式 → 从条目根**锚定匹配相对路径**(如 `sub/*.log`、`node_modules/**`)
- 通配: `*` 不跨 `/`; `?` 单字符不跨 `/`; `**` 跨 `/`, **只允许出现在模式结尾**(`prefix/**`); 其余字符字面匹配
- 大小写敏感; 分隔符统一 `/`(输入侧容忍 `\` 归一为 `/`)
- 非法模式(空串 / `**` 不在结尾)在解析期报错, 宁停不错

### 归一化(按后端可执行文件, 而非格式)

| 目标后端 | 不含 `/` 模式 p(任意深度 basename) | 含 `/` 锚定模式 | `prefix/**`(at-or-below) |
|---|---|---|---|
| `zip` 命令 | `-x "*/p"` + `-x "*/p/*"`(双开关, 见下注) | `-x "*/<p>"` + `-x "*/<p>/*"`(无 `**` 段) | `-x "*/prefix"` + `-x "*/prefix/*"` |
| `7z` 命令 | `-xr!p`(递归名排除) | `-x!<条目名>/<p>`(`\` 按平台分隔符, 精确锚定) | `-x!<条目名>/prefix`(7z 排除目录即整树) |

- 注(zip 双开关): zip 对被排除目录是否继续下钻行为不确定, `*/p` 覆盖目录自身与树内文件、`*/p/*` 兜底树内路径, 两条一起保证目录排除; 空目录条目可能残留(接受, 见 §1 非目标)。

- **已知语义缝隙**: zip 通配无目录段边界, `*/sub/*.log` 可能过度匹配更深层同名路径 —— 接受并文档标注(zip = 最大兼容模式, 排除精度次要); 7z 后端精确

## 4. 系统隐藏机制(2026-10 修订)

三平台统一: **只按系统元数据名单排除**。

- 不使用 Windows 文件隐藏属性(`FILE_ATTRIBUTE_HIDDEN`), 因此用户 `attrib +H` 的普通文件仍会进包
- 不使用 macOS Finder/UF_HIDDEN 位
- 不使用 Unix `.` 前缀
- 名单外的文件一律保留; 需要排除时用 `--exclude` / 高级区自定义模式
- `include_hidden = true` 表示**不加入**系统元数据默认集; 不涉及任何文件属性判定
- 删除属性枚举后, 不再有路径收集器/属性预算护栏; `EXCLUDE_ARG_BUDGET` 仅约束后端排除开关长度

### 4.1 系统元数据名单(跨平台)

`exclude::OS_METADATA_PATTERNS`(core 默认集; UI/CLI 共用):

- Windows: `Thumbs.db`, `ehthumbs.db`, `desktop.ini`, `$RECYCLE.BIN`, `System Volume Information`
- macOS: `.DS_Store`, `._*`, `__MACOSX`, `.Spotlight-V100`, `.Trashes`, `.fseventsd`, `.TemporaryItems`, `.apdisk`, `.VolumeIcon.icns`, `.com.apple.timemachine.donotpresent`, `.DocumentRevisions-V100`, `.PKInstallSandboxManager`, `.MobileBackups`
- Linux: `lost+found`, `.Trash`, `.Trash-*`, `.directory`, `.fuse_hidden*`, `.nfs*`

这些是系统自身生成的元数据, 可以安全排除; 名单外的 `.` 文件一律保留。

## 5. CLI

- `compress`:
  - `--exclude <模式>`, 可重复, 与系统元数据名单叠加
  - `--include-hidden-files`, 关闭系统元数据默认集(兼容旧参数名; 不再涉及隐藏属性)
- 扫描后被标条目打印提示(如 `excluded (pattern "Thumbs.db"): Thumbs.db`), 默认不压缩
- `extract` 无新参数

## 6. UI(Tauri)

- `scan` 命令新增可选参数 `filters?: { enabledPolicies }`(设置页策略)与 `excludes?: string[]`(高级区输入, 多行/逗号分隔); 壳层经 `filter::effective_rules` 展开, ScanContext 连带缓存展开后的规则
- `ScanItem` 新增 `excluded?: string`(原因文案:「隐藏文件」/「匹配排除模式: <p>」); 前端被标行**显示但默认不勾选**, detail 列标原因
- `compress` DTO 同步加 `filters` / `excludes`; 与扫描缓存规则不一致 = 报"扫描已过期"(沿用 5.4 保鲜语义)
- UI 的 `include_hidden` 由 `os-hidden` 策略决定(默认 false); core `effective_patterns` 据此加入系统元数据默认集; 浏览器 mock 路径同步

## 7. 测试计划

- **core 单测**: 统一语法解析(非法模式报错)与归一化矩阵(两后端 argv 断言); `.` 前缀/`attrib +H` 文件不被排除; 系统元数据模式命中与尺寸扣除; 顶层标记(Pattern)与 CLI 丢弃; `include_hidden=true` 全链路无默认元数据排除
- **e2e(实机)**: zip 与 7z 格式 roundtrip, 预埋 Thumbs.db/.DS_Store/用户 `.env`, 验证系统元数据不进包而 `.env` 进包; `--include-hidden-files` 后元数据也进包
- **UI node test**: 元数据/模式命中行默认不勾选、模式输入 → scan/compress 契约、勾选被标行可压缩、规则不一致报过期
- 全 workspace 回归

## 8. 文档同步

| 文档 | 修改 |
|---|---|
| compress.md | §6.2 命令表加排除开关列; 新增"排除规则"小节(语法 + 归一化 + 平台差异 + 不变式) |
| README | 支持列表 + CLI 示例加 `--exclude`/`--include-hidden-files` + 隐藏口径说明 |
| ui-backend.md | §4 scan/compress DTO 加 excludes; §8 表格加排除模式输入与不勾选行为 |
| 本文档 | 目标规格 |
