# 解压实现技术文档

> 定位: 目标实现规格. 单包解压按"识别 → 预检 → 解压"三步展开, 批量解压作为编排层(第 7 节), 指导 `crates/core` extract 模块的演进.
> 与当前实现的差异在各节末尾以"**与现状差异**"标注, 汇总清单见第 11 节.
> 包形态契约来自 compress.md 6.3, 本文是它的消费端.
> UI(Tauri)对接本文的命令面 / 事件协议 / 组级勾选筛选见 _docs/ui-backend.md; 对 core 的影响是 scan_groups 提升为 pub 并结构化返回(7.3)、组补大小, 语义不变.

## 1. 目标与非目标

### 目标

- **单包模式**: 输入一个压缩包或它的任一分卷, 解压到目标目录; 传非入口卷(.z01 / .zip.002)时自动定位入口卷, 不要求用户知道"该传哪个"
- **批量模式**: 输入 compress 的输出目录, 自动分组全部压缩包, 全量预检后逐组解压到共享 restore 目录, 一次还原整棵输入树
- 密码语义与压缩端一致(整批一个密码, 与 compress 对称; 无密码传 None, 密码错由后端拒绝)
- **还原保证**: compress 的多个包解到同一个 restore 目录, 即完整还原 compress 的输入树(布局语义见第 3 节); 随机包名不影响还原, mapping.csv 仅用于排序与展示(7.3)
- 延续"宁停不错": 未知格式 / 入口卷缺失 / 目标已有同名文件 / zip-slip 路径逃逸, 一律显式报错; 覆盖已有文件必须 `--force` 显式开启
- 阶段级进度回调(7z 系组内另有加权 tick 帧, 见 8.1; CLI 与 Tauri 共用同一回调模型), 错误信息可读(工具缺失 / 密码错 / 缺卷都能说清原因)

### 非目标

- 不自带解压实现, 只封装外部命令(Linux: `unzip`, Windows: `7z`; 入口卷为 `.zip.001` 时强制 `7z`)
- 只承诺本工具产物 + 普通 zip; 7z/rar 等其他格式不承诺不测试(7z 命令顺带能解, 属于免费能力而非契约)
- 不做断点续解 / 选择性解压(见第 12 节); 字节级进度 7z 系已实现(见第 12 节 6)
- 不提供"静默跳过已有文件"的解压模式(违背宁停不错, 见 5.3)

## 2. 总体流程

单包模式(原语, 第 4-6 节):

```
输入: archive 路径(.zip / .zNN / .zip.NNN / 其他) + dest + password
    │
    ▼
Step 1 识别      按扩展名识别包形态, 归一化到入口卷(.zip 或 .zip.001)
    │            产出: ResolvedArchive { 入口卷路径, 后端 }
    ▼
Step 2 预检      入口卷存在 → dest 可用(不存在则创建) → 列包内清单:
    │            zip-slip 校验 + dest 冲突检测(--force 跳过冲突检测)
    ▼
Step 3 解压      定位解压程序 → 构造命令(unzip -o / 7z x -aoa) → 执行
    │            失败: 按清单 best-effort 清理本次解出的文件(--force 除外)
    ▼
dest: 还原后的原始条目布局 + ExtractReport { groups }
```

批量模式(编排层, 第 7 节): 输入为目录时, 先扫描分组, 再对全部组做预检, 最后逐组执行上述原语 —— 与 compress 相同的"先全量预检、再开始产出"两段式.

三步串行执行. Step 1/2 只读不解(除 list 一次后端调用, 只读中央目录, 代价极小), 可独立测试; Step 3 是唯一的重 IO 步骤.

## 3. 包形态与布局还原

### 3.1 包形态(compress 端保证的契约)

| 包形态 | 产物(基础名 B) | 入口卷 | 解压端 |
|---|---|---|---|
| zip 整包 | `B.zip` | `B.zip` 自身 | 平台默认: Linux unzip / Windows 7z |
| zip -s 分卷 | `B.z01 ... B.zip` | **末卷 B.zip** | 仅 7z 系(unzip 不支持分卷包; 实测 Ubuntu 24.04 zip 3.0 的 `zip -s 0` 合并 / `zip -FF` / cat 拼接均产出坏包, 7z 直读分卷组有/无密码均字节一致) |
| zip 经 7z -v 分卷 | `B.zip.001 ...` | **首卷 B.zip.001** | 仅 7z 系(unzip 不认识这种分卷) |

| 7z 整包 | `B.7z` | `B.7z` 自身 | 仅 7z 系 |
| 7z -v 分卷 | `B.7z.001 ...` | **首卷 B.7z.001** | 仅 7z 系 |
| mapping.csv | 随机命名时的附带产物 | 不是包 | 不参与解压, 批量模式下作元数据(7.3) |

7z 格式(整包与分卷)见 _docs/compress-modes.md.

入口卷(entry volume): 后端工具从它开始就能找到整组其余分卷的卷. 两种分卷方案的入口卷方向相反(zip -s 是末卷, 7z -v 是首卷), 这是识别逻辑必须显式处理的核心事实.

### 3.2 布局还原语义

- compress v2 保证包内布局: 目录条目为 `<名>/...`, 文件条目为 `<名>` 本身
- 后端的 `-o`/`-d` 语义是"解到该目录**之下**" → 把 compress 的所有包解到**同一个** restore 目录, 就还原出原始输入树; 不要给每包传独立子目录(会双重嵌套)
- 随机包名只改包**文件名**, 不改包内布局: 解压 `zt_abcd....zip` 得到的仍是 `Novel/book.txt` 等原名路径. 还原布局**不需要** mapping.csv; mapping.csv 的用途是批量模式下的排序与展示(7.3), 以及人工选择性解压时知道哪个随机名包对应哪个原条目

## 4. Step 1: 识别与入口卷归一化

### 4.1 判定规则

按**扩展名**判定(大小写不敏感, Windows 上常见大写 `.ZIP`), 表驱动:

| 输入扩展名 | 判定 | 动作 |
|---|---|---|
| `.zip` | 整包或 zip -s 末卷 | 入口卷即自身, 后端 = 平台默认; 旁有 `.z01` 兄弟卷(= zip -s 分卷组)则**强制 7z**(unzip 读不了分卷组, 见 3.1) |
| `.7z` | 7z 格式整包 | 入口卷即自身, 后端 = **强制 7z**(Linux 也要找 7z) |
| `.001` | 7z -v 首卷(`.zip.001` / `.7z.001`) | 入口卷即自身, 后端 = **强制 7z**(Linux 也要找 7z) |
| `.zNN`(z+数字, 如 `.z01`) | zip -s 非末卷 | 同目录找 `{stem}.zip`: 有 → 归一化为它; 无 → `EntryVolumeMissing` |
| `.NNN`(NNN>001, 即 `.zip.002` / `.7z.002`) | 7z -v 非首卷 | 同目录找 `{stem}.001`(stem 已含 `.zip` 或 `.7z`): 有 → 归一化; 无 → `EntryVolumeMissing` |
| 其他 / 无扩展名 | 非本工具契约格式 | `UnsupportedArchive`(明确报错, 不盲目丢给后端) |

`Path::file_stem` 的行为正好配合: `B.z01` 的 stem 是 `B` → 找 `B.zip`; `B.zip.002` 的 stem 是 `B.zip` → 找 `B.zip.001`, 两条规则统一为"stem + 固定后缀".

### 4.2 产出与边界

```rust
pub(crate) struct ResolvedArchive {
    /// 入口卷路径(.zip 或 .zip.001), Step 2/3 均以此为准
    path: PathBuf,
    /// .zip → 平台默认后端; .zip.001 → 强制 Backend::SevenZip
    backend: Backend,
}
```

- 判定只看扩展名, 不读文件内容: 四种形态的扩展名由 compress 端保证; 外来包内容与扩展名不符时, 后端解压阶段自然失败并透出原因
- 归一化时对找到的入口卷检查 `is_file`(Step 2 的存在性预检由此覆盖大半)

**与现状差异**: 现状仅识别扩展名 `.001`(强制 7z), 其余一律走平台默认后端直接解 —— 传 `.z01` 时 Linux 会拿 unzip 解非末卷(unzip 报错, 信息晦涩), Windows 上 7z 行为未验证; 传 `.zip.002` 时无人定位 `.zip.001`. 目标设计把"任一分卷都能解"变成显式归一化逻辑.

## 5. Step 2: 预检(宁停不错)

### 5.1 检测顺序

1. **入口卷存在且为文件**, 否则 `ArchiveNotFound`
2. **dest 可用**: 已存在但是普通文件 → `DestNotADirectory`; 不存在 → `create_dir_all`(目录创建是核心职责, 不依赖后端隐式行为)
3. **列包内清单**: `unzip -Z1 <入口卷>`(zipinfo 模式, 每行一个名字)/ `7z l -ba -slt <入口卷>`(取 `Path =` 行). zip 格式不加密文件名, 列清单**不需要密码**(也少一次密码经 argv 暴露的机会)
4. 用清单做 5.2 / 5.3 两项检测

### 5.2 zip-slip 校验(路径逃逸)

清单中任一名字满足以下之一 → `UnsafeArchive`, 拒绝解压:

- 绝对路径(以 `/` 或盘符开头)
- 含 `..` 路径段

本工具压出的包不会有这类名字, 但解压端面对的可能是外来包; 解压漏洞的经典形态就是条目路径逃出 dest, 在预检处统一拦截, 不依赖后端各自的防护行为. 比对前把路径分隔符 `/` 与 `\` 归一(两后端清单输出的分隔符不一致).

### 5.3 冲突检测与覆盖策略

- 清单中任一相对路径在 dest 下已存在 → 默认 `ExtractConflict`(列出前若干个冲突路径, 提示用 `--force`)
- `--force` 时跳过本检测, 后端命令带覆盖参数强行解压
- **为什么不用后端的"跳过已有文件"开关**(unzip `-n` / 7z `-aos`): 静默跳过会让 dest 里的旧半成品冒充本次解压结果, 对搬运场景比失败更危险; 后端的交互式覆盖提示在非交互 stdin 下行为不可靠. 结论: 冲突判断自己做, 后端命令始终带覆盖开关(`-o` / `-aoa`), 何时允许覆盖由预检决定

### 5.4 清单的第二个用途

清单在 Step 3 失败时是半成品清理的依据(6.3), 也为后续空间预检/manifest 校验留了数据源(第 12 节).

**与现状差异**: 现状无任何预检 —— dest 存在同名文件时直接被 `-aoa`/`-o` 覆盖; dest 不存在依赖后端隐式创建; 不校验条目路径; 失败后已解出的文件残留且下次重跑必然冲突.

## 6. Step 3: 解压执行(单包原语)

### 6.1 工具定位

执行前在 PATH 里定位解压程序(Linux 的 `unzip`/`7z`, Windows 的 `7z`), 找不到 → `ToolMissing`(可读错误, 指出缺哪个程序), 而不是 spawn 失败后的 os error 2. 定位复用 `doctor::locate_tool`. 解压端程序与压缩端不同(zip 的解压端是 unzip), doctor 后续应增加 unzip 检查项(ToolCheck 是可扩展列表).

### 6.2 命令

| 后端 | 命令 |
|---|---|
| unzip | `unzip -o [-P <pw>] <入口卷> -d <dest>` |
| 7z | `7z x -aoa [-p<pw>] -o<dest> -- <入口卷>` |

与现状 `extract_command` 一致; 覆盖开关恒传(冲突已由 Step 2 拦截, 见 5.3), 密码 argv 传递的威胁模型同 compress.md 6.5, 不重复展开.

- unzip 解 `B.zip` 整包(不认识任何分卷形态)
- 7z 解 `B.zip.001` 时自动找 `.002...`; 解 zip -s 组的末卷 `B.zip` 时自动读 `.z01...` 同伴(缺卷 → 后端失败, 原因透出)
- 分卷解压只认 7z 一家(实测 unzip 及 zip 自带的合并/修复命令都不可用), 这就是 Step 1 按形态路由后端的原因

### 6.3 失败处理与半成品清理

- 后端非零退出 → `BackendFailed`(程序名、退出状态、stderr 末 5 行), 整次解压失败
- **按清单清理本次解出的文件**: 预检通过意味着清单路径在解压前都不存在, 因此"现在存在的清单路径"必是本次解出的 —— 逐个删除, 单个删除失败忽略(best-effort), 不吞掉原始后端错误
- `--force` 模式**不清理**: 覆盖模式下无法区分"本次解出"与"本来就存在"
- unzip 的退出码语义较丰富(1 = 警告、82 = 密码错等), 统一按非零失败处理, 具体原因看透出的输出 —— 不按退出码分派行为

**与现状差异(第 6 节)**: 现状不做工具定位(缺 unzip/7z 时报晦涩的 io error); 失败后已解文件残留, 重跑即冲突且无清理.

## 7. 批量解压(编排层)

输入是一个目录(典型为 compress 的输出目录)时进入批量模式. 核心库同一个入口函数按 `input.is_dir()` 分派: 文件 → 单包原语, 目录 → 本节流程.

### 7.1 CLI 形态

```
# 单包: 任一分卷可作入口
zip-tool extract -i E:\out\B.zip.001 -o D:\restore -p 密码
# 批量: 目录输入, 一次还原 compress 输出目录里的全部包
zip-tool extract -i E:\out -o D:\restore -p 密码 [--force]
```

新增 `--force`(单包/批量共用, 跳过冲突检测). 密码对整批统一(compress 整批一个密码).

### 7.2 流程

```
Step B1 扫描分组   read_dir 顶层 → 分类出压缩包组 + mapping.csv 元数据 + warnings
    │              产出: Vec<ArchiveGroup { 入口卷, 后端, original? }>, 排序完毕
    ▼
Step B2 全量预检   对每组执行第 5 节预检(list + zip-slip + 冲突, 冲突目标为共享 restore 根)
    │              任何组失败 → 整批不开始(在解出任何文件之前失败)
    ▼
Step B3 逐组解压   顺序执行单包原语(第 6 节); 一组失败 → 停批, 已完成的组保留
    ▼
restore 根: 还原的整棵输入树 + ExtractReport { groups, warnings, cancelled }
```

### 7.3 Step B1: 扫描分组

顶层扫描, 不递归(compress 产物都在输出目录顶层). 逐文件分类:

| 文件 | 分类 |
|---|---|
| `X.zip` | zip 组入口卷(整包或 zip -s 末卷) |
| `X.zip.001` / `X.7z.001` | 7z 组入口卷 |
| `X.7z` | 7z 格式整包, 独立组(后端强制 7z) |
| `X.zNN` | 成员卷: `X.zip` 存在 → 静默忽略; 不存在 → 孤儿卷, 记入 warnings, 该组跳过 |
| `X.zip.NNN` / `X.7z.NNN`(NNN>001) | 成员卷: 对应 `.001` 存在 → 静默忽略; 不存在 → 孤儿卷, 同上 |
| `mapping.csv` | 元数据: 读出 `archive,original` 行序与对应关系(见下) |
| 其他文件 | 非包, 记入 warnings(输入目录混杂的提示), 不中断 |

- 一个目录里同时有 `X.zip` 与 `X.zip.001`(或 `X.7z` 与 `X.7z.001`)是两个独立组(入口卷不同), 各自解压; 内容若相撞会由 B2 的冲突检测拦住
- **排序**: 有 mapping.csv 按 mapping 行序(= compress 时的原条目顺序, 与压缩顺序一致), 无 mapping.csv 按入口卷名排序 —— 进度与报告顺序稳定可复现
- 一个组都没有 → `NoArchives`(用户多半指错了目录)

### 7.4 mapping.csv 的角色

- **只是元数据**: 用于排序、进度与报告里显示原名. 还原布局不依赖它(3.2)
- 行的 `archive` 列就是主卷文件名 = 入口卷, 直接对上分组结果; 对不上的行(指向不存在的包)与解析失败的 mapping.csv → 记入 warnings, 降级为按名排序, 不中断
- 解压完成后 mapping.csv 留在原地不动, 不复制不删除

### 7.5 Step B2/B3: 全量预检与逐组执行

- B2 对每组做第 5 节预检, 冲突比对目标是**共享的 restore 根**: 先全量预检再解任何文件, 与 compress"任何压缩开始之前完成全部预检"同哲学 —— 避免解了一半才发现后面的包冲突
- B3 顺序逐组执行单包原语, 一组失败 → 停批(后续组不开始), 已完成组保留, 失败组按 6.3 清理半成品. 与 compress 6.4 的停批语义一致; 顺序执行理由同 compress 6.1(机械盘/USB 上顺序 IO 优先)
- dest 在输入目录内或与之相同**不报错**: 扫描在 B1 已完成, 后续写入不影响结果(与 compress 的 `OutInsideInput` 不同, 那里是边扫边写会污染产物)

### 7.6 报告

```rust
pub struct ExtractReport {
    /// 已完成的组, 按执行顺序
    pub groups: Vec<ExtractedGroup>,
    /// 孤儿卷 / 非包文件 / mapping 异常等非致命信息
    pub warnings: Vec<String>,
    /// 批量模式下用户取消(组间 Break 或组内杀进程)
    pub cancelled: bool,
}

pub struct ExtractedGroup {
    /// 入口卷文件名
    pub entry: String,
    /// mapping.csv 提供的原条目名(无 mapping 时为 None)
    pub original: Option<String>,
    /// 该组解出的文件数(来自 list)
    pub files: usize,
}
```

单包模式返回同一类型(恰好一个组), 两外壳无需区分两种返回.

## 8. 进度与取消

### 8.1 进度

- `Phase` 增加 `Extracting` 变体, 复用现有 `Progress` 结构与回调模型(签名与 `compress_dir` 相同)
- **单包**: 预检通过后、后端执行前回调一次(`entry` = 入口卷文件名, `total` = 组数, 单包为 1), 结束后回调一次
- **批量**: 每组开始/结束各回调一次, `done/total` = 组计数, `entry` = 原名(mapping 提供时)否则入口卷名
- 粒度分后端: unzip 维持**阶段/组级**(组内不推进, 展示层显示"解压中 <条目名>", total_bytes 为 0 时隐藏字节段); 7z 系后端组内按预扫清单字节加权推进 `done_bytes`(stdout 条目名信号), 展示层显示 (x/y) 字节
- API 变化: `extract` 返回 `Result<()>` → `Result<ExtractReport>` 并增加回调参数, 参数名 `archive` → `input`(文件或目录), 是 CLI 与 Tauri 两外壳的共同 breaking change, 随演进同步改

进度流协议与实测结论见 [progress.md](progress.md)(7z 百分比流 + 解压条目信号加权).

### 8.2 取消

- 回调返回 `ControlFlow::Break`: 批量模式在**组间**生效(当前组已解完, 计入报告, 后续组不开始); 单包模式仅"开始前"生效
- `CancelFlag` 置位: 组内立即 kill 解压进程, 本组按清单回滚(默认; `--force` 无法区分新旧, 不回滚)后停止, 与 compress 的取消语义一致
- CLI Ctrl+C 是控制台信号, 本进程与后端子进程同组同死, 等效硬中断: dest 留半成品, 重跑需清空 dest 或 `--force`(半成品在下次运行时会被冲突预检拦住, 不会静默覆盖)

**与现状差异**: 现状 `extract` 无进度回调、无取消参与点、无批量模式.

## 9. 错误模型与退出码

`CoreError` 在现有基础上扩充(全部有明确 Display 文案):

| 变体 | 触发 | 阶段 |
|---|---|---|
| `NoArchives(PathBuf)` | 批量模式: 输入目录里没有任何压缩包组 | Step B1 |
| `ArchiveNotFound(PathBuf)` | 入口卷不存在 | Step 2 |
| `UnsupportedArchive(PathBuf)` | 扩展名非 zip 系四形态 | Step 1 |
| `EntryVolumeMissing { got, expected }` | 传了非入口卷且同目录找不到入口卷 | Step 1 |
| `DestNotADirectory(PathBuf)` | dest 已存在但是文件 | Step 2 |
| `UnsafeArchive { path, reason }` | 清单含绝对路径或 `..` 段 | Step 2 |
| `ExtractConflict(Vec<String>)` | dest 已有同名文件(提示 `--force`) | Step 2 / B2 |
| `ToolMissing(String)` | 解压程序不在 PATH | Step 3 |
| `BackendFailed { .. }` | 后端命令非零退出(含密码错、缺卷) | Step 3 |
| `Io(std::io::Error)` | 文件系统 IO | 全程 |

孤儿卷、非包文件、mapping 异常走 warnings 不走错误(7.3/7.4, 批量模式下它们不妨碍其余组).

CLI 退出码沿用: `0` 成功(含 cancelled 完整返回), `1` 一般错误(CoreError), `2` 用法错误(clap 默认), `130` 用户取消(批量模式组间取消).

## 10. 测试策略

沿用 TDD, 按三步与编排层组织:

- **单测(无外部命令)**
  - 识别与归一化: 表驱动覆盖四形态 × 大小写、`.z01` → 定位 `.zip`、`.zip.002` → 定位 `.001`、入口卷缺失、未知扩展名拒绝
  - 批量分组: 混合目录(整包 + 两组分卷 + 孤儿卷 + mapping.csv + 杂文件)的分类、排序(mapping 行序 / 按名降级)、空目录 → NoArchives
  - zip-slip 清单校验: 绝对路径 / `..` 段 / 正常名字各自判定
  - `CoreError` 各新变体的 Display 文案
- **编排测试(注入 fake 后端, 扩展现有 `CommandBackend` 注入口覆盖 extract_command / list)**: 冲突默认拒绝且列出路径、`--force` 跳过冲突、失败按清单清理、`--force` 失败不清理、ToolMissing; 批量: 全量预检失败则一组都不解、组间停批、组间取消、mapping 缺行降级 + warnings
- **集成测试(真实后端, 本机 NanaZip)**: round-trip(压 → 删源 → 用 zip-tool 解 → 逐字节比对)、`.zip.001` 分卷 round-trip、错误密码被拒、`.z01` / `.zip.002` 入口归一化、冲突默认拒绝与 `--force` 覆盖、半成品清理、手工构造的 zip-slip 包被拒; 批量: compress 随机名 + 分包的输出目录整体还原、孤儿卷 warning
- **实机验证 / 回归清单(发布前人工)**
  - Windows `7z x` 解 zip -s 组(`B.zip` + `.z01` 同伴) —— 跨平台搬运的关键场景(Linux 压的包在 Windows 解), 7z 对 Info-ZIP 分卷的支持未验证
  - Linux: `unzip -Z1` 清单输出、zip -s 分卷由 7z 直读(`7z x B.zip` 识别 `.z01` 同伴)、unzip 对 `.zip.001` 报错(验证路由)、退出码 82(密码错)
- **E2E(test crate)**: 现有 E2E 用系统工具解压是 compress 正确性的基准, 保持不动; 另加"用 zip-tool extract 批量解压自己压缩的输出目录"的对称用例, 闭环验证解压端

## 11. 与当前实现的差异汇总(演进清单)

| # | 项 | 现状 | 目标 |
|---|---|---|---|
| 1 | 入口卷 | 仅识别 `.001`; 其余按平台默认后端直接解 | 四形态识别 + 归一化(`.z01`→`.zip`、`.zip.NNN`→`.zip.001`), 任一分卷可作入口 |
| 2 | dest 目录 | 依赖后端隐式创建 | 核心 `create_dir_all`; 存在同名文件 → `DestNotADirectory` |
| 3 | 覆盖策略 | `-aoa`/`-o` 无条件覆盖 | 冲突预检默认拒绝, `--force` 才覆盖 |
| 4 | 包内清单 | 无 | list 一次(`unzip -Z1` / `7z l -ba -slt`), 供冲突检测、zip-slip 校验与清理 |
| 5 | zip-slip | 不校验 | 清单校验绝对路径与 `..` 段, 拒绝 |
| 6 | 失败处理 | 报错返回, 已解文件残留 | 按清单 best-effort 清理(`--force` 除外) |
| 7 | 工具缺失 | spawn 失败报 io error | 预先定位, `ToolMissing` 可读报错 |
| 8 | 进度 | 无回调 | `Phase::Extracting` 阶段/组级回调(7z 系组内另有加权 tick 帧, 见 8.1) + `ExtractReport`(breaking) |
| 9 | 批量模式 | 无(用户逐包手工调用) | 目录输入自动分组 + mapping.csv 排序/展示 + 全量预检 + 逐组解压停批 |

## 12. 非目标与后续改进

按价值排序, 均不在本次规格内:

1. **manifest 校验**: 解压后校验 SHA-256(与 compress.md 11.2 的 manifest 输出配对)
2. **doctor 增加 unzip 检查项**: Linux 解压依赖 unzip, ToolCheck 列表天然可扩展
3. ~~**取消杀子进程**~~: 已实现(2026-10), 与 compress 同步——组内取消 kill 解压进程并按清单回滚
4. **空间预检**: list 带上条目大小 + 输出盘剩余空间比对(需 fs4 之类依赖, 与零依赖取向冲突)
5. **选择性解压**: 按 mapping 或通配只解部分条目(批量模式已具备分组与 mapping 基础设施); UI 的勾选筛选是**组级**选择, 已在 _docs/ui-backend.md 落地, 此处指解包内容级的更细粒度
6. ~~**字节级进度**~~: 7z 系已实现(2026-10)——压缩走 `-bsp1` 百分比流, 解压走预扫大小 + 条目名信号加权(见 [progress.md](progress.md)); unzip 无机器可读进度, 维持组级
