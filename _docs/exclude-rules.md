# 排除规则(隐藏文件 + 自定义排除模式)技术文档

> 定位: 目标实现规格. 在 _docs/compress.md 基础上引入**排除维度**: 扫描统计与后端命令两层同步生效.
>
> **核心决策(用户拍板)**:
> - 底层(core/CLI)通过**自定义排除模式**实现, 支持多个模式叠加生效
> - 隐藏文件 = 上层内建的**默认排除集**, 建立在同一套模式机制上
> - 隐藏判定**仅用各 OS 原生口径**, 不以 `.` 开头一刀切(避免误伤用户自命名文件): Windows = 文件系统隐藏属性, Linux/mac = `.` 前缀(Unix 原生约定)
> - CLI 默认排除隐藏文件, `--include-hidden-files` 关闭
> - UI 扫描后**显示**隐藏文件但**默认不勾选**; UI 高级区提供排除模式输入
> - UI 默认 `include_hidden = false`(设置页 `os-hidden` 策略默认开); 关闭该策略后为 `true`, 即包含隐藏项

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
/// 排除规则: 内建隐藏默认集 + 用户自定义模式, 叠加(并集)生效
pub struct ExcludeRules {
    /// false(默认) = 启用内建隐藏排除; true = 不排除任何隐藏项
    pub include_hidden: bool,
    /// 统一语法用户模式, 多条叠加; 无否定语义
    pub patterns: Vec<String>,
}

pub struct Options {
    // ...split_size / password / random_name / format / level 不变...
    pub exclude: ExcludeRules,   // Default = 排除隐藏、无用户模式
}

/// 顶层条目的排除标记(仅顶层; 内部文件不逐个上报)
pub enum ExcludeReason {
    /// 内建隐藏判定命中
    Hidden,
    /// 用户模式命中(保留命中模式, 供 UI 展示)
    Pattern(String),
}
```

- `Entry` 增加 `exclude: Option<ExcludeReason>` 与 `attr_hidden: Vec<String>`(§4 属性路径); `ScanReport` 增加 `excluded: Vec<ExcludedEntry>`(name + reason, CLI 提示用)
- 分层职责:
  - **顶层**: `scan_dir` 永远返回全部条目并打标(隐藏判定 / 模式匹配); CLI 默认丢弃被标条目(`include_hidden=true` 时保留 Hidden 标记条目); UI 渲染为不勾选(既有勾选名单机制消化)
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

- Windows 属性隐藏路径(非模式可表达)逐条: `zip` → `-x "<rel>"`; `7z` → `-x!<rel>`(目录一条即整树)
- **已知语义缝隙**: zip 通配无目录段边界, `*/sub/*.log` 可能过度匹配更深层同名路径 —— 接受并文档标注(zip = 最大兼容模式, 排除精度次要); 7z 后端精确

## 4. 隐藏文件双平台机制

| 平台 | 判定 | 机制 |
|---|---|---|
| Linux/mac | 名字以 `.` 开头(OS 原生约定) | 归一为一条统一模式(任意深度 basename 以 `.` 开头)走 §3 管线, 零额外机制 |
| Windows | 文件系统隐藏属性(`Permissions::hidden()`, std 稳定 API; `.` 开头**不算**) | 属性无法通配表达 → `dir_size` 遍历时顺带收集属性隐藏条目的**相对路径**(隐藏目录不下钻、整树一条), 后端逐条开关 |

- **预算护栏**(Windows 枚举): 排除开关总量超上限(24000 字符或 1000 条)即停止追加, 把未排除项**加回尺寸统计**(保持不变式), 并 warning「隐藏项过多, N 项未排除」
- `include_hidden = true`: 两平台均不排除任何隐藏项(顶层被标条目不丢弃、内部不剔除、后端无开关)

## 5. CLI

- `compress` 新增:
  - `--exclude <模式>`, 可重复, 与内建隐藏集叠加
  - `--include-hidden-files`, 关闭内建隐藏排除
- 扫描后被标条目打印提示(如 `已排除(隐藏): $RECYCLE.BIN`), 默认不压缩
- `extract` 无新参数

## 6. UI(Tauri)

- `scan` 命令新增可选参数 `filters?: { enabledPolicies }`(设置页策略)与 `excludes?: string[]`(高级区输入, 多行/逗号分隔); 壳层经 `filter::effective_rules` 展开, ScanContext 连带缓存展开后的规则
- `ScanItem` 新增 `excluded?: string`(原因文案:「隐藏文件」/「匹配排除模式: <p>」); 前端被标行**显示但默认不勾选**, detail 列标原因
- `compress` DTO 同步加 `filters` / `excludes`; 与扫描缓存规则不一致 = 报"扫描已过期"(沿用 5.4 保鲜语义)
- UI 的 `include_hidden` 由 `os-hidden` 策略决定(默认 false); 浏览器 mock 路径同步

## 7. 测试计划

- **core 单测**: 统一语法解析(非法模式报错)与归一化矩阵(两后端 argv 断言); `dir_size` 排除扣除与隐藏目录不下钻; 顶层标记(Hidden/Pattern)与 CLI 丢弃; Windows 属性枚举 + 预算护栏回退加回统计; `include_hidden=true` 全链路无排除
- **e2e(实机)**: zip 与 7z 格式 roundtrip, 预埋 Thumbs.db/.DS_Store/临时文件(Windows 用 `attrib +H` 置属性), 验证包内不存在; `--include-hidden-files` 后存在
- **UI node test**: 隐藏/模式命中行默认不勾选、模式输入 → scan/compress 契约、勾选隐藏行可压缩、规则不一致报过期
- 全 workspace 回归

## 8. 文档同步

| 文档 | 修改 |
|---|---|
| compress.md | §6.2 命令表加排除开关列; 新增"排除规则"小节(语法 + 归一化 + 平台差异 + 不变式) |
| README | 支持列表 + CLI 示例加 `--exclude`/`--include-hidden-files` + 隐藏口径说明 |
| ui-backend.md | §4 scan/compress DTO 加 excludes; §8 表格加排除模式输入与不勾选行为 |
| 本文档 | 目标规格 |
