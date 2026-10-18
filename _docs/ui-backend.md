# UI 对接后端交互规格

> 定位: 目标实现规格. 约定 Tauri 前端(`crates/tauri-app/ui/index.html`)与后端(`src/lib.rs` 命令层 → `crates/core`)之间的命令面、数据结构与事件协议、UI 状态机, 以及为支撑它们对 core 的最小扩展. 指导 tauri-app 与 core 的对接改造.
> 与当前实现的差异在各节末尾以"**与现状差异**"标注, 汇总清单见第 12 节.
> 本文是 compress.md / extract.md 的**消费端**(编排层): 执行语义(预检 / 停批 / 半成品清理 / 进度模型)以那两份为准, 本文不重复.

## 1. 目标与非目标

### 目标

- UI 选源即扫描并渲染表格, **勾选决定处理范围**, 确认后执行并实时反馈行级状态(进行中 / 完成 / 失败), 运行中可取消
- 压缩选项(分包 / 密码 / 随机包名)与解压选项(密码 / 覆盖)在 UI 上完整可用
- 前后端边界清晰: UI 只做状态机与展示; 业务语义(预检 / 停批 / 清理)全部在 core; 壳层只做参数转换与桥接
- 大盘友好: 递归大小扫描**只做一次**, 确认执行不重扫(core 分阶段 API, 见 5.3)
- 浏览器预览模式(无 `__TAURI__`)保留假数据路径, 界面样式迭代不依赖实机

### 非目标

- 不做扫描取消(`scan_dir` 无取消点, 大目录扫描期间 UI 锁定等待, 见 13.1)
- 不做多任务并行(单窗口同时至多一个后台任务, 见 3.5)
- 不做扫描结果持久化 / 增量重扫
- 不做内容级选择性解压(解包内的部分文件; extract.md 12.5 仍是后续, UI 勾选是**组级**选择)
- 字节级压缩 / 解压进度(两后端不统一, 沿用两份规格的结论不做)

## 2. 总体架构

```
ui/index.html (纯静态, 无打包器)
   │  window.__TAURI__.core.invoke / Channel (CSP 已放行 ipc:, 见 tauri.conf.json)
   ▼
tauri-app src/lib.rs — #[tauri::command] 薄壳
   │  参数转换(parse_size / 空串归一) + spawn_blocking + CancelFlag / Channel 桥接
   │  持有: AppState { 任务忙闲与 CancelFlag, ScanContext(扫描结果保鲜, 见 5.4) }
   ▼
crates/core — 纯库(分阶段 API, 见第 6 节)
   ▼
外部命令 zip / 7z / unzip
```

职责边界:

| 层 | 负责 | 不负责 |
|---|---|---|
| UI | 状态机、行状态推导、选项控件、错误与告警展示 | 任何文件系统访问与业务规则判断 |
| 壳层 | DTO 转换、parse_size、忙闲互斥、扫描结果保鲜、按名单筛选 | 预检 / 停批 / 清理等业务规则 |
| core | 扫描 / 命名 / 预检 / 执行 / 进度 / 取消 | 任何 UI 概念 |

进度沿用现有回调模型(compress.md 第 7 节 / extract.md 第 8 节): core 回调 `Progress`, 壳层经 `Channel` 推 `ProgressEvent`, UI 消费. 该载荷**不需要改动**.

## 3. 命令面

收敛为 4 个命令(`invoke_handler` 注册面):

| 命令 | 签名(前端视角) | 作用 |
|---|---|---|
| `scan` | `(mode, path, on_progress) → ScanResult` | 只扫描不执行, 选源后渲染表格 |
| `compress` | `(src, dst, entries, options, on_progress) → CompressResult` | 批量压缩勾选项 |
| `extract_archive` | `(src, dst, entries, options, on_progress) → ExtractResult` | 批量解压勾选项 |
| `cancel_current` | `()` | 取消当前任务(立即 kill 在跑的后端进程并清理半成品, 停止后续) |

### 3.1 scan

- `mode`: `"compress" | "extract"`, 与 UI 顶栏模式一致; `path`: 源文件夹(系统对话框返回值)
- compress → `core::scan_dir`(递归统计大小, `Phase::Scanning` 进度经 Channel 推送, 状态栏显示 `i/n`)
- extract → `core::scan_groups`(顶层分组, 量小无需进度); 每组补大小(组内各卷大小之和), mapping.csv 作为元数据行返回
- 错误透传 core: 路径不存在 / 非目录 → `InputNotFound`; 空目录 → `NoEntries`; 无包 → `NoArchives`
- **为什么 scan 不做 Step 2 预检**(OutputExists 等): 预检依赖 random_name / dst 等确认时才定的输入, 且用户扫完还可能换目标目录; 预检留在执行入口(plan / B2), 错误在任何产物产生前报出, 同样满足宁停不错

### 3.2 compress / 3.3 extract_archive

- `entries: string[]`: 勾选名单, 取值必须是 scan 返回的**原样 `name`**(compress = 条目名, extract = 入口卷名), 严格精确匹配
- `options`(压缩): `{ splitSize: string | null, password: string | null, randomName: boolean }`; `options`(解压): `{ password: string | null, force: boolean }`
- 壳层转换: `splitSize` 经 `parse_size`("2g" / "500m" / 字节数), 失败 → Err 提示; `password` 空串按 `None`(不加密)
- 执行路径见 5.3/5.4(壳层用 ScanContext 中缓存的扫描结果, 按名单筛选后调 core 分阶段 API; **不重扫**)
- 返回值: 现有 `CompressResult` / `ExtractResult` 增加 `warnings: string[]` 透传(现状 DTO 丢弃了 Report.warnings)

### 3.4 cancel_current

语义升级(2026-10):置 CancelFlag 后 core 立即 kill 在跑的后端进程并清理该条目 / 组半成品(返回 `Err(Cancelled)` 转为 `report.cancelled`), 后续不再开始; 条目 / 组间取消仍经 ControlFlow::Break. 关闭窗口时外壳在 `RunEvent::ExitRequested` 置取消并等任务收尾(上限 5s), 孤儿进程不再残留. 补充: 任务结束(成功 / 失败 / 取消)后 `current` 清为 `None`(现状残留已用过的 flag).

### 3.5 忙闲互斥

- scan / compress / extract_archive 三者互斥: 进入时发现已有任务在跑 → `Err("已有任务在运行")`(现状无守卫, 可重入); 无论成败结束即释放
- `cancel_current` 不受互斥限制, 任何时候可调(无任务时为 no-op)
- UI 侧配合: 任务运行中锁定全部输入(7.2), 正常触达不到互斥错误; 它是后端不信任前端的兜底

**与现状差异(第 3 节)**: 现状无 scan 命令(扫描内嵌在 compress_dir 里, UI 无法"先看后做"); 命令直接重扫输入; 无忙闲守卫; warnings 未透传; `current` 用完不清.

## 4. 数据结构(DTO)

serde 约定: 全部 DTO `#[serde(rename_all = "camelCase")]`(ProgressEvent 已是, 其余对齐).

```rust
/// scan 返回的一个表格行
struct ScanItem {
    name: String,        // compress=条目名; extract=入口卷名 / mapping.csv / 告警文件名
    kind: String,        // "file" | "dir"(extract 的组是文件, 恒 "file")
    size: u64,           // compress=递归大小; extract=组内各卷大小之和
    status: String,      // "pending"(可勾选) | "meta"(元数据) | "warning"(告警, 不可勾选)
    original: Option<String>, // extract: mapping.csv 原名; compress: None
    detail: Option<String>,   // warning 原因 / meta 附注(如 "2 行"); 展示文案由 UI 决定
}
struct ScanResult { mode: String, items: Vec<ScanItem>, warnings: Vec<String>, total_bytes: u64 }
```

- **显示名规则(UI)**: 表格"文件名"列显示 `original ?? name`; 进度事件的 `entry` 与该规则一致(extract 用原名, 无 mapping 用入口卷名), 所以行匹配天然成立
- 告警分两条通道: **表格行**(有明确文件名且不会被处理的: extract 的孤儿卷 / 非包文件)与 **statusbar 汇总**(挂在子树 / 全局的: compress 的无权限子树, mapping 解析异常). 区分的理由: 前者在表格里不可见就会被遗忘, 后者没有干净的行归属且条目仍会执行(后端失败时自然报错)

事件(不变): `{ phase: "scanning"|"compressing"|"extracting", done, total, doneBytes, totalBytes, entry }`

结果(扩展现有): `CompressResult { cancelled, entries: [{archive, original}], warnings }`; `ExtractResult { cancelled, groups: [{entry, original, files}], warnings }`.

## 5. 交互流程与时序

### 5.1 时序

```
UI(index.html)            壳层(lib.rs)                 core
│ scan(mode, src) ─────────▶ 忙闲抢占; spawn_blocking    │
│   ◀ Scanning 事件 ════════ Channel ════════════════════│ scan_dir 逐条目回调
│   (状态栏: 扫描中 i/n)    结果存入 ScanContext         │
│ ◀── ScanResult ──────────                             │
│ 渲染表格(默认全选 pending 行)                          │
│ (勾选 / 填选项 / 选目标)                                │
│ compress(src,dst,名单,opt)▶ 忙闲抢占; 校验 ScanContext │
│   ◀ Compressing 事件 ═════ Channel ════════════════════│ plan → compress_entries
│   (行状态推导, 7.3)       CancelFlag 就绪             │ 预检→逐条目→停批
│ [取消] ─ cancel_current ─▶ 置 CancelFlag               │
│ ◀── CompressResult / Err ─释放忙闲; 清 current         │
│ 修正行状态 / 状态栏汇总                                 │
```

extract_archive 同构(plan 换成 extract_prepared 内部的 B2 全量预检).

### 5.2 前端伪代码约定

`scanFolder` / confirm 两个 TODO(业务) 占位替换为 invoke 调用; `!window.__TAURI__` 时保留现有假数据分支(浏览器预览). invoke 与 Channel 取自 `window.__TAURI__.core`(withGlobalTauri 全局).

### 5.3 双扫描的消除(分阶段执行)

`compress_dir` 内部会完整重扫(递归统计大小). 数百万文件的盘上一轮扫描可达分钟级, "scan 命令扫一遍 + 确认再扫一遍"不可接受. 因此 core 把流程拆为可组合的阶段(见第 6 节): scan 命令产出 `ScanReport` 缓存在**壳层**, 确认时对它做 plan(命名 + 预检)→ compress_entries(执行), 不再重扫.

正确性代价与对策:

| 风险 | 对策 |
|---|---|
| 扫描到确认之间条目被删除 | compress_entries 开始前对每个条目做**顶层存在性复核**(每条目一次 stat, 便宜); 缺失 → 新错误 `EntryMissing`, 整批不开始 |
| 大小过期(文件增长) | 只影响进度加权, compress.md 4.2 已声明统计值允许误差, 接受 |
| 扫描后新增的条目 | 不在本批(与"勾选名单"语义一致); 需要时重新选择源文件夹即可 |

extract 无此问题: scan_groups 只列顶层(便宜), 真正的清单(list)与预检在确认后的 B2 执行, 天然保鲜.

### 5.4 扫描结果保鲜(ScanContext)

- 壳层 `AppState` 增加 `scan: Mutex<Option<ScanContext>>`; `ScanContext { mode, src, data: ScanReport | ExtractScan }`, 由 scan 命令写入
- compress / extract_archive 进入时校验: `mode` 与命令一致且 `src` 与传入路径一致, 否则 `Err("扫描结果已过期, 请重新选择源文件夹")`
- UI 传来的名单在壳层与缓存结果**严格匹配**(未知名同样按过期拒绝)—— 宁停不错
- UI 的模式切换 / 重选源已在 7.4 作废前端状态; 壳层 ScanContext 在下次 scan 时整体覆盖

## 6. core 扩展

为支撑上述流程, core 做四处**语义不变的拆分与导出**(组合行为与现有完全一致, CLI 与 E2E 不受影响):

```rust
/// Step 2 独立化: 输出目录校验(create_dir_all + 绝对化) + 生成 dst + 全部预检
/// (OutInsideInput / SplitSizeTooSmall / DuplicateDst / OutputExists), 在任何压缩开始之前
pub fn plan(input: &Path, out_dir: &Path, entries: &mut Vec<Entry>, opts: &Options) -> Result<()>

/// Step 3 独立化: mapping.csv + 逐条目压缩 + 停批 / 半成品清理;
/// 开始前对每条目做顶层存在性复核(5.3), 缺失 → 新错误变体 EntryMissing { name }
pub fn compress_entries(input: &Path, out_dir: &Path, entries: &[Entry],
                        opts: &Options, on_progress: &dyn Fn(&Progress) -> ControlFlow<()>) -> Result<Report>

/// 便捷组合(CLI 沿用, 行为不变): compress_dir = scan_dir → plan → compress_entries
pub fn compress_dir(...)  // 签名不变

/// 批量解压扫描: pub(crate) → pub, tuple → 结构化返回; 组补 size(组内各卷大小之和)
pub struct ExtractScan {
    pub groups: Vec<ExtractGroup>,  // 已排序(mapping 行序优先, 7.3 语义不变)
    pub meta: Option<MetaFile>,     // mapping.csv(name/size/rows), 展示用元数据行
    pub issues: Vec<ScanIssue>,     // 孤儿卷 / 非包文件等(结构化, 壳层转为表格 warning 行)
}
pub struct ScanIssue { pub name: String, pub message: String }
pub fn scan_groups(input: &Path) -> Result<ExtractScan>   // 空组仍报 NoArchives

/// 批量解压执行: B2 全量预检 + B3 逐组解压(原 extract_groups 的 pub 形态, 固定 RealExtract)
pub fn extract_prepared(groups: &[ExtractGroup], dest: &Path, password: Option<&str>,
                        force: bool, on_progress: &dyn Fn(&Progress) -> ControlFlow<()>) -> Result<ExtractReport>

/// 便捷组合(CLI 沿用, 签名不变): extract = 单包 resolve / scan_groups → extract_prepared
pub fn extract(...)
```

连带调整:

- `ExtractGroup` 增加字段 `pub size: u64` 并随 `ResolvedArchive`(字段本已 pub)一起提升为 pub; 单包分支(extract 的文件输入)size 取入口卷自身大小
- `CoreError` 增加 `EntryMissing { name }`(Display: 条目在扫描后消失, 提示重新扫描)
- `MetaFile { name, size, rows }` 新结构(read_mapping 的行数顺带透出)
- 测试注入口不变: `compress_with_backend` / `extract_with_backend`(pub(crate))保持, plan / compress_entries / extract_prepared 的组合测试经它们注入假后端

**与现状差异(第 6 节)**: 现状 compress 三步串在一个函数里, 外部只能整体调用(导致 UI 双扫描或无法先览后做); scan_groups 私有且无大小、返回 tuple; 勾选筛选无处安放.

## 7. UI 状态机与规则

### 7.1 状态机

```
idle ──选源──▶ scanning ──ScanResult──▶ ready ──确认──▶ running ──Ok──▶ ready(行状态修正)
  ▲              │ Err                                        ├─Err──▶ ready(当前行=失败)
  └─────────────┴───────── 切换模式 / 重选源(作废) ◀──────────┴─取消──▶ ready(其余行回待处理)
```

### 7.2 锁定规则

- scanning: 模式按钮 / 路径框 / 选项 / 确认全部禁用(扫描不可取消, 见 13.1); 状态栏实时 `扫描中 i/n`
- running: 同上全部禁用, **确认按钮变形为「取消」**(红色), 点击 → `cancel_current`; 无新增控件
- ready: 确认可用条件 = 存在勾选的 pending 行 且 dst 非空(现有逻辑); 选项行随模式切换显示压缩 / 解压各自的控件

### 7.3 行状态推导协议

前提约束: **表格行序必须等于 scan 返回序(UI 不做排序)** —— core 保证执行序 = 扫描序(compress 字典序 / extract mapping 行序), 索引推导才成立.

```
运行中(最新事件 done):  idx < done → 完成 | idx == done → 进行中 | idx > done → 待处理
Ok 返回:                报告集合(entries/groups 的 original/entry)内的行 → 完成;
                        cancelled 时其余行回「待处理」, 状态栏 "已取消: 完成 x/y"
Err 返回:               idx == done 的行 → 失败; 状态栏 = 错误文本
meta / warning 行:      恒定不变
```

徽章配色建议: 完成=绿 / 失败=红 / 进行中=蓝(现有蓝系) / 待处理=现状蓝灰 / 告警=琥珀 / 元数据=现状灰. 状态栏文本超长截断, 全文进 `title` 悬停可见.

### 7.4 作废规则(沿用已实现行为)

切换模式或重选源 → 清空源路径 + 表格 + 壳层将在下次 scan 覆盖 ScanContext; dst 保留(两模式的 dst 语义同类: 输出 / 还原目录).

## 8. 选项控件

路径区下方加一行, 随模式切换内容(7.2 锁定规则同样适用):

| 模式 | 控件 | 默认值 | 说明 |
|---|---|---|---|
| 压缩 | 分包大小(text, placeholder "不分包") | 空 = 不分包 | 确认时 `parse_size`, 失败提示并聚焦 |
| 压缩 | 密码(password) | 空 = 不加密 | 空串按 None; 不持久化 |
| 压缩 | 随机包名(checkbox) | 关 | 与 CLI `--random-name` 默认关闭一致 |
| 解压 | 密码(password) | 空 = 不加密 | 整批一个密码, 与 compress 对称 |
| 解压 | 覆盖已有文件(checkbox) | 关 | 宁停不错, 与 CLI `--force` 默认一致 |

随机名开启时目标产物为 `zt_xxxxxxxxxxxxxxxx.zip` + mapping.csv, UI 在选项行以说明文字提示一句, 避免用户在输出目录找不到原名.

## 9. 错误与告警呈现

- 命令 `Err(String)`: 状态栏显示(截断), `title` 存全文; Display 文案 core 已保证可读(ToolMissing / ExtractConflict / BackendFailed 带 stderr 末 5 行等)
- 扫描失败: 表格清空进空态, 源路径保留(便于改选); 状态栏错误
- **重跑已完成的批会在预检处失败**(OutputExists / ExtractConflict), 属预期行为而非缺陷 —— 宁停不错; 用户换目标目录或清理后重试
- warnings 汇总: 状态栏一次性给出(如 `完成: 3 成功 · 2 条警告, 见表格灰色行`), 明细按 4 节双通道呈现

## 10. 安全边界

- CSP 现值(`default-src 'self'; connect-src ipc: http://ipc.localhost`)已覆盖 invoke / Channel, 无需调整
- 密码经 argv 传递的威胁模型不变(compress.md 6.5); UI 侧 password 输入框不落任何持久化存储
- 路径虽来自系统对话框, 后端仍**不信任前端**: core 自身的 InputNotFound / OutInsideInput / zip-slip / 冲突预检全部照常执行, 互斥与名单严格校验(3.5 / 5.4)是壳层兜底
- UI 渲染保持 createElement / textContent(无 innerHTML): 错误消息里的路径、后端 stderr 等不可信内容无 XSS 面

## 11. 测试策略

- **core 单测(无外部命令)**: plan 在筛选集上的各预检变体; compress_entries 存在性复核(删条目 → EntryMissing、整批不开始); scan_groups 的 ExtractScan(size 合计 / meta 行 / issues / 排序不变); extract_prepared 组筛选后 B2 语义; EntryMissing Display
- **编排测试(注入假后端)**: compress_dir 组合 = plan + compress_entries 的等价性(现有用例不改即应全绿, 是拆分正确性的回归网); extract 同理
- **壳层单测**: DTO 映射(ScanItem 三种 status / warnings 透传); ScanContext 失配与未知名单拒绝; 忙闲互斥; parse_size 错误路径
- **UI 单测(node --test + jsdom, `npm test`)**: 前后端契约 + 界面行为。关键设计——**表面真实、传输造假**: `window.__TAURI__` 挂真实 `@tauri-apps/api` / `@tauri-apps/plugin-dialog` 包对象(全局 bundle 就是这些模块), 只 mock 最底层 `__TAURI_INTERNALS__`; 因此 UI 引用不存在的全局 API 路径在测试期必然 TypeError(抓过实际案例: `__TAURI__.ipc.Channel`, Tauri 2 的 Channel 在 core 命名空间), 命令名/参数形状/`__CHANNEL__:<id>` 序列化/进度回放协议(`{index, message}` 对象)全按真实链路校验。UI 逻辑独立为 `ui/app.js`(index.html 外链引用)
- **UI 走查(chrome-devtools, mock 模式)**: 状态机全迁移、行状态推导(虚拟事件序列驱动)、取消变形按钮、选项行切换
- **实机验证(用户)**: `cargo run -p zip-tool-tauri` 走通 scan → 表格 → 确认 → 进度 → 取消 / 完成全链路(命令真到 core 与后端工具)
- **回归**: workspace 全绿; CLI 与 E2E 用例零改动(compress_dir / extract 签名不变)

## 12. 与当前实现的差异汇总(落地清单)

| # | 项 | 现状 | 目标 |
|---|---|---|---|
| 1 | scan 命令 | 无(扫描内嵌 compress_dir) | 独立 `scan(mode, path)`, 选源即渲染 |
| 2 | 双扫描 | 确认执行时全量重扫 | plan / compress_entries 分阶段, 扫描结果壳层保鲜(5.3/5.4) |
| 3 | scan_groups | pub(crate), tuple, 无大小 | pub + `ExtractScan{groups, meta, issues}` + 组大小 |
| 4 | 勾选筛选 | 全量处理 | 壳层按名单严格筛选(UI 勾选决定范围) |
| 5 | 选项控件 | 无 | 分包 / 密码 / 随机名 / 覆盖(第 8 节) |
| 6 | 忙闲守卫 | 无(可重入), flag 不清 | 单任务互斥 + 结束释放(3.5/3.4) |
| 7 | 行级状态 | 静态徽章 | 进度推导 进行中 / 完成 / 失败(7.3) |
| 8 | warnings | DTO 丢弃 | 透传 + 表格行 / 状态栏双通道(第 4/9 节) |
| 9 | 取消入口 | 无 UI | 运行中确认按钮变形为取消(7.2) |
| 10 | 存在性复核 | 无 | compress_entries 顶层复核 → `EntryMissing`(第 6 节) |

## 13. 非目标与后续改进

按价值排序, 均不在本次规格内:

1. **扫描可取消**: scan_dir 加取消点(栈式遍历每轮回查), 大目录扫描期间允许反悔
2. ~~**取消杀子进程 / 关窗收尾**~~: 已实现(2026-10)——取消立即 kill 后端进程, 关窗时 `ExitRequested` 置取消并等待收尾
3. **doctor 自检入 UI**: 启动时调 `doctor` 检查后端工具, 缺 7z 提前预警(核心已有 check_backend_tool, 只差暴露与展示)
4. **打开输出目录**: 完成后一键 reveal 产物(含 mapping.csv 提示)
5. **dry-run 模式**: scan + plan 后即可渲染"预计包名 / 分卷数"预览表 —— 第 6 节分阶段 API 的直接受益者(与 compress.md 11.6 合流)
6. **内容级选择性解压**: 解包内部分文件(extract.md 12.5, 组级选择已具备基础设施)
