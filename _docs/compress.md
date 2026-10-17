# 压缩实现技术文档

> 定位: 目标实现规格. 按"扫描 → 生成目标名 → 压缩"三步展开, 可直接指导 `crates/core` compress 模块的重构.
> 与当前实现的差异在各节末尾以"**与现状差异**"标注, 汇总清单见第 10 节.
> UI(Tauri)对接本文的命令面 / 事件协议 / 勾选筛选见 _docs/ui-backend.md; 对 core 的影响仅是把第 2 节三步拆为可组合的 pub 阶段(plan + compress_entries), 语义不变.

## 1. 目标与非目标

### 目标

- 输入一个源目录, 把其中**每个顶层条目**(文件或目录)各压缩为一个独立 zip 包, 输出到另一目录
- 支持分包(用后端原生能力, 保持默认命名), 支持密码, 支持随机包名
- 全程进度可上报(CLI 与 Tauri 共用回调), 可取消(条目间生效)
- 数据搬运场景的原则: **宁停不错** —— 命名冲突 / 目标已存在 / 空间可疑 / 半成品, 一律显式报错, 不静默覆盖

### 非目标

- 不自带压缩实现, 只封装外部命令 (Linux: `zip`, Windows: `7z`), 输出统一为 zip 格式
- 不做断点续压 / 并行压缩 / 压缩级别透传 (见第 11 节)
- 不提供文件名加密 (zip 格式不加密文件名; 用随机包名 + mapping.csv 缓解, 见 5.3)
- 字节级压缩进度 (后端行为不统一, 见 7.2)

## 2. 总体流程

```
input 目录
    │
    ▼
Step 1 扫描        read_dir 顶层条目(按名字排序), 逐条目递归统计字节大小
    │              产出: Vec<Entry> + 汇总 total_bytes + warnings
    ▼
Step 2 生成 dst    决定每个条目的压缩包基础名 dst, 做冲突/已存在/预检
    │              产出: 补全 dst 后的 Vec<Entry>
    ▼
Step 3 压缩        逐条目: 构造后端命令 → 执行 → 解析主卷 → 进度回调/取消
    │              失败即停, 半成品清理; 取消则剩余条目不再开始
    ▼
out 目录: <dst>.zip 或其原生分卷 + mapping.csv(随机命名时) + Report
```

三步串行执行. Step 1/2 是纯内存操作(不触碰后端), 可独立测试; Step 3 是唯一调用外部命令的步骤.

## 3. 数据结构

```rust
/// 一个待压缩的顶层条目(Step 1 产出, Step 2 补全 dst, Step 3 消费)
#[derive(Debug, Clone)]
pub struct Entry {
    /// 条目名(不含路径), 如 "Novel"、"C.jpg"
    pub name: String,
    /// 条目类型
    pub kind: EntryKind,
    /// 递归统计的字节大小: 文件为自身大小, 目录为其中所有普通文件大小之和
    pub size: u64,
    /// 压缩包基础名(不带扩展名), Step 2 填充; 产物为 {dst}.zip 或其分卷
    pub dst: String,
    /// 压缩包格式, 当前恒为 "zip"
    pub format: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
}
```

设计说明:

- `type` 改为 `kind: EntryKind` 枚举: `type` 是 Rust 关键字; 且枚举可穷尽匹配, 比 `String` 安全
- `size` 用 `u64` 字节, 不用人类可读单位 —— 单位换算交给展示层
- `dst` 是**基础名**(不带 `.zip`): 分卷时的产物数量后端才能确定, 提前拼完整文件名没有意义
- `format` 按大纲保留, 当前恒 `"zip"`, 为将来支持其他格式留扩展点, 代码中不参与分支判断

**与现状差异**: 现状 Step 3 里用 `Vec<PathBuf>` 即扫即压, 没有独立的条目结构与大小信息; 目标设计把条目结构化为 `Entry`, 贯穿三步.

## 4. Step 1: 扫描源目录并统计大小

### 4.1 行为

1. 校验 `input` 存在且为目录, 否则 `CoreError::InputNotFound`
2. `read_dir(input)` 收集顶层条目, **按条目名字典序排序**(保证进度与 mapping.csv 顺序稳定、可复现)
3. 逐条目取 `symlink_metadata` 判定 `kind`(不跟随符号链接判定类型)
4. 递归统计大小(见 4.2), 汇总 `total_bytes`
5. 条目数为 0 → `CoreError::NoEntries`

### 4.2 大小统计规则

| 对象 | 规则 |
|---|---|
| 普通文件 | `metadata.len()` |
| 目录 | 递归累加其中所有普通文件的大小; 空目录计 0 |
| 符号链接 | 只计链接自身大小, **不跟随**(避免循环与重复计数) |
| 硬链接 | 每个链接各按完整大小计入(简单, 可能高估; 搬运场景可接受) |
| 无权限子树 | 该子树计 0, 记入 `Report::warnings`, **不中断**(后续 Step 3 压缩该条目时后端会真实失败, 到时统一报错) |

统计值只用于进度权重与预检, **不**作为压缩输入, 因此允许有误差.

### 4.3 扫描的目的

1. **加权进度**: 按字节而非条目数计算完成度(数T盘上一个 20GB 目录和一张 2MB 图片不应各占一格)
2. **压缩前预检**: 汇总总量与输出盘状况比对(见 5.2), 在压出任何半成品之前失败
3. **dry-run 预览**(CLI 层): 扫描 + 生成 dst 后即可打印"名字 / 大小 / 预计分卷数"表格, 不执行压缩
4. 早发现空目录、0 字节条目等边界

### 4.4 性能与扫描进度

数百万文件的全盘扫描本身可能耗时数分钟, 因此扫描阶段也要上报进度(`Phase::Scanning`, 见 7.1).
自实现栈式递归遍历, 不引入 walkdir 等依赖(与 naming.rs 的零依赖取向一致).

**与现状差异**: 现状没有扫描步骤(不做大小统计, 无 warnings, 进度只有条目计数); 扫描是本次设计新增的阶段.

## 5. Step 2: 生成目标名 dst

### 5.1 命名规则

- `random_name = false`: `dst = 条目名`, 与原名一致
- `random_name = true`: `dst = "zt_" + 16位十六进制随机串`

随机名说明:

- 大纲中的"64位随机字符串"落实为 **64 bit 随机量**(16 个 hex 字符), 加 `zt_` 前缀便于人眼识别本工具产物、也避免与源目录名撞形
- 随机源: 时间纳秒 ^ 进程 id ^ 进程内计数器, 经 splitmix64 扩散(现状实现, 零依赖). 它**不是密码学随机**, 但这里只为防包名泄露信息, 不抗针对性攻击, 足够; 万级条目碰撞概率 ~10^-12, 且仍有存在性检查兜底(5.2)
- 生成的 `dst` 若与输出目录已有文件重名 → 重新生成(最多重试 16 次, 仍失败则 `OutputExists`)

### 5.2 冲突与预检(宁停不错)

检测时机: Step 2 内一次性完成, 在**任何压缩开始之前**.

| 检测 | 条件 | 结果 |
|---|---|---|
| `DuplicateDst` | 非随机模式下, 两个条目的 dst 在**大小写不敏感**比较下相同 | 报错, 列出冲突对, 提示改用 `--random-name` |
| `OutputExists` | 某 dst 的 `{dst}.zip` 或 `{dst}.zip.001` 已存在于输出目录 | 报错, 指明路径; 不覆盖(重复运行产生的"更新"会造出脏包, 比 failure 更危险) |
| `OutInsideInput` | 输出目录是输入目录自身或其子目录 | 报错(边压边把半成品扫进包里, 产物不可预期) |

大小写不敏感检测的理由: 源盘可能是大小写敏感的 ext4("A" 与 "a" 可共存), 但输出盘往往是 NTFS/FAT(不能共存); 在源头就报错, 优于压到一半失败.

### 5.3 mapping.csv

随机命名时, 输出目录写 `mapping.csv`(UTF-8, 表头 `archive,original`, 各一行记录 `{dst} 分卷组 → 原条目名`):

- **时机: Step 3 开始前写表头, 每完成一个条目立即追加一行并 flush** —— 中断/断电/搬盘时, 已完成部分的对应关系不丢失
- 安全注意: mapping.csv 含原始名, **防泄露的对象是压缩包文件名, 不是整个输出目录**. 随盘搬运时应把 mapping.csv 留在本地不随盘(或理解为其本身即敏感文件)

**与现状差异**: 现状是"全部条目完成后一次性写 mapping.csv", 中断即丢失全部对应关系; 且现状不检测 DuplicateDst / OutputExists / OutInsideInput(重复运行会得到被 zip "更新"过的脏包).

## 6. Step 3: 压缩

### 6.1 逐条目流程

```
for entry in entries:                        # 顺序执行, 不并行
    1. 构造命令(6.2): 后端由平台决定
    2. 执行: 非零退出 → 失败处理(6.4)
    3. 解析主卷(6.3): 记入 Report.entries
    4. mapping.csv 追加一行(随机命名时)
    5. 进度回调: 返回 Break → cancelled = true, 跳出循环
```

顺序执行的理由: 搬运场景输入输出多为机械盘/USB, 随机读写是吞吐杀手, 单后端进程已能吃满顺序 IO; 并行是负优化.

### 6.2 命令构造: 工作目录与包内路径一致性

**关键设计: 子进程的工作目录设为 `input`, 命令行里只用条目名(相对路径), 输出用绝对路径.**

原因: 两个后端对"包内路径"的处理不同 ——
`zip -r out.zip /data/A` 会把 `data/A/...` 存进包(去掉开头 `/` 但保留父目录链),
而 `7z a out.zip D:\data\A` 只存 `A/...`.
若按现状从进程原目录传全路径, 两平台的包内布局不一致, 解压结果结构不同.
统一改为 `current_dir(input)` + 条目名后, 两后端的包内布局都是 `A/...`, 解压即还原原始目录名.

| 平台 | 后端 | 压缩命令(在 input 内执行) |
|---|---|---|
| Linux | zip 3.0 | `zip -r [-s <size>] [-P <pw>] <out_abs>/<dst>.zip <name>` |
| Windows | 7z | `7z a -tzip [-v<size>] [-p<pw>] -- <out_abs>/<dst>.zip <name>` |

- `<out_abs>`: 输出目录经 `create_dir_all` + `canonicalize` 得到的绝对路径(切换工作目录后相对路径会失效, 必须绝对化)
- `--` 仅 7z 需要(防参数注入式文件名); zip 的 `-` 开头文件名通过路径前缀 `./` 化处理
- 不暴露压缩级别, 用后端默认

### 6.3 产物与主卷解析

| 后端 | 不分卷 | 分卷(设 -s/-v size) |
|---|---|---|
| zip | `B.zip` | `B.z01, B.z02, ..., B.zip`(末卷 `.zip`) |
| 7z | `B.zip` | `B.zip.001, B.zip.002, ...`(首卷 `.zip.001`) |

主卷(primary archive, 记入 Report 与 mapping.csv, 也是解压入口): 优先 `B.zip`(zip 末卷 / 不分卷), 否则 `B.zip.001`(7z 首卷). 解压侧规则见 extract 模块, 此处不展开.

### 6.4 失败处理与半成品清理

- 单条目后端命令非零退出 → 整批停止(后续条目不开始), 返回 `BackendFailed`(含程序名、退出状态、stderr 末 5 行)
- 停止前**清理本条目的半成品**: 精确匹配 `<dst>.zip`、`<dst>.z[0-9][0-9]`、`<dst>.zip.[0-9][0-9][0-9]` 并删除. 半成品分卷被误当完整包比少一个包更危险
- 已完成条目的产物与 mapping.csv 已写入的行**保留**(它们是完好的)
- 失败条目不写入 mapping.csv

### 6.5 密码传递与威胁模型

| 后端 | 传递方式 | 替代方案 |
|---|---|---|
| zip | argv `-P <pw>` | 无(交互式 `-e` 需 TTY, 不适合自动化; 无 stdin 通道) |
| 7z | argv `-p<pw>` | 无(同样无可靠的非交互 stdin 通道) |

argv 密码对本机其他进程(`ps` / 任务管理器)短暂可见, 这是两后端的能力边界, 接受之. 本工具的威胁模型是**搬运途中的介质丢失/经手**, 不是同机的恶意进程; 且 zip 格式密码本身是弱 ZipCrypto(文件内容流加密, 文件名明文), 防的是随手查看, 不抗定向攻击. 这些限制写入用户文档, 不在代码里过度设计.

### 6.6 分卷注意事项

- zip `-s`: 官方下限 64k(`SplitSizeTooSmall` 拒绝, 现状已实现); 分卷包不可原地更新(`OutputExists` 检测已规避); **`-s` + `-P` 的组合在文档上无限制但生态验证少, Linux 实机回归是发布前必测项**(见第 9 节)
- 7z `-v`: 无 64k 硬下限, 但建议 ≥64k 保持两平台行为一致(只提示, 不强制); 实测 NanaZip 对 zip 格式支持原生分卷
- 大小参数统一走 `parse_size`("2g"/"500m"/"64k"/字节数)与 `format_split_size`(能整除则用最大单位), 两后端共用
- zip 分卷损坏可用 `zip -F` 修复, 属于手工救援手段, 不进自动化流程

**与现状差异(第 6 节)**: 现状命令从进程原目录传全路径(两平台包内布局不一致, 是需要修的正确性问题); 不检测/清理半成品; 不做 OutputExists 预检.

## 7. 进度与取消

### 7.1 进度模型(核心回调不变, 载荷扩展)

```rust
pub enum Phase { Scanning, Compressing }

pub struct Progress {
    pub phase: Phase,
    pub done: usize,        // 条目计数: 扫描=已扫描, 压缩=已完成
    pub total: usize,       // 条目总数
    pub done_bytes: u64,    // 压缩阶段: 已完成条目的 size 累计(扫描阶段为已扫描字节)
    pub total_bytes: u64,   // Step 1 汇总
    pub entry: String,      // 当前条目名
}
```

- 展示层: 完成度优先 `done_bytes / total_bytes`(total_bytes 为 0 时退化为 done/total); CLI 单行刷新 `[3/17] Novel (2.1GB/26GB)`, Tauri 经 Channel 推送
- 粒度仍是**条目级**: 一个条目压缩期间进度不动. 字节级见 7.2

### 7.2 取消

- 取消语义保持: 回调返回 `ControlFlow::Break` 后**不再开始下一个条目**, 当前条目让其后端命令自然跑完(产物完好, 计入 Report), 已完成的保留
- `cancel_current` 的桥接(Tauri `CancelFlag` / CLI 信号)不变
- 后续改进: 立即取消 = 杀子进程; 此时产物按 6.4 半成品规则清理

**与现状差异**: `Progress` 增加 phase 与字节字段, 是 CLI 与 Tauri 两外壳的共同 breaking change, 随重构同步改.

## 8. 错误模型与退出码

`CoreError` 在现有基础上扩充(全部有明确 Display 文案):

| 变体 | 触发 | 阶段 |
|---|---|---|
| `InputNotFound(PathBuf)` | input 不存在或非目录 | Step 1 |
| `NoEntries(PathBuf)` | 顶层无条目 | Step 1 |
| `DuplicateDst { entries }` | dst 大小写不敏感冲突 | Step 2 |
| `OutputExists(PathBuf)` | 目标包已存在(含随机名重试耗尽) | Step 2 |
| `OutInsideInput(PathBuf)` | 输出目录在输入目录内 | Step 2 |
| `SplitSizeTooSmall(u64)` | zip 后端分包 < 64k | Step 2/3 |
| `BackendFailed { .. }` | 后端命令非零退出 | Step 3 |
| `Io(std::io::Error)` | 文件系统 IO | 全程 |

CLI 退出码: `0` 成功(含 cancelled 完整返回), `1` 一般错误(CoreError), `2` 用法错误(clap 默认), `130` 用户取消.

## 9. 测试策略

沿用 TDD, 按三步组织:

- **单测(无外部命令)**
  - 扫描: 临时目录树的大小统计、排序、symlink 不跟随、权限子树 → warnings、空目录 → NoEntries
  - 命名: 随机名格式/唯一性、DuplicateDst(大小写)、OutputExists、OutInsideInput
  - 命令构造: 两后端 argv 表(含 current_dir 断言)、parse_size / format_split_size、64k 下限
- **编排测试(注入 fake 后端, 现有 `compress_with_backend` 注入口)**: 取消在条目间停止、失败停止且半成品被清理、mapping.csv 逐行追加且失败条目缺席
- **集成测试(真实后端)**: Windows 本机 NanaZip: 小目录 round-trip、分卷+密码 round-trip、错误密码被拒、半成品场景
- **Linux 实机回归清单(发布前人工, 本机 zip.exe 损坏无法自动化)**: `zip -s` 分卷 round-trip、`-s` + `-P` 组合 round-trip(已知生态风险项)、unzip 6.0 解我们的分卷

## 10. 与当前实现的差异汇总(重构清单)

| # | 项 | 现状 | 目标 |
|---|---|---|---|
| 1 | 条目模型 | `Vec<PathBuf>` 即扫即压 | `Entry{name,kind,size,dst,format}` 贯穿三步 |
| 2 | 扫描阶段 | 无 | 递归大小统计 + 排序 + warnings + total_bytes |
| 3 | 包内路径 | 全路径传入, 两平台布局不一致 | `current_dir(input)` + 相对条目名, 布局统一为 `A/...`(**正确性修复**) |
| 4 | 预检 | 仅 64k 下限 | + DuplicateDst / OutputExists / OutInsideInput |
| 5 | mapping.csv | 全部完成后一次性写 | 先写表头, 逐条目追加并 flush |
| 6 | 失败处理 | 报错返回, 半成品残留 | 停批 + 按前缀精确清理半成品 |
| 7 | 进度载荷 | done/total/entry | + phase / 字节加权 |
| 8 | 输出路径 | 相对可用 | canonicalize 绝对化(配合工作目录切换) |

## 11. 非目标与后续改进

按价值排序, 均不在本次规格内:

1. **取消杀子进程**: 取消只停"下一个条目"→ 立即中断当前条目(产物按半成品清理)
2. **manifest 校验**: 输出 `manifest.txt`(每包的大小 + SHA-256), 搬运后校验完整性 —— 与本工具场景高度契合, 建议优先
3. **--level 透传**: `-0`/store 模式对已是压缩内容的媒体盘可数倍提速
4. **空间预检**: 输出盘剩余空间 < total_bytes 时提前失败(需 fs4 之类的依赖获取剩余空间, 与零依赖取向冲突, 故列为后续)
5. **字节级进度**: 7z 可解析 `-bsp1` 输出, zip 无机器可读进度, 两后端不统一, 暂不做
6. **--dry-run**: 扫描+命名后打印预览表(核心已具备能力, 属 CLI 外壳功能)
