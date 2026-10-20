# 压缩实现技术文档

> 定位: 目标实现规格. 按"扫描 → 生成目标名 → 压缩"三步展开, 可直接指导 `crates/core` compress 模块的重构.
> 与当前实现的差异在各节末尾以"**与现状差异**"标注, 汇总清单见第 10 节.
> UI(Tauri)对接本文的命令面 / 事件协议 / 勾选筛选见 _docs/ui-backend.md; 对 core 的影响仅是把第 2 节三步拆为可组合的 pub 阶段(plan + compress_entries), 语义不变.

## 1. 目标与非目标

### 目标

- 输入一个源目录, 把其中**每个顶层条目**(文件或目录)各压缩为一个独立 zip 包, 输出到另一目录
- 可选: 第 12 节的**打包策略**可把多个小顶层条目合并为一个包; `strategy` 只产出打包计划, 不执行压缩. 未启用时仍保持"每个顶层条目一个包"
- 支持分包(用后端原生能力, 保持默认命名), 支持密码, 支持随机包名
- 全程进度可上报(CLI 与 Tauri 共用回调), 可取消(条目间 Break;条目内杀后端进程)
- 每个条目压缩成功后做一次**结构校验**(方案 A, 见 6.8, 默认开启、可关闭): 只读包头/中央目录, 确认解压端能正常打开; 校验不过按半成品处理, 坏包不进入 Report/mapping
- 数据搬运场景的原则: **宁停不错** —— 命名冲突 / 目标已存在 / 空间可疑 / 半成品 / 校验不过, 一律显式报错, 不静默覆盖

### 非目标

- 不自带压缩实现, 只封装外部命令 (Linux: `zip`/`7z`, Windows: `7z`); 输出格式支持 zip 与 7z (LZMA2), 见 _docs/compress-modes.md
- 不做断点续压 / 并行压缩 (见第 11 节)
- 不提供文件名加密 (zip 格式不加密文件名; 用随机包名 + mapping.csv 缓解, 见 5.3)
- ~~字节级压缩进度~~: 7z 系已实现(2026-10, 见 7.1); zip 后端无机器可读进度, 维持条目级
- 不做**全量数据校验**(`7z t` / `unzip -t`)与 manifest 哈希(方案 B/C, 见第 11 节); 方案 A 只保证包结构可打开, 不保证数据区无位翻转

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
Step 3 压缩        逐条目: 构造后端命令 → 执行 → 解析主卷 → 结构校验 → 进度回调/取消
    │              失败/校验不过即停, 半成品清理; 取消则剩余条目不再开始
    ▼
out 目录: <dst>.zip 或其原生分卷 + mapping.csv(随机命名时) + Report
```

三步串行执行. Step 1/2 是纯内存操作(不触碰后端), 可独立测试; Step 3 是唯一调用外部命令的步骤(压缩与结构校验).

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
    /// 包内条目数(普通文件 + 目录; 目录自身也计 1; 空目录 = 1)
    pub entry_count: usize,
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
    3. 解析主卷(6.3)
    4. 结构校验(6.8, 默认开启): 非零退出 → 半成品清理 + VerifyFailed
    5. 记入 Report.entries
    6. mapping.csv 追加一行(随机命名时)
    7. 进度回调: 返回 Break → cancelled = true, 跳出循环
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
| Linux | zip 3.0 | `zip -r [-s <size>] [-P <pw>] [-<0-9>] <out_abs>/<dst>.zip <name>` |
| Windows | 7z | `7z a -tzip [-v<size>] [-p<pw>] [-mx=<0-9>] -- <out_abs>/<dst>.zip <name>` |
| 7z 格式 | 7z | `7z a -t7z -m0=LZMA2 [-v<size>] [-p<pw>] [-mx=<0-9>] -- <out_abs>/<dst>.7z <name>` |

- `<out_abs>`: 输出目录经 `create_dir_all` + `canonicalize` 得到的绝对路径(切换工作目录后相对路径会失效, 必须绝对化)
- `--` 仅 7z 需要(防参数注入式文件名); zip 的 `-` 开头文件名通过路径前缀 `./` 化处理
- 压缩级别 0-9 经 `Options.level` 透传 (zip: `-<n>`; 7z: `-mx=<n>`, 0 = 仅存储); 缺省 = 后端默认。格式维度见 _docs/compress-modes.md

### 6.3 产物与主卷解析

| 后端 | 不分卷 | 分卷(设 -s/-v size) |
|---|---|---|
| zip | `B.zip` | `B.z01, B.z02, ..., B.zip`(末卷 `.zip`) |
| 7z | `B.zip` | `B.zip.001, B.zip.002, ...`(首卷 `.zip.001`) |
| 7z 格式 | `B.7z` | `B.7z.001, B.7z.002, ...`(首卷 `.001`) |

主卷(primary archive, 记入 Report 与 mapping.csv, 也是解压入口): 优先 `B.zip`/`B.7z`(zip 末卷 / 不分卷), 否则分卷首卷 `B.zip.001`/`B.7z.001`. 解压侧规则见 extract 模块, 此处不展开.

### 6.4 失败处理与半成品清理

- 单条目后端命令非零退出 → 整批停止(后续条目不开始), 返回 `BackendFailed`(含程序名、退出状态、stderr 末 5 行)
- 结构校验(6.8)非零退出 → 同样整批停止并清理该条目产物, 但返回独立的 `VerifyFailed`(含入口卷、退出状态、诊断文本), 不与压缩命令失败混淆
- 停止前**清理本条目的半成品**: 精确匹配 `<dst>.zip`、`<dst>.z[0-9][0-9]`、`<dst>.zip.[0-9][0-9][0-9]` 并删除; 7z 格式对应 `<dst>.7z`、`<dst>.7z.[0-9][0-9][0-9]`. 半成品分卷被误当完整包比少一个包更危险
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

## 6.7 排除规则(隐藏文件 + 自定义排除模式)

规格见 _docs/exclude-rules.md; 要点:

- **双层生效**: 扫描统计层(`total_bytes`/条目 size 扣除)与后端命令层(排除开关)同步; 不变式 = 统计口径 ≡ 后端实压内容
- **隐藏口径**: Linux/mac = `.` 前缀(归一为模式 `.*`); Windows = 文件系统隐藏属性(遍历时收集相对路径逐条开关, 预算护栏 1000 条 / 24000 字符, 超限回退加回统计并发 warning); Windows 上 `.` 开头**不算**隐藏
- **统一语法**: 不含 `/` = 任意深度 basename; 含 `/` = 条目根锚定(`*`/`?` 不跨 `/`, `**` 只在结尾, `prefix/**` = prefix 自身或其下); 大小写敏感; 非法模式宁停不错
- **归一化**(按后端可执行文件):

| 后端 | 开关位置 | 形态 |
|---|---|---|
| `zip`(Linux zip 格式) | 条目名**之后** | `-x "*/p"` + `-x "*/p/*"`(双开关兜底目录下钻) |
| `7z`(Windows zip 格式 + 7z 格式) | `--` 之前 | basename → `-xr!p`; 锚定 → `-x!<条目名>/<路径>`; `prefix/**` → `-x!<条目名>/prefix` |

- **分层职责**: 顶层条目永远返回并打标(`Entry.exclude`), `compress_dir`(CLI)丢弃被标条目并在 Report 带出名单, 分阶段 API(UI)不丢弃——标记仅展示, 勾选名单决定内容
- zip 通配无目录段边界, 锚定模式可能过度匹配更深层同名路径 —— 接受(zip = 最大兼容模式); 7z 后端精确

## 6.8 压缩后结构校验(方案 A)

**定位**: 每条目压缩命令退出码为 0 后, 在记入 Report / 写 mapping / 上报完成之前, 用对应解压工具对**入口卷**做一次只读结构检查. 目标是尽早发现"看起来像成品、实际打不开"的包(典型: 截断、缺中央目录/EOCD、缺分卷), 避免它进入产物清单.

**命令与判定**

| 产物形态 | 校验工具 | 命令 |
|---|---|---|
| 整包 zip(Linux `zip` 后端, 无分卷) | `unzip` | `unzip -l <entry>` |
| zip -s 分卷(`B.z01 ... B.zip`) | `7z` | `7z l -ba -bso0 -bsp0 -- <entry>` |
| 7z 格式整包 / `.7z.001` 分卷 | `7z` | 同上 |
| Windows zip 格式整包 / `.zip.001` 分卷 | `7z` | 同上 |

- **判定只看退出码**: `0` = 通过; 其他(含 7z 的 `1` warning)一律失败. 不要把 stdout/stderr 文本当判定依据
- 校验工具必须与解压端路由一致(见 6.3 / extract 模块): 整包 zip → `unzip`; zip -s、7z 格式与所有 `.001` 分卷 → `7z`. 这样"压缩后能校验"等价于"解压端能打开"
- `<entry>` 用 6.3 的主卷(`B.zip` / `B.zip.001` / `B.7z.001`), 分卷由 7z 自动补齐兄弟卷
- 校验进程的 stdout 不落地(避免百万条目包的清单占内存/拖慢校验), stderr 捕获末若干行用于诊断; `7z` 另用 `-bso0 -bsp0` 从源头静默
- 列表命令只读包头/中央目录, 属于**结构校验**: 不读数据区, 不验证 CRC. 数据区损坏/位翻转不在方案 A 范围(方案 B/C, 见第 11 节)

**为什么用 list 而不是 test**: `7z t` / `unzip -t` 会全量读并解压, 成本约等于再解压一遍; 对大容量搬运不可接受. `7z l` / `unzip -l` 只扫描头部, 成本与条目数相关, 通常毫秒到秒级; 对截断包(无 EOCD)已足够——7z 会以 exit 2 结束.

**失败处理**

- 判定失败 → 调用 6.4 的 `cleanup_partial` 删除该条目主卷与全部分卷, 返回 `VerifyFailed { archive, status, output }`, 整批停止; 已完成的条目保留
- 失败条目**不写入** Report / mapping.csv, 不触发完成进度回调
- `output` 只用于诊断(取 stderr 末若干行); 注意 NanaZip 在 `7z l -ba` 下可能 exit 2 且 stderr 为空, 此时错误文案必须自带"压缩包可能不完整/已损坏"的解释, 不能把 stdout 的条目属性当成原因
- 清理是 best-effort; 即使删除失败, 也必须返回 `VerifyFailed` 并带上产物路径, 便于用户手工删除

**取消与性能**

- 校验进程复用 `Backend::run` 的取消机制: 取消置位 → kill + 回收; 当前条目按半成品清理(即使压缩本身已经成功), 剩余条目不再开始, 与 7.2 语义一致
- 每条目只多一次头部扫描; 相对压缩本身的 IO/CPU 可忽略, 因此**默认开启**; `Options.verify=false`(UI「压缩成功后校验」取消勾选)可关闭
- 不新增 `Phase`: 校验发生在压缩命令成功之后、完成进度回调之前; 校验期间该条目仍显示为进行中, 通过后才标记完成
- 校验发生在压缩进程退出、文件句柄释放之后; 不解决"边压缩边解压"或"校验后磁盘掉电"的问题

**与解压预检的关系**

解压侧的 `list` 预检(见 extract 模块)继续保留: 它面向外来包与目标冲突, 不能假定包一定由本工具产出. 压缩后校验只对本工具刚生成的包负责, 两者命令形态相同但职责不同; 应复用同一套后端路由/命令构造, 避免两处规则漂移.

**与现状差异**: 现状压缩成功后直接记入 Report, 不验证产物; 本方案新增一次结构校验与独立错误类型 `VerifyFailed`.

## 7. 进度与取消

### 7.1 进度模型(核心回调不变, 载荷扩展)

```rust
pub enum Phase { Scanning, Compressing }

pub struct Progress {
    pub phase: Phase,
    pub done: usize,        // 条目计数: 扫描=已扫描; 压缩完成帧=已完成条目数(1 基), 条目内 tick 帧=进行中条目的 0 基序号(7z 系)
    pub total: usize,       // 条目总数
    pub done_bytes: u64,    // 完成帧=已完成条目的 size 累计(扫描阶段为已扫描字节); tick 帧=基础累计 + 条目内字节加权(size × 百分比)
    pub total_bytes: u64,   // Step 1 汇总
    pub entry: String,      // 当前条目名
}
```

- 展示层: 完成度优先 `done_bytes / total_bytes`(total_bytes 为 0 时退化为 done/total); CLI 单行刷新 `[3/17] Novel (2.1GB/26GB)`, Tauri 经 Channel 推送
- 粒度分后端: zip 后端仍是**条目级**, 一个条目压缩期间进度不动; 7z 系后端解析 `-bsp1` 百分比流, 条目内按 size × 百分比推进 `done_bytes`

进度流协议与实测结论见 [progress.md](progress.md)(7z 百分比流 + 解压条目信号加权).

### 7.2 取消

- 取消两个生效点: 条目间由回调 `ControlFlow::Break` 停止(当前条目已跑完, 产物完好计入 Report, 已完成的保留); 条目内由 `CancelFlag` 置位 kill 后端进程(含 6.8 的结构校验进程) —— 当前条目按 6.4 半成品规则清理、不计入 Report, 已完成的保留
- `cancel_current` 的桥接(Tauri `CancelFlag`;CLI 恒不取消)不变

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
| `VerifyFailed { archive, status, output }` | 压缩后结构校验非零退出(包不完整/打不开) | Step 3 |
| `Io(std::io::Error)` | 文件系统 IO | 全程 |

CLI 退出码: `0` 成功(含 cancelled 完整返回), `1` 一般错误(CoreError), `2` 用法错误(clap 默认), `130` 用户取消.

## 9. 测试策略

沿用 TDD, 按三步组织:

- **单测(无外部命令)**
  - 扫描: 临时目录树的大小统计、排序、symlink 不跟随、权限子树 → warnings、空目录 → NoEntries
  - 命名: 随机名格式/唯一性、DuplicateDst(大小写)、OutputExists、OutInsideInput
  - 命令构造: 两后端 argv 表(含 current_dir 断言)、parse_size / format_split_size、64k 下限
  - 校验命令构造: 整包 zip → `unzip -l`; zip -s / 7z / `.001` → `7z l`; stdout 静默与主卷参数断言
- **编排测试(注入 fake 后端, 现有 `compress_with_backend` 注入口)**: 取消在条目间停止、失败停止且半成品被清理、mapping.csv 逐行追加且失败条目缺席; 压缩成功但校验失败 → 产物清理、返回 `VerifyFailed`、不写 Report/mapping、不触发完成进度; 校验期间取消 → 当前条目清理并停批
- **集成测试(真实后端)**: Windows 本机 NanaZip: 小目录 round-trip、分卷+密码 round-trip、错误密码被拒、半成品场景; 正常整包/分卷通过校验, 人工截断的 zip/7z 与缺分卷被拒
- **Linux 实机回归清单(发布前人工, 本机 zip.exe 损坏无法自动化)**: `zip -s` 分卷 round-trip、`-s` + `-P` 组合 round-trip(已知生态风险项)、unzip 6.0 解我们的分卷; 整包走 `unzip -l` 校验、zip -s 走 `7z l` 校验

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
| 9 | 压缩后校验 | 无 | 每条目压缩成功后做一次结构校验(方案 A, 6.8) |

## 11. 非目标与后续改进

按价值排序, 均不在本次规格内(已落地的划掉)。格式维度(zip/7z)与压缩级别已于 2026-10 落地, 见 _docs/compress-modes.md:

1. ~~**取消杀子进程**~~: 已实现(2026-10)——取消立即中断当前条目, 产物按半成品清理; 关闭窗口时外壳置取消并等收尾, 孤儿进程不再残留
2. **manifest 校验 / 全量数据校验**: 方案 A(6.8)只保证包结构可打开; 进一步的 `7z t` / `unzip -t` 全量 CRC 校验与 `manifest.txt`(每包大小 + SHA-256)仍属后续 —— 与本工具搬运场景高度契合, 建议优先
3. ~~**--level 透传**~~: 已实现(2026-10)——`Options.level` 0-9(zip `-<n>` / 7z `-mx=<n>`, 0=store), CLI `--level` / UI「强度」均可调
4. **空间预检**: 输出盘剩余空间 < total_bytes 时提前失败(需 fs4 之类的依赖获取剩余空间, 与零依赖取向冲突, 故列为后续)
5. ~~**字节级进度**~~: 7z 系已实现(2026-10)——压缩走 `-bsp1` 百分比流(见 7.1 与 [progress.md](progress.md)); zip 无机器可读进度, 维持条目级
6. **--dry-run**: 扫描+命名后打印预览表(核心已具备能力, 属 CLI 外壳功能)

## 12. 打包策略(strategy)

> 定位: 可选规划层, 代码 `crates/core/src/strategy.rs`. 输入顶层条目列表 + 策略参数, 输出"哪些条目合并 / 哪些条目分包 / 哪些直接压缩"的纯内存计划; 不执行压缩、不命名、不写盘.
> P1/P2 已落地: `Entry.entry_count`、`crates/core/src/packaging.rs` 的 `plan_packages` / `compress_packages`; 与旧 `compress` 路径并行, 尚未接入 CLI/UI 编排.

### 12.1 目标与非目标

目标:

- 把"零碎条目合并"与"超大条目分包"从压缩执行中分离为可单测的决策层
- 阈值全部由用户配置; 软件只校验参数自洽性并给出默认值, 不替用户判断可接受的风险
- 输出确定性计划: 同一输入 + 同一参数 → 同一计划

非目标:

- 不估计压缩后大小/压缩率; `Split` 只表示"原始大小超过上阈值", 实际卷数由执行层决定
- 不决定包名、随机名、mapping.csv 格式
- 不引入传输、校验、重传逻辑
- 不把单个顶层条目拆到多个合并包; 一个顶层条目最多属于一个输出包

### 12.2 数据结构

```rust
pub struct StrategyItem {
    pub name: String,
    pub size: u64,
    pub entry_count: usize,
}

pub struct StrategyParams {
    pub merge_below: Option<u64>,
    pub split_above: Option<u64>,
    pub bundle_max: Option<u64>,
    pub max_entries: Option<usize>,
}

pub enum PackageKind { Direct, Split, Bundle }

pub struct PackagePlan {
    pub kind: PackageKind,
    pub members: Vec<String>,
    pub input_bytes: u64,
    pub entry_count: usize,
}

pub struct StrategyPlan {
    pub packages: Vec<PackagePlan>,
    pub warnings: Vec<StrategyWarning>,
}

pub fn plan(
    items: &[StrategyItem],
    params: &StrategyParams,
) -> Result<StrategyPlan, StrategyError>
```

`entry_count` = 该条目解压后在包内产生的条目数(普通文件 + 目录; 单文件 = 1, 空目录 = 1), 用于 `max_entries` 约束.

### 12.3 参数

| 参数 | 默认 | 语义 |
|---|---|---|
| `merge_below` | 100 MiB | `size <=` 它 → 合并候选; `None` = 不合并 |
| `split_above` | 2 GiB | `size >` 它 → `Split`; `None` = 不分包 |
| `bundle_max` | 取 `split_above`; 上阈值也 `None` 时取 2 GiB | 单个合并包的原始大小上限; 不启用合并时忽略 |
| `max_entries` | 50_000 | 单个合并包的条目数上限; `None` = 不限制 |

- `StrategyParams::default()` = 上表默认值
- `StrategyParams::disabled()` = 全部 `None`, 每个顶层条目各自 `Direct`
- `max_entries` 只约束合并包; 单条目 `Direct` / `Split` 包的条目数由源目录决定, 本策略无法降低. 若这类目录本身条目过多, 需要执行层的"大目录独立分包"能力, 不在本阶段范围
- 默认值是二进制单位(100 MiB / 2 GiB). 如果目的地按十进制 2 GB 限制, 2 GiB 会超; 用户应显式下调, 软件不擅自改用户设定值

### 12.4 分类规则

按顺序判定, 上阈值优先:

1. `split_above = Some(s)` 且 `size > s` → `Split`(单条目, 执行层分包)
2. `merge_below = Some(m)` 且 `size <= m` → 合并候选
3. 其余 → `Direct`(单条目, 不分包)

边界:

- 恰好等于 `merge_below` → 合并候选(`<=`)
- 恰好等于 `split_above` → `Direct`(分包用严格 `>`)
- 若希望"等于阈值也分包/也直接", 由用户把阈值下调/上调 1 字节; 本模块不提供第二种边界语义

参数校验:

- `split_above` / `bundle_max` / `max_entries` 显式给出时必须 > 0
- `merge_below <= split_above`
- 启用合并时: `merge_below <= effective bundle_max <= split_above`(后一个仅当上阈值给出)
- 条目名在大小写不敏感比较下重复 → `DuplicateName`(输出盘通常是 NTFS/FAT)

### 12.5 合并装箱

候选按 `(size 降序, name 升序)` 排序, 做首次适应递减(FFD):

- 依次尝试放入已有包, 条件: `input_bytes + item.size <= bundle_max` 且 `entry_count + item.entry_count <= max_entries`
- 放不下就开新包
- 单条目 `entry_count > max_entries` 时无法进任何包 → `Direct` + `StrategyWarning::TooManyEntries`
- 规划结束后, 只有一个成员的合并包退化为 `Direct`, 以保留原名; 成员数 >= 2 才是 `Bundle`

结果顺序: 输入顺序中的 `Direct` / `Split` 先输出, 合并包按 FFD 创建顺序追加在末尾; 单成员退化包也追加在末尾.

### 12.6 示例

参数: `merge_below = 100 MiB`, `split_above = 2 GiB`, `bundle_max` 取 2 GiB, `max_entries = 50_000`.

| 条目 | 大小 | 计划 |
|---|---|---|
| FolderA | 26 GiB | `Split` |
| FolderB | 100 MiB | 合并候选(`<=` 下阈值) |
| fileC | 16 KiB | 合并候选 |
| fileD | 1 KiB | 合并候选 |
| fileE | 100 MiB | 合并候选 |
| folderF | 20 MiB | 合并候选 |

合并候选合计约 220 MiB → 一个 `Bundle`(5 个成员). 若候选总量是 50 GiB, 则按 `bundle_max` 切成多个 `Bundle`, 而不是一个 50 GiB 包.

注意: 原例子把 FolderB / fileE 标为"直接压缩"; 按本策略的 `>` / `<=` 规则, 恰好等于下阈值的条目会进合并池. 若希望"等于下阈值直接压缩", 把 `merge_below` 下调 1 字节; 当前实现固定为 `<=`.

### 12.7 与执行层的关系

- `strategy` 只产出 `StrategyPlan`; 执行层(`packaging.rs`)按 `PackagePlan` 分组:
  - `Direct` / `Split`: 一个成员, 沿用原条目名
  - `Bundle`: 多个成员, 合成包名 `_bundle_NNNN`(随机模式为 `zt_*`)
- P2 已实现 `plan_packages` / `compress_packages`:
  - 多源命令: `zip -r out A B C` / `7z a out A B C`; 7z 成员 > 64 时用 `-i@listfile`(UTF-8 + `-spd`), zip 后端上限 10k 源
  - `Bundle` 不切卷; `Split` 才传 `-s`/`-v`, 分包大小取 `split_above`
  - 输出存在性/大小写重名/`OutInsideInput` 在 `plan_packages` 预检
  - 1:N mapping: 有 `Bundle` 或随机名时写 `mapping.csv`, 一包多行; 失败/取消按半成品清理
  - 进度按包计数(`done/total`), `entry` 为包标签; `Split` 的单条目仍是一个包
- `Split` 不携带卷数; 实际卷数由压缩后大小决定, 执行层不预测
- `bundle_max` 是**原始大小**上限; 压缩后可能略大(尤其不可压缩数据 + 每条目头部). 执行层按原始预算装箱, 并在产物阶段复核主卷实际大小; 超过 `split_above` 即失败并清理半成品, 不自动二次分卷/降级
- UI 已切到 `plan_packages` / `planId` / 包级 `compress`; CLI 尚未切到 `packaging` 路径; 旧 `compress` 仍保持"每个顶层条目一个包"

### 12.8 测试

`strategy.rs` 内置单测覆盖:

- 默认/禁用参数, 上下阈值严格边界
- 示例分类与合并
- FFD 装箱, `bundle_max` 恰好装满, `max_entries` 拆包
- 单成员退化, 条目数超限 → `Direct` + warning
- 非法参数, 大小写重复名, 空输入
- partition 不变量(每个条目恰好出现一次), 容量约束, 确定性

`packaging.rs` 单测 + 集成测试覆盖:

- 包级命名/冲突: `_bundle_NNNN`、`zt_*`、已有产物、直接包重名
- 多源命令 argv: Bundle 不切卷、Split 传 `-v`、排除开关去重、zip 排除在成员名之后
- 7z listfile: >64 成员走 `-i@listfile`, 运行后清理
- 包级进度/字节权重、取消保留已完成包、失败/校验失败清理半成品
- 真实后端 roundtrip: bundle / 64k 分卷 / 65 成员 listfile, 解压还原

`cargo test -p zip-tool-core` 覆盖; 真实 roundtrip 在 `crates/core/tests/packaging.rs`.
