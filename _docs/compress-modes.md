# 压缩格式双模式(format + level)技术文档

> 定位: 目标实现规格. 在 _docs/compress.md 与 _docs/extract.md 基础上引入**格式维度**, 两文档受影响的条目按第 9 节同步修改.
>
> **核心决策(用户拍板)**:
> - "模式"概念**只存在于 UI 层**(两个预设参数组合); core 与 CLI 只有底层参数 `format` + `level`, 不知道模式的存在
> - 默认 format = zip(向后兼容)
> - UI 形态 = 预设 + 高级微调
> - 解压侧 `.7z` 支持纳入本次范围
>
> **推翻的既有约束**: compress.md §1 非目标"输出统一为 zip 格式"、§6.2"不暴露压缩级别, 用后端默认".

## 1. 目标与非目标

### 目标

- 压缩支持两种格式:
  - **zip**(Deflate): 兼容性最大(Windows 资源管理器 / macOS / Linux unzip 皆可开)
  - **7z**(LZMA2): 压缩率显著优于 zip(同参数下通常再小 20-40%), 需 7z 系工具解压
- 压缩级别 0-9 透传(0 = 仅存储), core / CLI 暴露为原始参数
- UI 提供"最大兼容性 / 平衡"两个预设(参数组合快捷方式), 外加高级微调(格式 + 强度)
- 解压侧支持 `.7z` 单包与 `.7z.001` 分卷, 批量扫描分类同步支持
- 密码语义随格式: zip → ZipCrypto(通用但弱), 7z → AES-256(强); 文档与 UI 明示

### 非目标

- 不自带压缩实现(维持"只封装外部命令"原则, 见 compress.md §1)
- 不新增第三种格式; 算法固定: zip 格式 = Deflate, 7z 格式 = LZMA2(不暴露 `-m0` 选择)
- 不改"逐条目独立压缩包"的编排与事件协议(compress.md §7 不变)
- 不做 solid 压缩、多线程参数调优(后端默认)

## 2. 参数面(core)

```rust
/// 压缩包格式. 决定外部工具、命令参数、产物扩展名、分卷命名与加密方式
pub enum Format {
    /// zip 格式: Linux 走系统 zip 命令, Windows 走 7z -tzip
    Zip,
    /// 7z 格式: 两平台都走 7z 命令(-t7z -m0=LZMA2)
    SevenZ,
}

pub struct Options {
    // ...split_size / password / random_name 不变...
    pub format: Format,      // 默认 Zip
    pub level: Option<u8>,   // 0-9; None = 不传级别 flag, 用后端默认
}
```

- `Format::extension()` → `"zip"` / `"7z"`; `Format::backend()` → 工具选择: `Zip` 格式 → `Backend::native()`(平台原生), `SevenZ` 格式 → `Backend::SevenZip`(两平台统一)
- `Backend` 结构与职责不变(工具定位 + 进程执行); `compress_command` 增加 format/level 入参, 命令参数由格式决定(见 §3 矩阵)
- 级别参数映射(level 统一 0-9 语义; 注意 Info-ZIP zip 的级别 flag 是数字本身, 如 `-9`):

| 格式 | 工具 | level = Some(n) | level = None |
|---|---|---|---|
| Zip | zip(Linux) | `-<n>`(如 `-9`) | 不传(zip 默认 -6) |
| Zip | 7z -tzip(Windows) | `-mx=n` | 不传(7z 默认 -mx=5) |
| SevenZ | 7z -t7z | `-m0=LZMA2 -mx=n` | `-m0=LZMA2`(7z 默认 -mx=5) |

- `-m0=LZMA2` 显式传递: 防旧版 p7zip 默认 LZMA1, 保证两平台算法一致
- zip 命令的 64k 分卷下限约束(`check_split_size`)不变, 仍仅对 `Backend::Zip` 生效

## 3. 压缩侧

- **dst 命名**(plan): 产物为 `{dst}.zip` → 按 format 为 `{dst}.zip` 或 `{dst}.7z`; 随机名规则不变(`zt_` 前缀 + 扩展名随 format)
- **Entry.format**: 由 plan 按 `opts.format` 统一填充; `scan_dir` 不再硬编码 `"zip"`(扫描先于格式选择, 不感知格式)
- **OutputExists 预检**(plan):

| format | 检查存在性 |
|---|---|
| Zip | `{dst}.zip` 或 `{dst}.zip.001` |
| SevenZ | `{dst}.7z` 或 `{dst}.7z.001` |

- **半成品清理**(6.4 规则扩展):

| format | 精确匹配删除 |
|---|---|
| Zip | `{dst}.zip`、`{dst}.z[0-9][0-9]`、`{dst}.zip.[0-9][0-9][0-9]` |
| SevenZ | `{dst}.7z`、`{dst}.7z.[0-9][0-9][0-9]` |

- **分卷**: SevenZ 格式 `-v<size>` → `{dst}.7z.001, {dst}.7z.002, ...`(首卷 `.001`); Zip 格式不变(Linux `.z01..B.zip` 末卷制 / Windows `.zip.001` 首卷制)
- **密码**: Zip 格式 → ZipCrypto(处处能解, 密码学上已破, 与"最大兼容"定位自洽); SevenZ 格式 → AES-256. core 注释、README、UI 提示三处明示
- 命令构造矩阵(compress_command, `--` 与相对条目名规则不变):

| format | 平台 | 命令骨架 |
|---|---|---|
| Zip | Linux | `zip -r [-s <size>] [-P <pw>] [-<0-9>] -- <out_abs>/{dst}.zip <name>` |
| Zip | Windows | `7z a -tzip [-v<size>] [-p<pw>] [-mx=n] -- <out_abs>/{dst}.zip <name>` |
| SevenZ | 两者 | `7z a -t7z -m0=LZMA2 [-v<size>] [-p<pw>] [-mx=n] -- <out_abs>/{dst}.7z <name>` |

## 4. 解压侧

- **resolve_archive** 扩展(大小写不敏感, 规则与 `.zip` 系对齐):

| 输入 | 解析 |
|---|---|
| `X.7z` | 单包, 强制 `Backend::SevenZip`(unzip 不认 7z) |
| `X.7z.001` | 分卷首卷, SevenZip |
| `X.7z.NNN`(NNN>001) | 归一化到同目录 `X.7z.001`(缺失 → EntryVolumeMissing) |

  - 7z 分卷只有 `.7z.NNN` 一种形态, 无 zip 系 `.zNN` 别名
- **scan_groups**(B1 分类): 入口卷集合扩为 `{X.zip, X.zip.001, X.7z, X.7z.001}`, 成员卷集合扩为 `{X.zNN, X.zip.NNN, X.7z.NNN}`; 孤儿卷 / 非包文件 / mapping.csv 规则不变; zip 组与 7z 组可混处同一目录
- **extract_command**: 零改动(7z `x` 自动识别容器格式); `.7z` 永不映射到 unzip(resolve 层强制保证)
- **工具要求**: Linux 解 `.7z` 需 p7zip 的 `7z` 命令; 压缩/解压统一在 PATH 预定位工具, 缺失报 `ToolMissing`(与解压既有行为一致)。README 工具矩阵与 doctor 体现

> 勘误(落地时): 原文写"spawn 失败(不在 PATH)走既有 BackendFailed 路径"——实现时改为与解压对齐的 `ensure_tool_available` 预检, 两条路径的错误语义统一为 `ToolMissing`。

## 5. CLI

- `compress` 新增:
  - `--format <zip|7z>`, 默认 `zip`
  - `--level <0-9>`, 默认不传(= 后端默认强度; 与 UI 初始预设行为一致, 均不突变拉满)
- `extract` 无新参数(格式由输入扩展名决定)
- 报告打印格式不变, 产物扩展名自然体现格式

## 6. UI(Tauri)

- 压缩选项行新增:
  - **模式预设**: 分段单选 `[最大兼容性 | 平衡]`, 初始选中"最大兼容性"
  - **「高级」展开区**: 格式单选(zip/7z) + 强度步进器(0-9)
- 预设映射: 最大兼容性 → (zip, 后端默认强度, UI 不传 level); 平衡 → (7z, 5); 手动改动高级参数 → 预设区显示「自定义」(清除预选高亮)。2026-10 起 zip 预设不再拉满 9(压缩时间过长, 后端默认 zip = -6)
- 密码联动提示(密码输入旁一行): zip 格式+密码 → 「ZipCrypto: 兼容性最好, 强度弱」; 7z 格式+密码 → 「AES-256」
- CompressOptions DTO 增加 `format: "zip"|"7z"` 与 `level: number|null`(留空/null = 后端默认强度; 手动填了才传具体数值; `level` 类型在 core 侧仍为 `Option<u8>`)
- 解压侧: scan_groups 返回的 7z 组自然进表格(展示/勾选/meta 行规则不变), 无额外 UI 改动; 浏览器 mock 路径同步

## 7. doctor

- 检查列表从"平台 native 工具"扩为平台工具全集(复用 check_backend_tool 定位逻辑):
  - Linux: `zip`、`7z`、`unzip`
  - Windows: `7z`(zip 格式与 7z 格式压缩都走 7z)
- 退出码规则不变(任一 FAIL → 1); Linux 上 `7z` 缺失仅影响 7z 格式与解 `.7z`, 该项文案标注适用范围

## 8. 测试计划

- **core 单测**:
  - compress_command 参数矩阵: format × level(Some 各档/None) × split × password(按 Backend 显式构造, 两分支可测; Format→Backend 映射按当前平台断言)
  - plan: dst 扩展名随 format; OutputExists 按 format
  - 半成品清理: SevenZ 格式的 `{dst}.7z` + `{dst}.7z.NNN` 模式
  - resolve_archive: `.7z` / `.7z.001` / `.7z.005` 归一化 / 大小写 / EntryVolumeMissing
  - scan_groups: 纯 7z 组、zip+7z 混合目录、孤儿卷
- **E2E**(NanaZip 本机可验): 7z 格式 roundtrip(单包 / `-v` 分卷 / 密码); 既有 zip 格式用例回归
- **UI node test**: 预设 → 参数联动、手动改 → 自定义状态、密码提示随格式切换
- 全 workspace 回归(现状 86 测试)

## 9. 文档同步与现状差异汇总

| 文档 | 修改 |
|---|---|
| compress.md | §1 非目标改述("输出统一为 zip"→"格式由参数决定"); §6.2 命令表加 format 维度, 移除"不暴露压缩级别"; §6.3 产物表加 7z 行; §6.4 清理模式加 7z 行 |
| extract.md | 补 `.7z` / `.7z.001` 解析与 B1 分类规则 |
| README | 工具矩阵(zip 格式: Linux zip / Windows 7z; 7z 格式: 两平台 7z; 解 `.7z`: 需 7z)+ 密码语义说明 |
| 本文档 | 格式维度的目标规格 |
