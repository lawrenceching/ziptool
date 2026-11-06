# 重复文件查找技术方案

> 状态: **已实现**(2026-11-07)——`crates/core` 查重算法(`dedup.rs` + 单测)、壳层 4 个命令(`dedup_scan` / `dedup_compare` / `dedup_verify` / `reveal_dedup_file`)与桌面 UI(查重模式 + 结果表 + 全量校验)均已落地; CLI 本期不变
> 范围: `crates/core`(查重算法)、`crates/tauri-app`(壳层命令 + 桌面 UI); CLI 本期不变
> 相关: [ui-backend.md](ui-backend.md)(命令面 / DTO / 状态机)、[filtering_rules.md](filtering_rules.md)(排除策略)、[progress.md](progress.md)(进度协议)、[i18n.md](i18n.md)(词条规则)

## 1. 需求与目标

### 1.1 原始需求

在大数据盘搬运前, 找出源目录中的重复文件:

1. 找出可疑文件:
   - 同文件名;
   - 同后缀名且文件大小相同。
2. 对可疑文件比较前 1 MB 数据是否相同。
3. 第二步仍相同的:
   - 小于 500 MB 的文件: 计算全量 hash, 比较是否相同;
   - 大于等于 500 MB 的文件: 比较前 10 MB 数据是否相同。
4. 第三步中大于等于 500 MB 且前 10 MB 仍相同的文件, 在 UI 中列出, 经用户确认后再全量计算 hash。

第四步需要确认的原因: 源文件夹可能包含大量大文件, 自动全量计算 hash 会导致大量磁盘读操作。

### 1.2 目标

- 桌面 UI 新增第三个工作模式「查重」, 与「压缩 / 解压」并列
- 查重只读: **仅查看与定位重复文件, 不删除、不移动、不压缩**
- 候选扫描(元数据)在选源后自动开始; 内容比对必须由用户手动触发, 避免误读大盘
- ≥500 MB 大文件的全量校验必须由用户勾选并确认后执行
- 全程可取消; 进度、告警、过期校验沿用现有外壳模式
- 排除策略沿用设置页的「文件排除」, 与压缩扫描口径一致
- 新增文案进入 i18n 五语言词条

### 1.3 非目标

- 不删除 / 移动 / 隔离重复文件; 「定位」只调用系统文件管理器
- 不做跨源目录比对(只在一个源文件夹内部查重)
- 不做查重结果持久化 / 增量重扫 / 历史记录
- 不把 1 MB / 500 MB / 10 MB 阈值做成 UI 设置项(v1 固定, 代码内 `DedupOptions` 预留)
- 不做按组流式刷新表格(v1 只在阶段结束时整表重绘, 运行中靠状态栏进度)
- 不做并行哈希(v1 单线程顺序读, 对机械盘友好)
- 不做硬链接识别 / 推荐保留哪一份 / 修改时间展示
- 查重模式不提供单次自定义排除模式(只用设置页策略)
- CLI 不新增查重命令
- 不做查重结果的「打开所在文件夹」以外的处置动作

## 2. 已确认的设计决策

| # | 决策 | 结论 |
|---|---|---|
| D1 | 入口位置 | 顶栏第三模式「查重」, 复用源路径、表格、进度、取消等现有骨架 |
| D2 | 候选分组规则 | 「同完整文件名 + 同大小」与「同扩展名 + 同大小」**取并集** |
| D3 | 查重后的动作 | 仅查看 / 定位; 不删除 |
| D4 | 排除策略 | 沿用设置页现有「文件排除」策略 |
| D5 | 阶段启动方式 | 元数据扫描自动; 1 MB 前缀比对与哈希比对手动触发 |
| D6 | 大文件确认交互 | 待确认组显示在结果表中, 组标题行可勾选; 表格上方横幅显示预计读盘量并提供「全量校验选中组」 |
| D7 | 哈希算法 | BLAKE3(256 位) |
| D8 | 架构 | core 分阶段无状态 API + 壳层 `DedupContext` 会话缓存 + UI 四个命令 |
| D9 | 并发 | 单线程顺序读 |
| D10 | 链接 / 空文件 | 不跟随符号链接与目录联接; 0 字节文件跳过 |

## 3. 用户流程

```
选择源文件夹
  │
  ├─ 自动: 阶段 1 扫描元数据(两次目录遍历, 只 stat)
  │     ├─ 无候选 ──▶ 空态「未发现可疑文件」
  │     └─ 有候选 ──▶ 候选表 + 「开始查重」按钮
  │
  └─ 用户点击「开始查重」
        │
        ├─ 阶段 2: 每个候选文件读前 1 MB, BLAKE3 前缀哈希分桶
        ├─ 阶段 3:
        │     ├─ ≤1 MB 文件: 前缀即全文件 → 完全相同
        │     ├─ <500 MB 文件: 全量 BLAKE3 → 完全相同 / 淘汰
        │     └─ ≥500 MB 文件: 前 10 MB BLAKE3 → 待确认 / 淘汰
        │
        └─ 结果表
              ├─ 完全相同组(绿色)
              └─ 前 10 MB 相同组(琥珀, 可勾选)
                    │ 用户勾选后点横幅按钮
                    ▼
              阶段 4: 全量 BLAKE3(仅选中组)
                    └─ 组替换为「完全相同」或移除
```

取消语义:

- 阶段 1 取消 = 本次查重失败, 返回「查重已取消」
- 阶段 2/3 取消 = 已完成组保留结论, 当前组退回候选, 未处理组仍为候选; 用户可再次点「开始查重」继续
- 阶段 4 取消 = 已完成组替换为确认组, 当前组与未处理组仍为待确认

## 4. UI 设计

### 4.1 入口与布局

顶栏模式切换变为三档: `压缩 | 解压 | 查重`。

查重模式下:

- 路径区只保留「源文件夹」行; 「目标文件夹」行隐藏
- 压缩/解压选项行隐藏, 替换为查重说明行
- 表格复用现有表格容器与单元格样式
- 底部状态栏复用现有进度与告警区

```
┌──────────────────────────────────────────────────────────────────────┐
│ [压缩] [解压] [查重]                                    [取消/确认]   │
├──────────────────────────────────────────────────────────────────────┤
│ 源文件夹  [ D:\data                                              ]    │
├──────────────────────────────────────────────────────────────────────┤
│ 比对策略: 前 1 MB → 小文件全量 BLAKE3 / ≥500 MB 先比前 10 MB          │
│ 找到 12 组可疑文件 · 首轮预计读取 3.4 GB              [开始查重]      │
├──────────────────────────────────────────────────────────────────────┤
│ ⚠ 2 组大文件前 10 MB 相同, 全量校验预计读取 8.2 GB                   │
│                                          [全量校验选中组 (2)]        │
├──┬────────────────────────────────────────────┬─────────┬───────────┬────────┤
│☑ │ 文件名                                     │ 大小    │ 状态      │ 操作   │
├──┼────────────────────────────────────────────┼─────────┼───────────┼────────┤
│  │ 3 个文件 · 可释放 2.1 GB                   │         │ 完全相同  │        │
│  │   photos/2023/a.jpg                        │ 1.1 GB  │           │ [定位] │
│  │   backup/2023/a.jpg                        │ 1.1 GB  │           │ [定位] │
│  │   copy/a.jpg                               │ 1.1 GB  │           │ [定位] │
│☑ │ 2 个文件 · 可释放 4.0 GB                   │         │ 前10MB相同│        │
│  │   video/a.mp4                              │ 2.0 GB  │ 待校验    │ [定位] │
│  │   backup/video/a.mp4                       │ 2.0 GB  │ 待校验    │ [定位] │
└──┴────────────────────────────────────────────┴─────────┴───────────┴────────┘
```

### 4.2 表格

- 分组行沿用「智能打包方案」视图的组标题行样式, 成员行缩进
- 组标题: `{{N}} 个文件 · 可释放 {{bytes}}` + 状态徽章
  - 候选组与待确认组的「可释放」都是潜在值(尚未全量校验), 由状态徽章明确标注
- 成员行:
  - 文件名列显示**相对源目录的路径**(`/` 分隔); 悬停 `title` 显示完整路径
  - 大小列显示该文件大小
  - 状态列: 待确认组成员显示「待校验」; 已确认组成员留空(组标题已表达)
  - 操作列: 每行一个「定位」图标按钮, 调用系统文件管理器并选中该文件
- 勾选框列:
  - 只有「待全量校验」的组标题行显示勾选框
  - 表头全选框在查重模式下等价于「全选所有待确认组」
  - 已确认组无勾选框(本期无处置动作)

### 4.3 状态徽章

| 状态 | 徽章文案 | 配色 | 出现时机 |
|---|---|---|---|
| candidate | 候选 | 灰 | 阶段 1 完成, 等待「开始查重」 |
| confirmed | 完全相同 | 绿 | 全量 BLAKE3 一致 |
| largePrefixMatched | 前 10 MB 相同 | 琥珀 | ≥500 MB 且前 10 MB 一致, 尚未全量校验 |
| needVerify | 待校验 | 琥珀(成员行) | 待确认组成员 |

内容比对失败的候选组**不在结果表展示**; 状态栏给「已排除 N 组候选」。

### 4.4 按钮与忙碌状态

| 场景 | 顶栏确认钮 | 查重行按钮 | 大文件横幅 |
|---|---|---|---|
| 元数据扫描中 | spinner「扫描中…」, 禁用 | 禁用 | 隐藏 |
| 候选就绪 | 禁用 | 「开始查重」可用 | 隐藏 |
| 阶段 2/3 运行 | 红色「取消」 | 禁用 | 隐藏 |
| 阶段 2/3 取消且有剩余候选 | 禁用 | 「开始查重」恢复可用 | 有待确认组时显示 |
| 有结果、无剩余候选 | 禁用 | 隐藏 | 有待确认组时显示 |
| 有结果、仍有剩余候选 | 禁用 | 「开始查重」可用 | 有待确认组时显示 |
| 阶段 4 运行 | 红色「取消」 | 隐藏 | 按钮 loading / 禁用 |
| 无候选 / 无重复 | 禁用 | 禁用 | 隐藏 |

锁定规则:

- 阶段 1: 只锁顶栏确认钮(与现有扫描一致); 源路径、模式、设置菜单保持可用; 换源会以新扫描抢占并取消旧扫描, 切模式作废旧结果(后端扫描自然收尾或由后续扫描抢占)
- 阶段 2/3/4: 全部输入禁用(沿用现有 `running` 规则); 顶栏确认钮变「取消」

### 4.5 查重说明行

显示固定文案与实时摘要:

- 固定文案: `比对策略: 前 1 MB → 小文件全量 BLAKE3 / ≥500 MB 先比前 10 MB`
- 阶段 1 完成: `找到 {{groups}} 组可疑文件 · 首轮预计读取 {{bytes}}`
- 无候选: `未发现可疑文件`
- 阶段 2/3 完成后: 该行隐藏或只保留说明; 摘要移到状态栏

### 4.6 大文件横幅

- 仅当存在 `largePrefixMatched` 组时显示
- 文案: `{{groups}} 组大文件前 10 MB 相同, 全量校验预计读取 {{bytes}}({{files}} 个文件)`
- 预计读取量 = 选中组内所有文件的完整大小之和
- 按钮: `全量校验选中组 ({{count}})`, 未选中任何组时禁用
- 横幅采用现有方案提醒条的琥珀色样式

### 4.7 状态栏文案

| 场景 | 文案 key(示意) | 文案 |
|---|---|---|
| 阶段 1 | `dedup.scanning` | `正在扫描文件 {{count}} 个…` |
| 候选就绪 | `dedup.foundCandidates` | `找到 {{groups}} 组可疑文件 · 首轮预计读取 {{bytes}}` |
| 阶段 2/3 | `dedup.comparing` | `正在比对 {{done}}/{{total}} 组 · 已读 {{bytes}}` |
| 阶段 4 | `dedup.verifying` | `正在全量校验 {{done}}/{{total}} 组 · 已读 {{bytes}}` |
| 完成 | `dedup.summary` | `找到 {{groups}} 组重复文件 · {{copies}} 个副本 · 可释放 {{bytes}}` |
| 有待确认 | `dedup.summaryPending` | `; 另有 {{pending}} 组待确认` |
| 无重复 | `dedup.noDuplicates` | `没有发现重复文件(已排除 {{rejected}} 组候选)` |
| 取消 | `dedup.cancelled` | `已取消: 已完成 {{done}}/{{total}} 组` |
| 过期 | 壳层错误 | `查重结果已过期, 请重新选择源文件夹` |

「可释放」口径 = `(组内文件数 - 1) × 文件大小`, 仅统计已确认组; 待确认组不计入。「副本数」口径同样是 `Σ(组内文件数 - 1)`, 仅统计已确认组。

### 4.8 空态与错误

- 未选源: 现有空态提示(文案改为通用「点击选择源文件夹」)
- 阶段 1 无候选: 状态栏 `未发现可疑文件`, 表格空
- 阶段 2/3 后无重复: 状态栏 `没有发现重复文件(已排除 N 组候选)`, 表格空
- 命令错误: 状态栏显示错误文本, 超长截断并进 `title`
- 告警: 状态栏汇总 `N 条警告`, 明细不逐条进表格(查重表格是结果视图, 不混入告警行)

### 4.9 交互细节

- 切换模式或重选源: 清空查重表格、会话与勾选; 源路径按现有规则清空
- 设置页修改排除策略后关闭设置页: 若当前是查重模式且已有源, 自动重跑阶段 1(与压缩模式一致)
- 阶段 1 运行中换源/切模式: 新扫描胜出, 旧结果按 seq 丢弃
- 阶段 2/3/4 运行中不可换源/切模式; 只能取消
- 定位失败(文件已删除等): 状态栏显示可读错误, 不影响结果表
- 浏览器预览(无 `__TAURI__`): 用 mock 数据走完候选 → 比对 → 待确认 → 全量校验流程

### 4.10 无障碍与 i18n

- 表格 caption: `重复文件组`
- 待确认组勾选框 aria-label: `选择此组进行全量校验`
- 定位按钮 aria-label / title: `在资源管理器中显示`
- 全部新增文案进入 `ui/locales/{zh-CN,zh-HK,zh-TW,en,ru}.js`
- 动态文案一律走 `t(key, params)`, 不用 HTML 字符串拼接
- 行内容用 `createElement` / `textContent` 渲染, 保持现有无 innerHTML 的 XSS 约束

## 5. 总体架构

```
ui/index.html + ui/app.js
  │ dedup_scan / dedup_compare / dedup_verify / reveal_dedup_file
  ▼
crates/tauri-app/src/lib.rs — #[tauri::command] 薄壳
  │ 参数校验 + 忙闲互斥 + CancelFlag / Progress Channel + DedupContext
  ▼
crates/core/src/dedup.rs — 纯库
  │ 元数据扫描 / 候选分组 / BLAKE3 比对 / 全量校验
  ▼
文件系统(只读)
```

职责边界:

| 层 | 负责 | 不负责 |
|---|---|---|
| UI | 模式切换、表格渲染、勾选、按钮状态、状态栏文案 | 文件系统访问、分组与哈希规则 |
| 壳层 | DTO 转换、`sessionId` 保鲜、忙闲互斥、进度/取消桥接、定位路径校验 | 分组与比对算法 |
| core | 扫描、分组、哈希、进度回调、取消检查 | UI 概念、会话缓存 |

**core 无状态, 会话状态放壳层**: `DedupContext { session_id, src, rules, groups }` 缓存在 `AppState`。每次阶段操作校验 `src + session_id`; `session_id` 的作用等同现有打包方案的 `planId`, 防止旧 UI 操作新会话。

## 6. core 设计(`crates/core/src/dedup.rs`)

### 6.1 新增依赖

- 工作区 `Cargo.toml` 与 `crates/core/Cargo.toml` 增加 `blake3 = "1"`。
- 不引入 `rayon` / 异步运行时; 哈希在调用线程内顺序执行。

### 6.2 类型

```rust
/// 查重阈值; Default = 1 MiB / 500 MiB / 10 MiB。
/// 阈值不暴露到 UI, 但可配置以便单测用小值覆盖大文件分支。
pub struct DedupOptions {
    pub prefix_bytes: u64,          // 默认 1 MiB
    pub large_threshold: u64,       // 默认 500 MiB
    pub large_prefix_bytes: u64,    // 默认 10 MiB
}

impl Default for DedupOptions { /* 上述默认值; 单位均为二进制(MiB) */ }

pub enum DedupGroupState {
    Candidate,          // 阶段 1 产出, 等待内容比对
    Confirmed,          // 全量 BLAKE3 一致
    LargePrefixMatched, // ≥ large_threshold 且前 large_prefix_bytes 一致, 待全量校验
}

pub struct DedupFile {
    pub rel_path: String, // 相对源根, '/' 分隔; 不含源根前缀
    pub size: u64,
}

pub struct DedupGroup {
    pub id: u64,          // 输出时按排序重新分配 1..n
    pub state: DedupGroupState,
    pub size: u64,        // 组内每个文件的大小(候选分组保证同大小)
    pub files: Vec<DedupFile>,
    pub wasted_bytes: u64, // (files.len() - 1) * size
}

pub struct DedupScanReport {
    pub groups: Vec<DedupGroup>,        // 全部 state = Candidate
    pub warnings: Vec<String>,
    pub files_scanned: u64,
    pub estimated_first_pass_bytes: u64, // Σ min(size, prefix_bytes)
}

pub struct DedupCompareReport {
    pub groups: Vec<DedupGroup>, // Confirmed / LargePrefixMatched / 未处理 Candidate
    pub warnings: Vec<String>,
    pub cancelled: bool,
    pub rejected_groups: usize,  // 已处理完且未产出任何结果组的候选组数
}

pub struct DedupVerifyReport {
    pub groups: Vec<DedupGroup>,
    pub warnings: Vec<String>,
    pub cancelled: bool,
}
```

UI 文案沿用用户口径的「MB」, 实际阈值按二进制 MiB 计算(与现有 `parse_size` / 打包阈值一致)。

### 6.3 对外 API

```rust
pub fn scan_candidates(
    input: &Path,
    rules: &ExcludeRules,
    opts: &DedupOptions,
    cancel: &CancelFlag,
    on_progress: &dyn Fn(&Progress),
) -> Result<DedupScanReport>;

pub fn compare_candidates(
    input: &Path,
    groups: Vec<DedupGroup>,
    opts: &DedupOptions,
    cancel: &CancelFlag,
    on_progress: &dyn Fn(&Progress),
) -> Result<DedupCompareReport>;

pub fn verify_large_groups(
    input: &Path,
    groups: Vec<DedupGroup>,
    selected_ids: &[u64],
    cancel: &CancelFlag,
    on_progress: &dyn Fn(&Progress),
) -> Result<DedupVerifyReport>;
```

- `compare_candidates` 只处理 `Candidate` 组, 其它状态原样保留; 一个候选组可产出 0..n 个结果组
- `verify_large_groups` 只处理选中的 `LargePrefixMatched` 组; 一个待确认组可产出 0..n 个 `Confirmed` 组
- 两个函数返回完整组列表并重新编号, UI 整表重绘, 不需要增量合并
- 输出排序: 状态优先级 `Confirmed < LargePrefixMatched < Candidate`, 同状态按 `wasted_bytes` 降序、首路径字典序升序; 排序后 `id = 1..n`

### 6.4 阶段 1: 元数据扫描与候选分组

#### 遍历

- 使用显式栈迭代, 避免深递归
- `fs::symlink_metadata`; 目录入栈, 普通文件收集
- 不跟随目录符号链接; Windows 上跳过带 `FILE_ATTRIBUTE_REPARSE_POINT` 的目录, 避免 junction 循环
- 跳过文件符号链接(避免把链接与其目标算成重复)
- 跳过 0 字节文件(不占空间, 且会产生海量无意义分组)
- 排除规则:
  - 与 `scan.rs::dir_size` 同一坐标系: 相对源根的路径, 如 `App/cache`、`Novel/sub/*.log`
  - `matches_patterns(patterns, rel_key)` 命中文件跳过; 命中目录整树不下钻
- 读目录 / stat 失败: 记 warning, 跳过该子树, 不中断

#### 两遍扫描(控制内存)

1. 第一遍: 只把普通文件的 `size` 推入 `Vec<u64>`(8 字节/文件)
2. `sort_unstable()` 后找出出现次数 ≥2 的 size, 得到候选 size 集合
3. 第二遍: 再遍历一次, 只收集 size 在候选集合中的文件 `(rel_path, size)`

内存口径:

- 第一遍峰值 ≈ 8 字节 × 文件数(100 万文件约 8 MB)
- 第二遍只保留候选文件路径, 与候选数量成正比
- 不使用 `HashMap<size, count>`(每项开销更大)

#### 分组(并集规则)

1. 按 `size` 分桶
2. 每个 size 桶内用并查集:
   - 所有 `file_name.to_lowercase()` 相同的文件 union(规则 A: 同文件名 + 同大小)
   - 所有 `extension.to_lowercase()` 相同的文件 union(规则 B: 同扩展名 + 同大小)
   - 扩展名取 `Path::extension()`; 无扩展名视为空串; `.tar.gz` 的扩展名是 `gz`
3. 取连通分量 ≥2 的作为候选组
4. 组内文件按 `rel_path` 字典序排序
5. `wasted_bytes = (files.len() - 1) * size`
6. 组按 `wasted_bytes` 降序、首路径字典序升序排序
7. 分配 `id = 1..n`

说明:

- 文件名/扩展名大小写不敏感只是多产生候选, 最终由内容哈希裁决, 不会误报
- 并查集允许传递连接: A 与 B 同名, B 与 C 同扩展名, 则 A/B/C 同组; 三者大小必然相同

#### 进度与取消

- 新增 `Phase::DedupScanning`
- `done` = 已访问文件数; `total = 0`(总数未知)
- 每个目录边界必查取消旗标; 目录内每 512 个条目查一次(复用 `ScanCtl` 思路)
- 进度按时间节流(约 100 ms 一次), 避免百万文件刷爆 IPC
- 取消返回 `CoreError::Cancelled`, 不返回部分候选

### 6.5 阶段 2/3: 前缀比对与哈希

对每个候选组(文件大小 S):

```
P = min(S, opts.prefix_bytes)            // 默认 1 MiB
每个文件读前 P 字节 → BLAKE3 前缀哈希
  └─ 按哈希分桶, 只保留 ≥2 个文件的分桶
       ├─ S ≤ P        → 前缀已覆盖全文件 → Confirmed
       ├─ S <  large_threshold           → 全量 BLAKE3 → Confirmed 分桶
       └─ S ≥ large_threshold           → 前 Q = min(S, large_prefix_bytes)
                                            BLAKE3 → LargePrefixMatched 分桶
```

- 内容比对直接比较 BLAKE3 256 位哈希, 不做逐字节二次比对
- 阶段 2 已读过的 1 MB 在阶段 3 会重读一次; 只有通过前缀比对的少量文件进入阶段 3, 额外开销可接受, 换来实现简单
- 一个候选组可能分裂成多个结果组, 也可能全部淘汰
- 分桶后只剩 1 个文件的分支直接丢弃
- 读取失败(文件消失、无权限):
  - 记 warning, 该文件移出组
  - 组不足 2 个文件则整组丢弃
- 文件在扫描后大小变化:
  - 前缀比对按扫描时的 S 决定读取上限, 读到 EOF 提前结束
  - 阶段 3 全量哈希读到 EOF; 实际内容不同则自然淘汰, 不产生误报
- 取消:
  - 当前组退回 `Candidate`
  - 已完成组保留结论
  - 未处理组保持 `Candidate`
  - 返回 `cancelled = true` 与部分结果
- 组处理是原子的: 组内取消会丢弃该组的中间结果, 不产生半个组
- 按传入 `groups` 的顺序处理(候选已按可释放空间降序), 大收益组先出结果

进度:

- 新增 `Phase::DedupComparing`
- `done/total` = 已处理组数 / 候选组数
- `done_bytes` = 累计读取字节; `total_bytes = 0`
- `entry` = 当前文件相对路径(状态栏截断显示)
- 组内每读约 16 MB 或每 100 ms 发一次进度(取先到者)

### 6.6 阶段 4: 大文件全量校验

- 仅处理用户选中的 `LargePrefixMatched` 组
- 每个文件流式全量 BLAKE3(读到 EOF)
- 按哈希分桶; ≥2 个文件的分桶转为 `Confirmed`; 其余丢弃
- 一个待确认组可能分裂成多个确认组, 也可能全部淘汰
- 读取失败 / 文件消失: 记 warning, 移出组; 组不足 2 个文件则整组丢弃
- 取消:
  - 当前组保持 `LargePrefixMatched`
  - 已完成组替换为结果
  - 未处理组保持 `LargePrefixMatched`
  - 返回 `cancelled = true` 与部分结果
- 组处理是原子的: 组内取消会丢弃该组的中间结果
- 按缓存 `groups` 顺序处理选中项, 保证结果确定性

进度:

- 新增 `Phase::DedupVerifying`
- `done/total` = 已处理选中组数 / 选中组数
- `done_bytes` = 累计读取字节
- `entry` = 当前文件相对路径

### 6.7 哈希读取器

```rust
/// 流式读取并计算 BLAKE3; limit = None 表示读到 EOF。
/// 每读取一个 chunk 检查一次取消旗标; 读取字节数经回调上报。
fn hash_reader(
    path: &Path,
    limit: Option<u64>,
    cancel: &CancelFlag,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<blake3::Hash>;
```

- 固定 1 MiB 读缓冲, 避免每个文件重复分配
- `on_bytes` 用于累计进度; 实际发送进度由外层节流
- 取消返回 `CoreError::Cancelled`
- IO 错误由调用方转为 warning 并移出组, 不终止整批

### 6.8 进度 Phase 扩展

`crates/core/src/progress.rs`:

```rust
pub enum Phase {
    Scanning,
    Compressing,
    Extracting,
    DedupScanning,   // 阶段 1: 元数据扫描
    DedupComparing,  // 阶段 2/3: 前缀与哈希比对
    DedupVerifying,  // 阶段 4: 大文件全量校验
}
```

壳层 `ProgressEvent` 映射:

- `DedupScanning` → `"dedupScanning"`
- `DedupComparing` → `"dedupComparing"`
- `DedupVerifying` → `"dedupVerifying"`

## 7. Tauri 壳层

### 7.1 状态

```rust
struct AppState {
    current: Mutex<Option<CancelFlag>>,
    busy: Mutex<Option<&'static str>>,
    scan: Mutex<Option<ScanContext>>,
    scan_generation: AtomicU64,
    dedup: Mutex<Option<DedupContext>>,
    dedup_generation: AtomicU64,
}

struct DedupContext {
    session_id: u64,
    src: PathBuf,
    rules: ExcludeRules,
    groups: Vec<DedupGroup>,
}
```

### 7.2 命令面

| 命令 | 签名(前端视角) | 说明 |
|---|---|---|
| `dedup_scan` | `(src, filters?, on_progress) → DedupScanResult` | 选源后自动调用; 可被新的 dedup_scan / scan 抢占取消 |
| `dedup_compare` | `(src, sessionId, filters?, on_progress) → DedupCompareResult` | 用户点「开始查重」; 严格忙闲互斥 |
| `dedup_verify` | `(src, sessionId, groupIds, on_progress) → DedupVerifyResult` | 用户勾选后全量校验; 严格互斥 |
| `reveal_dedup_file` | `(src, sessionId, groupId, fileIndex) → Result<(), String>` | 校验后调用系统文件管理器定位 |

#### dedup_scan

- `filters` 经 `effective_rules` 展开并校验; 缺省 = 默认策略
- 忙闲类型 `"dedup-scan"`; 扩展 `acquire_scan_slot` 的可抢占判定: `Some("scan") | Some("dedup-scan")` 都可被新扫描取消
- `spawn_blocking` 调 core `scan_candidates`
- 成功后:
  - `dedup_generation += 1`, 得到新 `session_id`
  - 覆盖 `DedupContext`
  - 返回 `DedupScanResult`

#### dedup_compare

- 校验 `src`、`session_id` 与缓存一致; 不一致返回 `查重结果已过期, 请重新选择源文件夹`
- `filters` 展开后与 `DedupContext.rules` 比对; 不一致按过期拒绝
- 忙闲类型 `"dedup-compare"`; 严格互斥(不抢占)
- 取缓存 groups 调 core `compare_candidates`
- 用返回的完整组列表覆盖缓存(保留原 `session_id`)
- 返回 `DedupCompareResult`

#### dedup_verify

- 校验 `src`、`session_id`
- `groupIds` 必须:
  - 非空
  - 全部存在于缓存
  - 全部为 `LargePrefixMatched`
  - 无重复 ID
- 任一不满足 → 拒绝开工(未知 ID / 状态不符都按过期处理)
- 忙闲类型 `"dedup-verify"`; 严格互斥
- 调 core `verify_large_groups`; 用返回列表覆盖缓存
- 返回 `DedupVerifyResult`

#### reveal_dedup_file

- 校验 `src`、`session_id`
- 按 `groupId` 找到组, 按 `fileIndex` 找到文件
- 用 `src.join(rel_path)` 拼出绝对路径
- 不信任前端传绝对路径; 路径必须来自缓存组
- 调 `desktop::reveal_path`
- 不加忙闲锁(只读、短操作)

### 7.3 DTO

全部 `#[serde(rename_all = "camelCase")]`:

```rust
struct DedupFileDto { rel_path: String, size: u64 }

struct DedupGroupDto {
    id: u64,
    state: String, // "candidate" | "confirmed" | "largePrefixMatched"
    size: u64,
    files: Vec<DedupFileDto>,
    wasted_bytes: u64,
}

struct DedupScanResult {
    session_id: u64,
    groups: Vec<DedupGroupDto>,
    files_scanned: u64,
    estimated_first_pass_bytes: u64,
    warnings: Vec<String>,
}

struct DedupCompareResult {
    session_id: u64,
    groups: Vec<DedupGroupDto>,
    cancelled: bool,
    rejected_groups: usize,
    warnings: Vec<String>,
}

struct DedupVerifyResult {
    session_id: u64,
    groups: Vec<DedupGroupDto>,
    cancelled: bool,
    warnings: Vec<String>,
}
```

`session_id` 为 `u64`, JS 在 2^53 内可安全表示; 每次查重会话递增。

### 7.4 忙闲与取消

- `dedup_scan` 与现有 `scan` 共用可抢占扫描语义
- `dedup_compare` / `dedup_verify` 进入时若已有任务 → `Err("已有任务在运行")`
- 所有命令进入时设置 `current = Some(CancelFlag)`; 结束(成功/失败/取消)调 `finish_job` 清空
- `cancel_current` 任何时候可调; 无任务时 no-op
- 关闭窗口沿用现有 `ExitRequested` 取消并等待收尾逻辑

### 7.5 定位实现(`desktop.rs` 新增)

- Windows: `explorer /select,<path>`
- macOS: `open -R <path>`
- Linux: `xdg-open <父目录>`(无统一「选中文件」语义)
- 只启动不等待; 子进程后台回收, 与 `open_external_url` 一致
- Windows 调用 `explorer` 时不加 `CREATE_NO_WINDOW`(GUI 程序)

## 8. UI 实现

### 8.1 状态

```js
state.mode = "compress" | "extract" | "dedup"

state.dedup = {
  sessionId: null,
  groups: [],            // DedupGroupDto 列表
  seq: 0,                // 丢弃过期阶段 1 结果
  stage: "idle",         // idle | scanning | candidates | comparing | results | verifying
  selectedGroupIds: new Set(), // 待确认组勾选
  rejectedGroups: 0,
};
```

### 8.2 模式切换与选源

- `setMode("dedup")`:
  - 显示查重说明行; 隐藏目标目录行与压缩/解压选项
  - 按现有作废规则清空源路径、表格与查重会话
- 点击/拖拽选择源:
  - 压缩模式 → `doScan(path)`
  - 查重模式 → `doDedupScan(path)`
- `doDedupScan`:
  - `seq = ++state.dedup.seq`
  - 清空表格与会话勾选
  - 调 `dedup_scan(src, filters, on_progress)`
  - 结果按 `seq` 丢弃过期; 成功后渲染候选表
- 阶段 1 期间允许换源/切模式; 换源会由新扫描抢占并取消旧扫描, 切模式按现有 `seq` 机制作废旧结果

### 8.3 命令调用

- `dedup_scan` / `dedup_compare` / `dedup_verify` 都传 progress Channel(复用 `makeChannel`)
- `dedup_compare` / `dedup_verify` 期间:
  - `setLocked(true)`(源、模式、设置菜单禁用)
  - 顶栏确认钮变「取消」, 点击调 `cancel_current`
  - 结束后 `setLocked(false)`
- 阶段 1 期间:
  - 只锁顶栏确认钮(spinner「扫描中…」)
  - 其余控件可用

### 8.4 表格渲染

新增 `renderDedupRows()`:

- 按 `state.dedup.groups` 顺序渲染
- 组标题行:
  - `{{N}} 个文件 · 可释放 {{bytes}}`
  - 状态徽章
  - 待确认组显示勾选框, `checked` 来自 `selectedGroupIds`
- 成员行:
  - 缩进
  - 相对路径(转义/`textContent`)
  - 大小
  - 待确认组显示「待校验」
  - 定位按钮
- 表头全选框:
  - 查重模式 → 全选/取消所有待确认组
  - 无待确认组时禁用
- 每次整表重绘后, 从 `selectedGroupIds` 移除已不存在或已不是 `largePrefixMatched` 的 ID
- 切换语言时重渲染表格与状态栏

### 8.5 按钮状态

新增 `syncDedupActions()`:

- 「开始查重」:
  - 仅当存在 `candidate` 组时显示
  - 可用条件 = 无任务运行
- 横幅显示条件:
  - 存在 `largePrefixMatched` 组
  - 不在阶段 2/3 运行中
- 「全量校验选中组」可用条件:
  - `selectedGroupIds.size > 0`
  - 无任务运行
  - 所有选中 ID 仍是 `largePrefixMatched`
- 顶栏确认钮:
  - 阶段 1: spinner(只锁自己)
  - 阶段 2/3/4: 「取消」
  - 其余查重状态: 禁用

### 8.6 进度与状态栏

`onProgress(ev)` 按 `ev.phase` 分流:

- `dedupScanning` → `正在扫描文件 {{done}} 个…`
- `dedupComparing` → `正在比对 {{done}}/{{total}} 组 · 已读 {{doneBytes}}`
- `dedupVerifying` → `正在全量校验 {{done}}/{{total}} 组 · 已读 {{doneBytes}}`
- 压缩/解压分支保持现状

完成后由命令返回结果统一设置最终状态文案。

### 8.7 设置联动

- 设置页关闭时:
  - 压缩模式: 现有逻辑重扫
  - 查重模式且策略有变化: 重跑 `dedup_scan`
  - 查重模式策略未变: 不重扫
- 策略变化导致会话作废: 清空旧结果与勾选

### 8.8 浏览器预览 mock

- `mockDedupScan`: 生成 2~3 组候选, 带小/大文件
- `mockDedupCompare`: 用定时器推进度, 把部分候选变为 `confirmed` / `largePrefixMatched`
- `mockDedupVerify`: 推进度并把选中组变为 `confirmed`
- mock 数据路径使用假相对路径; 定位按钮在 mock 下只写状态栏提示

## 9. 错误与告警

- 命令 `Err(String)`:
  - 状态栏显示, 超长截断, 全文进 `title`
  - 错误文案保持 core / 壳层现有可读风格
- 阶段 1 失败: 清空表格进空态, 源路径保留, 状态栏错误
- 阶段 2/3/4 失败:
  - 保留已有结果, 状态栏错误
  - 缓存 groups 与 UI 可能不一致时, 以最后一次成功返回为准
- 读文件失败 / 文件消失:
  - core 记 warning 并移出组, 不中断整批
  - 状态栏汇总 `N 条警告`
- 过期:
  - `sessionId` / `src` / `filters` 不匹配 → 拒绝开工, 状态栏提示重新选择源文件夹
- 取消:
  - 阶段 1 → 错误文案「查重已取消」
  - 阶段 2/3/4 → 返回部分结果 + `cancelled = true`, 状态栏「已取消: 已完成 X/Y 组」

**长任务实现约定**:

- `dedup_compare` / `dedup_verify` 进入时先克隆缓存 groups 并**释放互斥锁**, 在 `spawn_blocking` 中跑 core
- 完成后重新加锁, 再次校验 `src + session_id` 再写回; 防止长任务期间状态被替换
- 忙闲锁保证运行期间不会有新的 `dedup_scan` 写缓存; 校验是兜底

## 10. 边界与已知取舍

| 项 | 决定 | 理由 |
|---|---|---|
| 0 字节文件 | 跳过 | 不占空间, 会产生海量无意义分组 |
| 符号链接 / junction | 不跟随; 文件链接不参与 | 避免循环、避免链接与目标算成重复 |
| 硬链接 | 作为不同路径参与, 内容相同会被列出 | 本期不删除, 无数据风险; 识别硬链接留作后续 |
| 文件名大小写 | 分组时统一小写 | 多产生候选不影响正确性(内容裁决) |
| 扩展名 | `Path::extension()` 小写; 无扩展名为空串 | `.tar.gz` 归为 `gz`; 无扩展名仍可命中规则 B |
| 同大小但不同名/扩展名 | 不是候选 | 遵循 D2 的并集规则 |
| 扫描后文件变化 | 以最终全量哈希为准; 消失/读失败移出组 | 与现有扫描保鲜策略一致 |
| 内容比对失败的候选 | 不显示, 只给排除计数 | 避免结果表被非重复项淹没 |
| 阈值 | v1 固定 1 MB / 500 MB / 10 MB | 用户需求明确; 代码层 `DedupOptions` 预留 |
| 并发 | 单线程顺序读 | 机械盘顺序读更稳; NVMe 并行留作优化 |
| 结果表规模 | 与现有扫描表一致, 不做虚拟滚动 | 现有表格已支持数万行方案视图 |
| 大文件确认 | 必须用户勾选 + 点按钮 | 避免自动读大量数据 |
| 删除 / 移动 | 不做 | 本期只读, 避免数据风险 |

## 11. 性能与内存

- 阶段 1:
  - 两次目录遍历, 只做 `read_dir + symlink_metadata`
  - 第一遍 size 向量: 8 字节/文件; 100 万文件约 8 MB
  - 第二遍只保留候选路径
  - 排除命中的目录整树不下钻
- 阶段 2:
  - 每个候选文件读 1 MB
  - 10 万候选约读 100 GB; UI 在开始前显示「首轮预计读取」
- 阶段 3:
  - 小文件: 仅前缀通过者全量读
  - 大文件: 仅读前 10 MB
- 阶段 4:
  - 仅用户勾选的大文件全量读; 横幅显示预计读取量
- 进度:
  - 时间节流约 10 次/秒, 避免 IPC 淹没前端
- 哈希:
  - BLAKE3 单线程即可获得较高吞吐
  - 固定 1 MB 读缓冲, 不随文件数分配

## 12. 安全

- 查重全程只读: 不删除、不移动、不写源目录
- `reveal_dedup_file` 只接受 `(groupId, fileIndex)`, 由壳层从缓存解析绝对路径; 不信任前端路径
- 系统文件管理器调用使用 `Command` 参数, 不经过 shell 拼接
- 渲染继续使用 `createElement` / `textContent`, 不引入 innerHTML
- CSP 不变; 不引入远程资源
- BLAKE3 256 位, 碰撞概率可忽略, 不附加逐字节二次比对

## 13. 测试策略

### 13.1 core 单测(无外部工具)

- 候选分组:
  - 同名 + 同大小 → 候选
  - 同扩展名 + 同大小 → 候选
  - 同名但不同大小 → 不候选
  - 同大小但不同名且不同扩展名 → 不候选
  - 无扩展名 + 同大小 → 候选
  - 大小写不同的文件名/扩展名 → 候选
  - 0 字节文件 → 跳过
  - 单独文件 → 不候选
  - 排除规则命中文件 / 目录 → 跳过
  - 符号链接 / 目录联接 → 不参与
  - 传递连接(A 同名 B, B 同扩展名 C)→ 同组
- 前缀比对:
  - 前 1 MB 相同、尾部不同 → 淘汰
  - ≤1 MB 且内容相同 → 阶段 2 直接确认
  - ≤1 MB 且内容不同 → 淘汰
- 大文件:
  - ≥阈值且前 10 MB 相同、尾部不同 → `LargePrefixMatched`
  - 阶段 4 全量相同 → `Confirmed`
  - 阶段 4 全量不同 → 淘汰
  - 一个候选组分裂成多个确认组
- 取消:
  - 阶段 1 取消 → `Cancelled`
  - 阶段 2/3 取消 → 部分结果, 当前组回候选
  - 阶段 4 取消 → 部分结果, 未处理组保持待确认
- 读失败 / 文件消失:
  - 记 warning, 文件移出组
  - 组不足 2 个文件 → 整组移除
- 确定性:
  - 组排序、组内排序、ID、`wasted_bytes`
- 测试用小阈值 `DedupOptions`(如 64 B / 1 KB / 256 B)覆盖大文件分支, 不造真实 500 MB 文件

### 13.2 壳层单测

- DTO 映射(state 字符串 / camelCase)
- `sessionId` 过期、`src` 不一致、`filters` 不一致 → 拒绝
- `dedup_verify` 的 groupIds 校验: 空 / 未知 / 状态不符 / 重复 → 拒绝
- 忙闲互斥; `dedup_scan` 对旧 scan 的抢占
- `reveal_dedup_file` 路径校验: 未知组/索引 → 拒绝
- 取消后的 `finish_job` 清理

### 13.3 UI 单测(node --test + jsdom)

- 模式切换: 三档按钮、目标目录行/选项行显隐、源路径清空
- 契约测试: 命令名、参数形状、progress Channel、sessionId 透传
- 阶段 1 结果渲染: 组标题、成员路径、大小、候选徽章
- 「开始查重」启用条件、运行中取消按钮、结束恢复
- 结果渲染: 确认组绿色、待确认组琥珀、成员「待校验」
- 勾选与横幅: 全选、单选、预计读取量、按钮启用条件
- 阶段 4 调用参数: `groupIds` 与勾选一致
- 取消与部分结果渲染
- 状态栏文案与语言切换重渲染
- mock 三阶段流程
- i18n 五语言词条完整性(现有测试扩展)

### 13.4 实机验证

- 真实目录(含 <500 MB 与 ≥500 MB 文件)走完:
  - 选源 → 候选 → 开始查重 → 结果 → 勾选 → 全量校验 → 定位
- 取消: 阶段 1 / 阶段 2/3 / 阶段 4
- 换源与切模式: 旧结果不残留
- 设置页改排除策略后关闭: 查重自动重扫
- 无 Tauri 的浏览器预览: mock 流程

### 13.5 回归

- `cargo test --workspace` 全绿
- `cd crates/tauri-app && npm test` 全绿
- 现有扫描 / 压缩 / 解压 / 设置 / 菜单测试零改动或仅做扩展

## 14. 涉及文件

| 文件 | 动作 | 职责 |
|---|---|---|
| `Cargo.toml` | 修改 | 工作区增加 `blake3` |
| `crates/core/Cargo.toml` | 修改 | 引用 `blake3` |
| `crates/core/src/dedup.rs` | 新增 | 扫描、分组、前缀/全量哈希、校验 |
| `crates/core/src/progress.rs` | 修改 | 新增三个 Phase |
| `crates/core/src/lib.rs` | 修改 | 模块导出、所需错误变体 |
| `crates/tauri-app/src/lib.rs` | 修改 | `DedupContext`、4 个命令、DTO、忙闲/抢占 |
| `crates/tauri-app/src/desktop.rs` | 修改 | `reveal_path` |
| `crates/tauri-app/ui/index.html` | 修改 | 第三模式、查重说明行、横幅、操作列 |
| `crates/tauri-app/ui/app.js` | 修改 | 查重状态机、渲染、命令调用、mock |
| `crates/tauri-app/ui/locales/*.js` | 修改 | 五语言词条 |
| `crates/tauri-app/test/ui.test.mjs` | 修改 | 契约与界面行为用例 |
| `crates/tauri-app/test/i18n.test.mjs` | 修改 | 词条完整性覆盖 |
| `crates/tauri-app/ui/app.css` | 按需重新生成 | 新增 Tailwind 类名时执行 `npm run css` |
| `README.md` | 修改 | 功能说明与使用方式 |
| `_docs/ui-backend.md` | 修改 | 命令面与 DTO 增补 |
| `_docs/dedup.md` | 新增 | 本设计文档 |

## 15. 后续可做(不在本期)

1. 删除 / 移动到回收站 / 隔离区
2. 按组流式刷新表格, 长任务更早看到确认结果
3. 并行哈希(可配置线程数, NVMe 友好)
4. 阈值进入设置页
5. 结果持久化与历史会话
6. 硬链接识别与「保留建议」(如保留最早修改的副本)
7. CLI `ziptool dedup` 子命令
8. 跨多个源目录互查
