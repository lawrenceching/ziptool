# 设置页面与文件排除策略技术方案

> 状态: 已实现(2026-10-20, P0–P6); 决策 D-1 / D-2 / D-3 / D-4 / D-6 / D-11 / D-12 均已落地
> 范围: `crates/tauri-app`(UI + 壳层)与 `crates/core`(策略展开); CLI 行为不变
> 相关: [exclude-rules.md](exclude-rules.md)(模式语法与后端归一化)、[ui-backend.md](ui-backend.md)(命令面 / DTO / 状态机)、[i18n.md](i18n.md)(词条规则)、[compress.md](compress.md)(压缩编排 / 取消)、[progress.md](progress.md)
>
> **已确认**: D-1 `lang-temp` 默认开、D-2 `os-hidden` 默认开、D-3 设置存 `app_config_dir()/settings.json`、D-4 关机本期仅 UI、D-6 内建清单按当前表、D-11 OS 元数据并入 `os-hidden`、D-12 关机控件 **B(置灰 + 即将支持)**
> **实现前如对 D-5 / D-7 / D-8 / D-9 有异议**: 见 §15.2; 未反对则按建议执行

## 1. 需求与目标

### 1.1 原始需求

- UI 增加设置页面, 结构为**边栏 + 内容区**
- 边栏目前包含 **通用**、**文件排除** 两项
- 通用: 提供 **压缩完成后关机** 勾选框
- 文件排除: 配置**文件排除策略**, 目前内建两种:
  1. 操作系统隐藏文件
  2. 编程语言的临时文件
- 策略以**列表**展示, 每项为 **标题 + 描述 + Switch**:

  ```
  编程语言的临时文件                          [ Switch ]
  排除编程语言的临时文件, 例如 JavaScript 的 node_modules, ...
  ```

### 1.2 目标

- 设置页与原主界面在同一文档内切换, 不丢扫描结果、勾选、滚动位置等内存状态
- 设置**持久化**到用户配置目录, 重启后保持; 读取失败安全回退默认值
- 排除策略在 **core 统一展开**为既有 `ExcludeRules`, 不新增第二条排除管线:
  - 扫描统计与后端实压内容仍然一致(沿用 exclude-rules.md 的不变式)
  - 策略变更后扫描结果自动失效, 不会带着旧规则执行压缩
- 通用区提供「压缩完成后关机」**勾选框**, 本期只做 UI 占位:
  - **置灰不可交互**, 旁边/描述注明「即将支持」; 默认不勾选
  - **本期不实现关机行为**: 不注册 `power_off` 命令、不做倒计时 / 横幅 / 平台关机命令; 实际行为由后续 ticket 消费 `shutdownAfterCompress`
  - 字段保留在 `settings.json` 中(默认 `false`), 但本期 UI 不接受用户修改; 若读到未来版本写入的 `true`, 只展示并原样保留, 不丢失
- 全部新增文案进入 i18n 五语言词条; 不新增前端构建器、不放宽 CSP

### 1.3 非目标

- 不改 CLI 参数与 CLI 默认排除行为(CLI 不读 UI 设置)
- 不把既有的「高级 → 排除模式」输入搬进设置页; 自定义模式仍按次生效
- 不支持用户新建 / 编辑策略、策略排序、导入导出、云同步
- 不把语言偏好从 `localStorage` 迁入设置文件(见 §5.9)
- 不改压缩 / 解压的预检、命名、分包、进度协议
- **不实现实际关机**: 无 `power_off` 命令、无倒计时 / 横幅 / 平台命令 / dry-run; 这些属于后续 ticket
- 本期关机控件**置灰**, 不接受输入; 不因为点击 / 键盘操作写入 `shutdownAfterCompress`
- 不做多窗口同步(单窗口应用)
- 不本地化 core / 壳层返回的告警与错误原文(与 i18n.md 的非目标一致)

## 2. 现状与约束

### 2.1 现状

| 层 | 现状 | 与本方案相关的点 |
|---|---|---|
| UI | `ui/index.html` + `ui/app.js`, 纯静态无打包器 | 单视图; 选项行已有「高级 → 排除」文本框; 语言下拉在状态栏 |
| 壳层 | `crates/tauri-app/src/lib.rs`, 5 个命令 | `scan` / `compress` 接收 `excludes`; `ScanContext` 保鲜并比对规则; UI 恒定 `include_hidden=false` |
| core | `ExcludeRules { include_hidden, patterns }` | Unix = `.*` 名字隐藏 + 属性隐藏(Windows); 用户模式按统一语法匹配; 后端命令按 exclude-rules.md §3 归一化 |
| 设置 | 无 | 只有 `localStorage["ziptool.locale"]`(语言偏好) |
| 测试 | Rust 单测 + 壳层单测 + Node UI 单测 + CLI e2e | UI 测试「表面真实、传输造假」, 命令名与参数形状按真实链路校验 |

### 2.2 关键约束

1. **单一排除管线**: 策略必须最终落成 `ExcludeRules`, 否则扫描统计、Windows 属性隐藏收集、后端 `-x` 开关、`ScanContext.rules_eq` 会出现两套语义。
2. **数据安全优先**: 误排除 = 压缩包里缺文件; 漏排除只是多占空间。内建名单必须保守, 且随时可关。
3. **扫描可能很贵**: 大盘扫描分钟级, 策略切换不能在每次 toggle 时立即重扫; 只在离开设置页且确实影响当前压缩扫描时重扫一次。
4. **core 无第三方依赖**(利于 devcontainer / CLI); 设置文件的 serde 结构留在壳层, core 只保留纯逻辑。
5. **单任务互斥**: 扫描 / 压缩 / 解压运行中不允许打开设置页, 避免设置页遮住「取消」入口。
6. **离线 / CSP 不变**: 不引入远程资源; 设置文件读写走 Tauri 命令。

## 3. 关键设计决策

| # | 决策 | 结论 | 理由 |
|---|---|---|---|
| D1 | 策略在哪里展开 | **core 提供策略目录与 `effective_rules()`**, 壳层调用, UI 只传策略 id | 单一实现、可 Rust 单测、未来 CLI 可复用; 避免 JS 与 Rust 两套名单漂移 |
| D2 | 设置存哪里 | **`app_config_dir()/settings.json`**, 壳层 `load_settings` / `save_settings` | 无新插件依赖; 设置属于应用而非 WebView; 可被壳层校验与测试; localStorage 只用于浏览器预览 |
| D3 | 策略如何序列化 | **策略 id 数组** `enabledPolicies`, 不用布尔字段 | 将来新增策略无需改 schema; 未知 id 可前向兼容地忽略 |
| D4 | 设置页形态 | **同一文档内视图切换**(`#view-main` / `#view-settings`), 不是独立 HTML、不是路由、不是模态框 | 保留扫描表 / 勾选 / 滚动状态; 无打包器也能实现; 桌面应用无 URL 需求 |
| D5 | 保存方式 | **即时保存**: 每次切换立即写盘, 失败回滚开关并提示, 无「保存」按钮 | 设置项少; 避免「改了没保存」; 失败可恢复 |
| D6 | 策略变更后的扫描 | 若当前是压缩模式且已有扫描结果, **关闭设置页时自动重扫**; 否则只标记 | 既保证规则与结果一致, 又不打断用户逐项切换; 壳层 `ScanContext` 仍是最终兜底 |
| D7 | 关机范围 | **本期仅 UI 占位**: 控件置灰 + 「即将支持」, 不实现实际关机, 不接受修改; 字段保留, 后续 ticket 消费 | 与 D-4 / D-12 的确认一致; 避免发布一个「看起来能关机、实际不关」的开关 |
| D8 | 与自定义模式的关系 | **保留高级区「排除」输入**, 与策略模式**并集** | 不破坏现有用法; 策略负责全局默认, 自定义负责单次特例 |
| D9 | 内建名单尺度 | 只收**明确由工具生成、可再生**的缓存/临时物; 不收 `build` / `dist` / `target` / `vendor` 等通用名 | 通用名碰撞会误删用户数据; 宁可漏排, 用户可用自定义模式补 |
| D10 | CLI | **不变** | 本次只做 UI 设置; CLI 已有 `--exclude` / `--include-hidden-files`, 后续可接同一策略目录 |

## 4. 文件排除策略

### 4.1 策略目录

| id | 标题(zh-CN) | 默认 | 展开方式 |
|---|---|---|---|
| `os-hidden` | 操作系统隐藏文件 | **开**(已确认) | `ExcludeRules.include_hidden = false` + OS 元数据模式 |
| `lang-temp` | 编程语言的临时文件 | **开**(已确认) | 追加语言临时模式 |

- 开关语义为**正向**: 「排除 …」= 开。core 的 `include_hidden` 是反向命名, 映射时取反, 见 §4.2。
- `os-hidden` 默认开: **隐藏判定口径与现状一致**(现状 UI 恒 `include_hidden=false`), 但额外加入 6 条跨平台 OS 元数据模式(§4.3), 因此默认产物会比现状多排除 `Thumbs.db`、`.DS_Store` 等非隐藏口径的元数据文件。D-11 已确认并入, 这是有意的行为增强。
- `lang-temp` 默认开(D-1 已确认)会让 `node_modules`、`__pycache__` 等从默认压缩内容中消失, 是**有意的行为升级**; 扫描表仍会显示被排除的顶层条目(默认不勾选), 用户可手动勾选覆盖。若要改回默认关, 只改 core 默认函数与 UI 默认常量即可。

### 4.2 展开映射

```text
enabled = normalize(enabledPolicies)          # 去未知 id、去重、按目录顺序
os_hidden  = "os-hidden"  ∈ enabled
lang_temp  = "lang-temp"  ∈ enabled

patterns = []
if os_hidden: patterns += OS_METADATA_PATTERNS
if lang_temp: patterns += LANG_TEMP_PATTERNS
patterns += customExcludes                     # 高级区, 按次传入
patterns = dedupe(patterns)                    # 斜杠归一后精确去重, 保留首次出现
validate_patterns(patterns)                    # 非法即报错, 不开工

ExcludeRules {
    include_hidden: !os_hidden,
    patterns,
}
```

- `os-hidden` 关 = `include_hidden=true`: Unix 不再注入 `.*`, Windows 不再收集隐藏属性, 元数据模式也不加。若仍想排除 `Thumbs.db` 等元数据, 把对应名字填入高级区自定义模式。
- `lang-temp` 关 = 不追加语言模式; 自定义模式照旧生效。
- 策略模式与自定义模式**没有否定语义**, 并集生效; 用户要「只排除 A 不排除 B」时关掉策略、用自定义模式表达。
- 展开只发生在扫描 / 压缩入口各一次, 之后仍走 `scan_dir` / `compress_entries` / 后端 `exclusion_args` 的既有路径。

### 4.3 内建模式清单(v1)

> 本表是实现的唯一来源; 代码常量 `OS_METADATA_PATTERNS` / `LANG_TEMP_PATTERNS` 必须与本表一致, 并有测试断言每条模式能通过 `validate_patterns`。语法限制见 exclude-rules.md §3: 只有 `*`、`?`、`**`(仅结尾), 没有字符类 `[...]`。

#### 这张表怎么评审

- **本表是建议的默认名单, 不是需求方给定的硬性规范**。审核时可以直接在表里增删 / 修改行, 或口头告诉我「删 X、加 Y」; 最终表会原样落成 core 里的常量, 不需要你改代码。
- **增**一行 = 默认多排除一类东西。例:
  - 加 `target/**` → 排除所选条目根下的 `target`(Rust 构建目录);
  - 加 `*.log` → 任意深度的 `.log` 文件都不进包;
  - 加 `.git` → 连 Git 历史一起排除(风险很高, 不建议)。
- **删**一行 = 默认恢复包含。例:
  - 删 `venv` → 名为 `venv` 的目录恢复进包(少一点误排风险, 包更大);
  - 删 `*.class` → Java 编译产物进包。
- **匹配语义决定「一行」覆盖什么**:
  - 不含 `/` 的模式 = **任意深度的文件/目录名(basename)**; `node_modules` 会命中 `A/node_modules`、`A/B/node_modules`。
  - 含 `/` 的模式 = **从所选源目录的条目根锚定**; `.yarn/cache` 只命中条目根下的 `.yarn/cache`, 不会命中 `sub/.yarn/cache`。
  - 只支持 `*`(不跨 `/`)、`?`(单字符)、`**`(仅结尾); 区分大小写; 没有 `[...]` 字符类。
- **增删的实际影响**:
  - 顶层被排除条目仍会显示在扫描表里、默认不勾选, 可手动勾选覆盖;
  - 嵌套文件不会单独显示, 只影响目录大小统计与最终包内容 —— 所以把通用名(`target`、`build`、`cache`)加进去的误排风险明显更高。
- **同步成本**: 增删后实现会同步 core 常量、单测、设置页描述里的示例、README 的默认值说明; 这些由实现方处理, 审核只需对「表」拍板。
- **结论**: 你可以直接回复「保留当前表」或「删 `venv`, 加 `target/**`」这类变更; 没有意见就按当前表执行。

#### 操作系统元数据(`os-hidden`)

| 模式 | 说明 |
|---|---|
| `Thumbs.db` | Windows 缩略图缓存 |
| `ehthumbs.db` | Windows 媒体缩略图缓存 |
| `desktop.ini` | Windows 文件夹配置 |
| `.DS_Store` | macOS Finder 元数据 |
| `._*` | macOS AppleDouble 资源分支 |
| `__MACOSX` | macOS 解压产生的资源目录 |

- 这 6 条是**跨平台补齐**: 在 Linux 上复制 Windows 盘、或在 Windows 上复制 macOS 盘时, 对方的「隐藏」口径不生效。
- `$RECYCLE.BIN` / `System Volume Information` 不列入: Windows 上通常带隐藏属性, 已由 `include_hidden=false` 覆盖; 其他平台若需要, 用户可自定义。
- 上述模式与 Unix `.*` 名字隐藏、Windows 属性隐藏**叠加**, 不替代。

#### 编程语言 / 构建工具临时物(`lang-temp`)

| 分组 | 模式 | 说明 |
|---|---|---|
| JavaScript / TypeScript | `node_modules` | npm / pnpm / yarn 依赖目录(任意深度) |
| | `.pnpm-store` | pnpm 内容寻址存储 |
| | `.yarn/cache` | Yarn Berry 缓存(仅条目根; 见 §4.4 限制) |
| | `.next` | Next.js 构建缓存 |
| | `.nuxt` | Nuxt 构建缓存 |
| | `.svelte-kit` | SvelteKit 构建缓存 |
| | `.turbo` | Turborepo 缓存 |
| | `.parcel-cache` | Parcel 缓存 |
| | `.eslintcache` | ESLint 缓存文件 |
| | `.stylelintcache` | Stylelint 缓存文件 |
| Python | `__pycache__` | 字节码缓存目录 |
| | `*.pyc` / `*.pyo` / `*.pyd` | 编译产物 |
| | `.pytest_cache` | pytest 缓存 |
| | `.mypy_cache` | mypy 缓存 |
| | `.ruff_cache` | Ruff 缓存 |
| | `.tox` / `.nox` | tox / nox 虚拟环境 |
| | `.hypothesis` | Hypothesis 缓存 |
| | `*.egg-info` / `.eggs` | 打包元数据 |
| | `.venv` / `venv` | 虚拟环境(`venv` 为通用名, 风险见 §4.5) |
| Java / Kotlin | `.gradle` | Gradle 项目缓存 |
| | `.kotlin` | Kotlin 编译缓存 |
| | `*.class` | 编译字节码 |
| C / C++ | `*.o` | 目标文件 |
| | `cmake-build-*` | CLion / CMake 构建目录 |
| Elixir | `_build` | Mix 编译产物 |
| Dart / Flutter | `.dart_tool` | Dart 工具缓存 |
| Swift | `.build` | SwiftPM 构建目录 |
| 通用临时文件 | `*.tmp` / `*.temp` | 临时文件 |
| | `*.swp` / `*.swo` | Vim 交换文件 |
| | `*~` | Emacs 等编辑器备份文件 |

- 表中单元格用 `/` 或 `、` 分隔的写法表示**多个独立模式**, 例如 `*.pyc` / `*.pyo` / `*.pyd` 是三条, 不是一条带斜杠的模式。
- 大多数是**不含 `/` 的 basename 模式**, 在任意深度匹配; `.yarn/cache` 是锚定条目根的路径模式。
- 所有模式都区分大小写(沿用现有语义); Windows 上 `Node_Modules` 不会被排除, 这是可接受的精度损失。

### 4.4 已知限制

1. **`.yarn/cache` 只锚定条目根**: 受语法限制(`**` 只能结尾), 无法表达「任意深度的 `.yarn/cache`」。monorepo 子目录里的 `.yarn/cache` 不排除; 需要时用自定义模式, 或未来扩展语法。
2. **`venv` 是通用词**: 确有极小概率误排普通目录; 换来的收益是常见 Python 虚拟环境。审核可决定删掉 `venv`, 保留 `.venv`。
3. **`*.o` 与 `*.class` 按扩展名**: 其他领域若有同名文件类型会被误排; 这两类扩展名在数据目录中极少见, 暂保留。
4. **Unix 的 `os-hidden` 会排除 `.git` / `.env`**: 这是现状 `include_hidden=false` 的行为, 不是本方案新增。设置页描述已明示; 代码仓库备份场景建议关闭 `os-hidden`, 或只勾选需要的顶层条目。
5. **Unix 上点开头的语言缓存受两重策略影响**: 即使关掉 `lang-temp`, 只要 `os-hidden` 还开着, `.next` / `.venv` / `.pytest_cache` 等仍会因 `.` 前缀被排除。要完全包含它们, 需同时关 `os-hidden`, 或手动勾选对应的顶层条目。Windows 上这些名字不是隐藏属性, 只受 `lang-temp` 控制。
6. **语言名单不覆盖所有生态**: 未列出的工具(如 Rust `target`、Go `vendor`、PHP `vendor`)请用自定义模式; 见 §4.5。

### 4.5 刻意不收录的通用名

以下名字**不进入**内建名单, 避免误排用户数据:

| 名字 | 原因 | 用户替代 |
|---|---|---|
| `target` | Rust 构建目录, 但也是常见业务目录名 | 高级区填 `target/**`(仅根)或 `target`(任意深度) |
| `build` / `dist` / `out` | 构建输出, 但常存放用户交付物 | 自定义 |
| `deps` / `vendor` | 依赖源码, 删掉可能破坏离线构建; 且名字通用 | 自定义 |
| `cache` | 通用缓存名, 可能是应用数据 | 自定义 |
| `*.obj` | 与 3D 模型文件冲突 | 自定义 |
| `*.pdb` | 与程序数据库 / 其他领域文件冲突 | 自定义 |
| `*.log` / `*.bak` | 可能是用户要保留的记录 / 备份 | 自定义 |
| `.idea` / `.vscode` | IDE 工程配置, 不属临时文件 | 自定义 |

原则: **内建只收「工具生成的、可再生的」且名字足够专有的项**; 其余交给按次自定义。

### 4.6 顺序、去重与校验

- 展开顺序固定: `OS_METADATA_PATTERNS` → `LANG_TEMP_PATTERNS` → 自定义模式。用户传入的 `enabledPolicies` 顺序不参与输出顺序, 保证规则可比较(`rules_eq`)。
- 去重键 = 把 `\` 归一为 `/` 后的精确字符串; 保留首次出现的原始写法。`node_modules` 与 `node_modules/**` 语义不同, 不去重。
- 所有模式在展开后统一走 `validate_patterns`; 任一非法(空串 / 空路径段 / `**` 位置错误)→ 返回错误, 扫描/压缩不开工(宁停不错)。
- 内建模式在 core 单测中逐条校验; 运行期仍统一校验一次(约 40 条, 每次扫描一次, 成本可忽略)。

### 4.7 扫描表中的呈现

- 策略命中的顶层条目仍会出现在表格中, `ScanItem.excluded` 文案沿用现状:
  - 隐藏判定 → `隐藏文件`
  - 模式命中 → `匹配排除模式: <pattern>`
- 不为策略新增 `ExcludeReason` 变体: 展开后模式已无来源信息, 且现有文案能准确说明「为什么被排除」。策略名称的完整解释由设置页描述承担。
- 被排除行默认不勾选; 用户手动勾选即可覆盖(现有机制), 包括策略命中的 `node_modules`。

### 4.8 性能护栏

- 现状默认只有 0 条用户模式; 默认开启 `lang-temp` 后, 每个文件会多出约 35~40 次 glob 匹配。
- 必做的小优化: `matches_pattern()` 在模式/路径不含 `\` 时**不做 `replace` 分配**(目前每条模式每个文件会分配两个 String)。路径由 `scan_dir` 用 `/` 构造, 内建模式也不含 `\`, 因此绝大多数调用变成纯比较。
- 基准方法: 生成 10 万文件的目录, 分别以「策略全关」与「策略全开」跑 `scan_dir`, 记录耗时; 目标额外开销 **≤ 15%**。
- 若超过阈值: 在 core 内引入一次性编译的 `PatternSet`(精确 basename 用 `HashSet`, 通配 basename 用小数组, 锚定模式单独走路径匹配), 由 `scan_dir` 每次扫描编译一次; 公共 API 不变。该优化不在本次必做范围, 但本方案的分层已为它留好位置。
- 后端 argv 预算不受影响: 约 40 条模式在 zip 下最多约 80 个 `-x` token, 远低于 `EXCLUDE_ARG_BUDGET`(24k 字符); Windows 属性路径另有 `EXCLUDE_PATH_LIMIT`(1000 条)护栏, 两者都由现有 `exclusion_args` 统一执行。

## 5. 设置数据模型与持久化

### 5.1 磁盘格式

`settings.json`(UTF-8, 2 空格缩进, 末尾换行):

```json
{
  "version": 1,
  "general": {
    "shutdownAfterCompress": false
  },
  "filters": {
    "enabledPolicies": ["os-hidden", "lang-temp"]
  }
}
```

- `version`: schema 版本, 当前 `1`
- `general.shutdownAfterCompress`: 默认 `false`; **本期 UI 置灰、不接受修改, 没有消费者**; 字段保留, 后续关机 ticket 读取。若文件中已有 `true`(未来版本写入), 本期只展示并原样保留, 不覆盖为 `false`
- `filters.enabledPolicies`: 显式空数组 `[]` = 两种策略都关; **字段缺失**才使用默认列表 `["os-hidden", "lang-temp"]`
- 未知字段读取时忽略(serde 默认行为), 下次保存时被丢弃; 这是有意的前向兼容取舍
- 未知策略 id: 读取时忽略(不参与展开), 保存时丢弃; 不报错

### 5.2 类型边界

core(无 serde 依赖):

```rust
pub const POLICY_OS_HIDDEN: &str = "os-hidden";
pub const POLICY_LANG_TEMP: &str = "lang-temp";

pub const OS_METADATA_PATTERNS: &[&str] = &[/* §4.3 */];
pub const LANG_TEMP_PATTERNS: &[&str] = &[/* §4.3 */];

/// 策略开关集合; 只存 id, 不存展开后的模式(目录更新对已有设置自动生效)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterSettings {
    pub enabled_policies: Vec<String>,
}

impl Default for FilterSettings {
    fn default() -> Self { Self { enabled_policies: default_enabled_policies() } }
}

pub fn default_enabled_policies() -> Vec<String>;
/// 去未知、去重、按目录顺序; 空输入保持空(代表全部关闭)
pub fn normalize_enabled_policies(ids: &[String]) -> Vec<String>;
pub fn is_known_policy(id: &str) -> bool;
/// 策略 + 自定义模式 → 既有 ExcludeRules; 非法模式返回可读错误
pub fn effective_rules(
    settings: &FilterSettings,
    custom: &[String],
) -> Result<ExcludeRules, String>;
```

壳层(serde DTO, 与前端字段一一对应):

```rust
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
struct FilterSettingsDto {
    #[serde(default = "default_enabled_policies")]
    enabled_policies: Vec<String>,
}

/// 唯一默认列表来源(壳层包装 core 函数, 不在两处各写一份)
fn default_enabled_policies() -> Vec<String> {
    zip_tool_core::filter::default_enabled_policies()
}

impl Default for FilterSettingsDto {
    fn default() -> Self {
        Self { enabled_policies: default_enabled_policies() }
    }
}

#[derive(Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
struct GeneralSettingsDto {
    shutdown_after_compress: bool,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
struct SettingsDto {
    general: GeneralSettingsDto,
    filters: FilterSettingsDto,
}

const SETTINGS_VERSION: u32 = 1;
fn current_version() -> u32 { SETTINGS_VERSION }

/// 磁盘结构 = SettingsDto + version; 命令 DTO 不暴露 version
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct SettingsFile {
    #[serde(default = "current_version")]
    version: u32,
    #[serde(default)]
    general: GeneralSettingsDto,
    #[serde(default)]
    filters: FilterSettingsDto,
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self { version: SETTINGS_VERSION, general: Default::default(), filters: Default::default() }
    }
}
```

- 为什么 core 不直接 derive serde: `crates/core` 目前零依赖, CLI / devcontainer 构建轻; 序列化是壳层职责。
- `FilterSettingsDto` 的 `Default` 必须调用 core 的 `default_enabled_policies()`, 保证只有一处默认列表。
- 壳层映射: `FilterSettingsDto → FilterSettings { enabled_policies: dto.enabled_policies }`; 展开与校验都在 core。

### 5.3 文件位置

用 Tauri 的 `app.path().app_config_dir()`(由 bundle identifier `com.ziptool.desktop` 决定):

| 平台 | 路径 |
|---|---|
| Windows | `%APPDATA%\com.ziptool.desktop\settings.json` |
| Linux | `$XDG_CONFIG_HOME/com.ziptool.desktop/settings.json`(通常 `~/.config/...`) |
| macOS | `~/Library/Application Support/com.ziptool.desktop/settings.json` |

- 不放在安装目录(无写权限)、不放当前工作目录(随启动方式变化)。
- 文件名固定 `settings.json`; 不提供自定义路径。
- bundle identifier 是稳定契约, **不要改**; 改了等于设置丢失。

### 5.4 读取语义

`load_from(dir) -> (SettingsFile, Vec<String>)` 是纯函数, 便于单测:

| 情况 | 行为 | 警告 |
|---|---|---|
| 文件不存在 / 父目录不存在 | 返回默认值 | 无(首次启动是正常路径) |
| JSON 语法错误 / 类型错误 | 返回默认值 | `设置文件损坏, 已使用默认值: <err>` |
| IO 错误(权限 / 是目录等) | 返回默认值 | `读取设置失败: <err>` |
| `version > 1` | 返回默认值 | `设置版本过新, 已使用默认值` |
| `version` 缺失 / `0` | 视为 1, 正常解析 | 无 |
| 字段缺失 | 该字段用默认值 | 无 |
| 未知策略 id | 忽略该 id | 无(可调试日志) |

- **不因为读取失败而阻止启动**: 默认值一定可用; 设置页照常可打开, 用户改动后会尝试覆盖写。
- 读取只在启动时发生一次; 外部编辑文件不热加载(非目标)。运行期以内存为准。

### 5.5 保存语义

`save_to(dir, &SettingsFile) -> io::Result<()>`:

1. `create_dir_all(dir)`
2. 归一化: `version = 1`; `enabledPolicies = normalize(...)`(去未知、去重、目录顺序)
3. `serde_json::to_vec_pretty` + 末尾换行
4. 写 `settings.json.tmp` → `rename` 到 `settings.json`
   - Unix: `rename` 原子替换
   - Windows: `rename` 覆盖已存在文件会失败; 先 `remove_file` 再 `rename`(极小的「暂无文件」窗口, 崩溃最坏退回默认值, 可接受)
5. 失败向 UI 返回 `Err(String)`; UI 回滚开关并提示

- 不做 fsync: 文件极小, 掉电最坏丢最后一次 toggle, 安全方向是默认值。
- 不做备份轮转 / 不保留 `.bak`(非目标); 损坏文件在下次成功保存时被覆盖。

### 5.6 命令

```rust
#[tauri::command]
async fn load_settings(app: AppHandle) -> Result<SettingsLoadResult, String>;

#[tauri::command]
async fn save_settings(app: AppHandle, settings: SettingsDto) -> Result<SettingsDto, String>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsLoadResult {
    settings: SettingsDto,
    warnings: Vec<String>,
}
```

- 两者都用 `tauri::async_runtime::spawn_blocking` 做磁盘 IO, 避免卡 UI。
- `save_settings` 返回**归一化后的 canonical DTO**, UI 以此为准重绘(例如未知 id 被丢弃、顺序被规范化)。
- 命令面注册进 `invoke_handler`; 自定义命令不需要改 `capabilities/default.json`(不用插件)。
- `SettingsDto` 增加字段时保持 `#[serde(default)]`, 旧前端 / 旧文件都能读。

### 5.7 默认值与迁移

- 默认: `shutdownAfterCompress=false`; `enabledPolicies=["os-hidden","lang-temp"]`(D-1 / D-2 已确认)。
- UI 侧 `DEFAULT_SETTINGS` 只用于加载失败 / 浏览器预览兜底; 单测断言其 `enabledPolicies` 与 core 默认一致, 避免两处漂移。
- 未来新增默认开启的策略时, 需要一次 schema 迁移(把新 id 合并进已有 `enabledPolicies`); 在 `version` 升级分支里做, 不在读取时隐式追加。
- 未来新增默认关闭的策略: 不需要迁移, 用户自行打开。
- 版本降级(旧应用读新文件): 版本过新 → 默认值 + 警告; 不尝试部分解析, 避免误读语义。

### 5.8 安全边界

- 路径固定, 不接收前端传入路径; JSON 只解析为固定结构, 不做任意文件访问。
- 设置值只影响排除名单; `shutdownAfterCompress` 本期置灰、不接受 UI 修改, 只在保存其它设置时原样带回。
- 策略 id 经白名单归一化, 不会拼进 shell 命令。
- 设置文件不含密码 / 密钥; 没有敏感信息。
- 本期没有关机命令 / 平台调用; 该字段只是布尔值, 不构成任何执行面。

### 5.9 浏览器预览(localStorage)

- 无 `window.__TAURI__` 时, 用 `localStorage["ziptool.settings"]` 存同一 DTO 的 JSON; 读取失败 / 损坏 → 默认值。
- 这条路径只服务界面预览与 Node 单测; 真实应用始终走 `load_settings` / `save_settings`。
- **语言偏好仍留在 `localStorage["ziptool.locale"]`**: 它是 WebView 关注点, 迁移会牵动现有 i18n 测试与启动时序; 本次不动。两处存储并存是已知的小不一致, 记录在 §13。

## 6. 前后端协议变更

### 6.1 `scan`

```text
scan(mode, path, filters?, excludes?, on_progress) -> ScanResult
```

- `filters?: { enabledPolicies: string[] }`: 仅压缩模式有意义; 缺省 = 默认策略(见 §4.1)
- `excludes?: string[]`: 高级区自定义模式, 语义不变
- 解压模式不使用 `filters`(解压不做排除); 现有 `excludes` 在解压模式本来也不使用
- 壳层: `effective_rules(filters, excludes)` → `ExcludeRules`; 校验失败直接 `Err`, 不占忙、不扫描
- 扫描结果 `ScanContext.rules` 保存**展开后的** `ExcludeRules`

**为什么由 UI 传 `filters`, 而不是壳层每次读 `settings.json`**:

- 请求自包含: 本次扫描 / 压缩用了哪套策略由参数明确, `ScanContext` 的规则比对不依赖「命令执行时文件恰好是什么」。
- 避免命令隐式依赖可变全局文件; 也避免一次扫描过程中文件被外部改动导致前后不一致。
- UI 在启动时已经通过 `load_settings` 持有同一份状态; 保存失败会回滚, 因此 UI 发送的就是用户看到的。
- 代价: UI 必须 `await settingsReady` 后才能扫描(§7.5); 壳层仍做策略 id 白名单与模式校验, 不信任前端。

示例:

```json
{
  "mode": "compress",
  "path": "D:\\data",
  "filters": { "enabledPolicies": ["os-hidden", "lang-temp"] },
  "excludes": ["*.bak"]
}
```

### 6.2 `compress`

`options` 增加 `filters`:

```json
{
  "splitSize": "2g",
  "password": null,
  "randomName": false,
  "format": "zip",
  "level": null,
  "filters": { "enabledPolicies": ["os-hidden", "lang-temp"] },
  "excludes": [],
  "verify": true
}
```

- `filters` 为 `Option`, 缺省 = 默认策略; 旧调用方不传也能工作
- 壳层用同一 `effective_rules()` 生成规则, 再与 `ScanContext.rules` 做 `rules_eq`
- 规则不一致 → 沿用现有错误「扫描结果已过期, 请重新选择源文件夹」; UI 侧应先自动重扫(§7.6)

### 6.3 保鲜与规则等价

- `rules_eq` 继续比较 `include_hidden` + `patterns` 逐条相等:
  - 切换 `os-hidden` → `include_hidden` 变 → 过期
  - 切换 `lang-temp` → patterns 变 → 过期
  - 策略 id 顺序不同但归一化后集合相同 → 规则相同 → 不过期(合法)
- 壳层不直接比较 `filters` DTO, 只比较展开后的规则; 这样未来策略实现变化不影响保鲜语义。
- 自定义模式顺序参与比较(现有语义), UI 透传顺序即扫描顺序。

### 6.4 与现状差异

| 项 | 现状 | 本方案 |
|---|---|---|
| `scan` 参数 | `excludes?` | `filters?` + `excludes?` |
| `compress.options` | 无 `filters` | 增加 `filters?` |
| UI 的 `include_hidden` | 恒定 `false` | 由 `os-hidden` 策略决定 |
| 规则来源 | 仅自定义模式 + 内建隐藏 | 策略展开 + 自定义模式 |
| `ScanContext.rules` | 已存在 | 内容改为展开后规则, 比较逻辑不变 |
| Rust DTO 测试 | `CompressOptions` 构造无 `filters` | 所有构造点补 `filters: None` 或默认值 |

### 6.5 CLI 不变

- `ziptool compress` 仍只有 `--exclude` / `--include-hidden-files`; 不读 `settings.json`。
- `crates/core` 的 `Options::default()` 保持 `include_hidden=false`、无用户模式; 新增 `filter` 模块不影响现有调用。
- 后续可加 `--filter-policy <id>`(复用 `effective_rules`), 不在本次范围。

## 7. UI 设计

### 7.1 结构线框

```text
┌────────────────────────────────────────────────────────────┐
│ [← 返回]  设置                                              │  settings header
├──────────────┬─────────────────────────────────────────────┤
│  通用         │  通用                                       │
│  文件排除     │  ☐ 压缩完成后关机   [即将支持]              │
│              │    该功能将在后续版本提供, 当前不可用…       │
│              │                                             │
│              │  文件排除                                    │
│              │  策略在每次扫描时生效; 修改后会重新扫描…      │
│              │  ┌───────────────────────────────────────┐  │
│              │  │ 操作系统隐藏文件              [ ON  ] │  │
│              │  │ 排除操作系统标记为隐藏的文件与目录…    │  │
│              │  ├───────────────────────────────────────┤  │
│              │  │ 编程语言的临时文件            [ ON  ] │  │
│              │  │ 排除编程语言与构建工具生成的缓存…      │  │
│              │  └───────────────────────────────────────┘  │
├──────────────┴─────────────────────────────────────────────┤
│ 状态栏 …                                        语言  v0.1.2 │  footer(共用)
└────────────────────────────────────────────────────────────┘
```

- 主界面 header 右侧新增齿轮按钮(`#settings-open`), 位于「确认」左边
- 设置页有自己的 header(返回 + 标题); 主界面 header 隐藏
- **footer 共用**: 状态消息、语言菜单、版本号在设置页仍可用
- 本期**没有**关机倒计时横幅 / 全局电源提示条; 「压缩完成后关机」只是通用区里的一个勾选框(§8)

### 7.2 视图切换

- DOM 外层:

  ```html
  <body>
    <div id="view-main" class="flex flex-col flex-1 min-h-0">…</div>
    <div id="view-settings" class="hidden flex flex-col flex-1 min-h-0">…</div>
    <footer>…</footer>
  </body>
  ```

- `showView("main" | "settings")`: 切换两个容器的 `hidden` 类; 主界面 DOM 不销毁, 扫描表 / 勾选 / 滚动位置自然保留
- 打开: 默认「通用」; 记录上一次焦点, 关闭时还给齿轮按钮
- 关闭: 返回按钮 / `Escape`; 若策略确实变化且当前是压缩模式并已有扫描结果 → 自动重扫(§7.6)
- 运行 / 扫描中齿轮禁用(加入 `LOCK_IDS`), 与现有锁定规则一致; 因此设置页不会遮住「取消」按钮
- 拖拽仅在主视图生效: `zoneAt()` / `handleDrop()` 在 `state.view !== "main"` 时直接忽略, 打开设置时清掉拖拽高亮

### 7.3 边栏

- `<nav data-i18n-aria-label="settings.nav.label">`, 两个 `<button type="button" data-section="general|filters">`
- 宽度 `w-40`(160px), `shrink-0`, 右边框, 浅底; 内容区 `flex-1 min-w-0 overflow-auto p-6`
- 选中项: `aria-current="page"` + 浅蓝/浅灰底 + 左边框强调; 未选中 hover 浅底
- 内容区两个 `<section data-panel="general|filters" role="region" aria-labelledby="…">`; 非活动 panel 加 `hidden`
- 侧栏选中态是**会话内状态**, 不持久化; 重新打开应用默认「通用」

### 7.4 列表项与开关

每项(语义为 `ul > li`):

```html
<li class="flex items-start justify-between gap-4 px-4 py-4">
  <div class="min-w-0">
    <div id="policy-os-hidden-title"
         data-i18n="settings.filters.osHidden.title"
         class="text-sm font-medium text-slate-700">操作系统隐藏文件</div>
    <p id="policy-os-hidden-desc"
       data-i18n="settings.filters.osHidden.desc"
       class="mt-1 text-xs leading-relaxed text-slate-500">…</p>
  </div>
  <label class="relative inline-flex shrink-0 cursor-pointer items-center">
    <input id="policy-os-hidden" data-policy="os-hidden" type="checkbox" role="switch"
           aria-labelledby="policy-os-hidden-title"
           aria-describedby="policy-os-hidden-desc"
           class="peer sr-only" />
    <span class="h-5 w-9 rounded-full bg-slate-300 transition-colors
                 peer-checked:bg-blue-600 peer-focus-visible:ring-2
                 peer-focus-visible:ring-blue-400"></span>
    <span class="absolute left-0.5 top-0.5 h-4 w-4 rounded-full bg-white shadow
                 transition-transform peer-checked:translate-x-4"></span>
  </label>
</li>
```

- 列表容器: `ul` 外套 `rounded-lg border border-slate-200`, 行间 `divide-y divide-slate-100`
- 标题 `text-sm font-medium`, 描述 `text-xs text-slate-500`, 描述换行完整展示(不截断、不加省略号)
- 只有开关可交互; 行 hover 可浅底提示, 但不整行绑定 toggle(避免误触)
- 开关是**原生 checkbox + `role="switch"`**: Space / 点击切换, `checked` 即无障碍状态; 不额外手写 `aria-checked`
- 保存中: `disabled` + `opacity-50`, 内容区 `aria-busy="true"`; 防止并发写
- 通用区的「压缩完成后关机」按需求用**普通勾选框**(`accent-blue-600`), 而不是 switch; 右侧不放控件:

  ```html
  <label class="flex items-start gap-2.5 cursor-default">
    <input id="set-shutdown" type="checkbox" disabled
           class="mt-0.5 accent-blue-600 disabled:cursor-not-allowed"
           aria-describedby="set-shutdown-desc" />
    <span>
      <span data-i18n="settings.general.shutdown.title"
            class="block text-sm text-slate-400"></span>
      <span id="set-shutdown-desc"
            data-i18n="settings.general.shutdown.desc"
            class="mt-1 block text-xs text-slate-400"></span>
    </span>
  </label>
  ```

  - `disabled` 是本期 D-12 B 的核心: 控件不可聚焦 / 不可切换, 不会触发 `change`
  - 视觉上用 `text-slate-400` + 原生 disabled 态表达「不可用」; 描述含「即将支持」
  - 若 `load_settings` 返回 `shutdownAfterCompress=true`(未来版本写入), 控件仍 disabled, 但 `checked=true` 展示, 保存其它设置时原样带回该值

### 7.5 加载 / 保存状态机

```text
boot ── load_settings ──▶ ready
                          │
            打开设置 ◀────┘
              │
              ▼
        open(section) ──toggle──▶ saving ──ok──▶ open(重绘, 静默)
              ▲                     │
              └──── err: 回滚 ◀──────┘  + 状态栏错误
              │
        back / Escape ──▶ closed ──(策略变化?)──▶ 自动重扫
```

- **启动即加载**: 主界面不显示策略, 但压缩扫描需要它; `doScan()` / 压缩确认前 `await settingsReady`, 避免「先用默认值扫了再加载」的竞态
- 加载中齿轮禁用; 加载失败仍进入 ready(默认值), 状态栏提示
- 即时保存: 每次 change 立即 `save_settings`; 成功不弹提示(开关状态即反馈), 失败回滚并提示
- 保存串行: 保存中禁用全部设置控件, 天然避免并发写; **关闭设置页时若保存仍在飞行, 先 `await` 保存完成再决定是否重扫**(失败则回滚, 不重扫), 避免「用未落盘的策略扫了表」
- 保存失败回滚后重新计算 `filtersDirty`(相对 `scanFilters`), 不让失败的回滚触发多余重扫
- `load_settings` 返回 `warnings` 时, 状态栏显示第一条(文件损坏 / 版本过新)

### 7.6 策略变更后的失效与重扫

- 扫描成功时记录 `state.scanFilters = 当前 enabledPolicies 快照`(排序后比较)
- 设置保存成功后:
  - `state.filtersDirty = !samePolicies(当前策略, state.scanFilters)`
  - 如果当前是**解压模式**, 不标记(排除策略不影响解压)
  - 如果还没有扫描结果 / 没有源路径, 只记录, 不重扫
- 关闭设置页时:
  - `state.mode === "compress"` 且 `state.rows.length > 0` 且 `$("src").value` 且 `filtersDirty`
    → `setStatus("status.settingsRescan")` + `doScan(src)`
  - 否则只清 `filtersDirty`
- 重扫会清空表格与勾选(现有 `doScan` 行为); 这是策略变化的必然结果, 设置页描述已提示
- 壳层 `ScanContext` 的规则比对仍是**最终兜底**: 即使 UI 漏了重扫, 压缩也会因规则过期被拒绝, 不会用错规则执行

### 7.7 锁定、键盘与可访问性

- `LOCK_IDS` 增加 `settings-open`: 扫描 / 压缩 / 解压期间禁用; 设置页内的控件不在 `LOCK_IDS`(页面打开时不可能有任务)
- 打开设置: 焦点到当前侧栏项; 关闭: 焦点回 `#settings-open`
- `Escape` 优先级: 语言菜单 > 设置页; 语言菜单打开时先关菜单, 否则关闭设置页
- 侧栏按钮 `aria-current="page"`; 内容 section `role="region"` + `aria-labelledby` 指向标题
- 开关 `aria-labelledby` 指标题、`aria-describedby` 指描述; 描述是真实 DOM 文本, 读屏可读
- 状态栏建议补 `aria-live="polite"`(可选; 不影响现有测试)
- 窄窗口: 侧栏固定 160px, 内容区 `min-w-0`; 描述换行; 不针对手机宽度做适配

### 7.8 浏览器预览

- 无 `__TAURI__` 时: 设置读写 localStorage(§5.9); 没有关机命令 / 倒计时逻辑
- 预览路径同样走 `normalizeSettings()` 与 `settingsReady`, 保证 UI 测试与真实环境行为一致

### 7.9 样式构建

- `tailwind/input.css` 增加 `@source "../ui/app.js";`: 设置项的部分类名在 JS 里(或由 JS 切换), 避免漏生成
- 开关的视觉状态用 `peer-checked:` 静态类, 不靠 JS 拼类名; 这样类名一定出现在 `index.html` 中
- 新增类名后执行 `cd crates/tauri-app && npm run css`, 提交生成的 `ui/app.css`
- 不新增前端依赖; 不修改 CSP

## 8. 压缩完成后关机(本期仅 UI)

### 8.1 本期范围

- 只做「通用」区的一个**置灰勾选框**:
  - 标题: 压缩完成后关机; 描述含「即将支持」
  - 默认: 未勾选; 控件 `disabled`, 不可聚焦 / 不可切换
  - 不响应输入: 点击 / 键盘都不会触发 `change`, 也不会调用 `save_settings`
  - 若 `load_settings` 返回 `shutdownAfterCompress=true`(未来版本写入), 展示为 disabled + checked; 用户改其它设置时原样带回 `true`, 不静默改成 `false`
- **不做**: 压缩完成后的实际关机、倒计时、取消关机横幅、`power_off` 命令、Windows / Linux / macOS 平台命令、dry-run、电源相关状态栏消息
- 设置字段与默认值已经定型, 后续 ticket 不需要改 schema; 本期只是禁用入口

### 8.2 UI 契约

| 元素 | 规格 |
|---|---|
| 控件 | 原生 checkbox(`#set-shutdown`), 位于通用区; **`disabled`**; 不是 switch(按需求) |
| 文案 | `settings.general.shutdown.title` / `.desc`; 描述含「即将支持 / 该功能将在后续版本提供」 |
| 状态 | 来自 `state.settings.general.shutdownAfterCompress`; 默认 `false`; 为 `true` 时展示 `checked`, 但仍 disabled |
| 保存 | 本控件不触发保存; 其它设置保存时把读到的值原样写回, 不丢失未来版本的 `true` |
| 无副作用 | 不调用任何电源命令; 不创建定时器; 不监听压缩完成事件 |
| 可访问性 | label 包裹 checkbox; `aria-describedby` 指向描述; disabled 由原生语义表达, 不需要 `aria-disabled` |

- 这是 D-12 B 的明确取舍: **宁可入口不可用, 也不让用户误以为勾选后会关机**。
- 后续 ticket 打开入口时, 只需移除 `disabled`、接上 `change → save_settings` 与压缩成功后的行为, 不改变本期 DOM 结构。

### 8.3 后续 ticket 的衔接约定(不在本期实现)

- 字段: `general.shutdownAfterCompress`(boolean, 默认 false), 已在 `settings.json`
- 触发: 仅当本批压缩成功完成(非取消、非错误、至少一个产物); 解压 / 取消 / 失败 / 零产物不触发
- 行为: 可取消倒计时 + 平台关机命令 + 失败提示; 具体时长 / 平台命令 / dry-run 由该 ticket 定义
- 开通入口: 移除控件的 `disabled`, 恢复 `change` 时写 `save_settings`; 校验 / 回滚沿用其它设置
- 本期不预留 `power_off` 命令、不预留前端定时器 / 横幅 DOM; 后续 ticket 自行新增, 避免死代码
- 设置 schema 不需要迁移; 只新增消费方

### 8.4 验收

- [ ] 勾选框可见、`disabled`、默认未勾选; 描述明确「即将支持 / 后续版本提供」
- [ ] 点击 / 键盘操作不能切换, 不调用 `save_settings`
- [ ] `load_settings` 返回 `true` 时展示 disabled + checked; 修改其它设置保存后仍为 `true`
- [ ] 即使字段为 `true`, 执行压缩成功也不发生关机 / 电源命令 / 倒计时 / 横幅
- [ ] 代码中不存在 `power_off` 命令、平台关机命令与相关状态栏 key

## 9. i18n

### 9.1 新增 key(zh-CN 基线)

| Key | 简体中文 |
|---|---|
| `settings.title` | 设置 |
| `settings.open` | 设置(齿轮按钮 aria-label / title) |
| `settings.back` | 返回 |
| `settings.nav.label` | 设置导航 |
| `settings.nav.general` | 通用 |
| `settings.nav.filters` | 文件排除 |
| `settings.general.heading` | 通用 |
| `settings.general.shutdown.title` | 压缩完成后关机 |
| `settings.general.shutdown.desc` | 即将支持: 压缩成功后自动关机的能力将在后续版本提供。 |
| `settings.filters.heading` | 文件排除 |
| `settings.filters.intro` | 策略在压缩扫描时生效; 修改后会在关闭设置页时重新扫描当前源文件夹。被排除的条目仍会显示, 但默认不勾选。单次任务仍可在主界面的「高级」中添加自定义模式。 |
| `settings.filters.osHidden.title` | 操作系统隐藏文件 |
| `settings.filters.osHidden.desc` | 排除操作系统标记为隐藏的文件与目录: Windows 为隐藏属性, Linux/macOS 为以 . 开头的名字(含 .git、.env), 以及 Thumbs.db、desktop.ini 等系统元数据。 |
| `settings.filters.langTemp.title` | 编程语言的临时文件 |
| `settings.filters.langTemp.desc` | 排除编程语言与构建工具生成的缓存和临时文件, 例如 JavaScript 的 node_modules、Python 的 __pycache__、Java 的 .gradle 与 *.class。 |
| `status.settingsLoadFailed` | 读取设置失败, 已使用默认值: {{error}} |
| `status.settingsLoadWarning` | 读取设置时有问题: {{error}} |
| `status.settingsSaveFailed` | 保存设置失败, 已还原: {{error}} |
| `status.settingsRescan` | 排除策略已更新, 正在重新扫描… |

- 本期不新增 `settings.shutdown.*` / `status.shutdown*`: 关机行为由后续 ticket 负责, 避免无消费方的文案。
- 政策描述中的标识符(`node_modules`、`__pycache__`、`.gradle`、`*.class`)保持原样, 不翻译。
- 五种语言都需补齐; `i18n.test.mjs` 的 base key 集合测试会强制这一点。翻译建议由实现方起草后请母语者审校(与 i18n.md 的交付流程一致)。

### 9.2 动态渲染

- 静态文案(标题 / 描述 / 按钮)用 `data-i18n` / `data-i18n-title` / `data-i18n-aria-label`, 由 `i18n.applyDom()` 统一处理; 切换语言自动更新。
- 动态内容(状态栏消息)用 `t()`; 现有 `localechange` 监听里的 `renderStatus()` 已覆盖。
- 设置页不重建 DOM, 语言切换不丢焦点; 本期没有倒计时 / 横幅需要动态重绘。

## 10. 测试计划

### 10.1 core 单测(`crates/core/src/filter.rs`)

- 目录: 恰好两个已知 id; `normalize_enabled_policies` 去未知、去重、按目录顺序; 空数组保持空。
- 每条内建模式通过 `validate_patterns`; `effective_rules` 输出通过校验。
- 展开矩阵:
  - 两策略开 → `include_hidden=false` + OS 元数据 + 语言模式
  - 仅 `os-hidden` → 只含 OS 元数据
  - 仅 `lang-temp` → `include_hidden=true` + 语言模式(验证 `.venv` 等 dot 目录仍被排)
  - 全关 → `include_hidden=true` + 仅自定义
- 自定义模式追加、与内建重复时只保留一份、顺序为「OS → 语言 → 自定义」。
- 安全回归:
  - 内建语言模式不匹配 `.git`、`.git/config`、`data`、`src`、`target`、`build`、`dist`、`vendor`
  - `os-hidden` 的 Unix 语义仍排除 `.git`(现有行为, 用条件编译测试)
- 非法自定义模式 → `Err`; 内建清单不会被误改成非法。
- `matches_pattern` 无 `\` 时的零分配改造不改变现有匹配矩阵(现有测试全绿)。

### 10.2 壳层单测(`crates/tauri-app/src/`)

- `settings.rs`:
  - 文件不存在 → 默认值, 无警告
  - 损坏 JSON / 类型错误 → 默认值 + 警告
  - `version: 2` → 默认值 + 警告; 缺失 version → 正常解析
  - 缺字段 → 该字段默认; `enabledPolicies: []` → 保持空
  - 未知 id → 丢弃; 重复 id → 去重
  - `save_to` → `load_from` 往返一致; 目录自动创建; 覆盖已有文件(Windows 分支)
- `lib.rs`:
  - `FilterSettingsDto` 默认 = core 默认; `filters: None` = 默认
  - `scan` / `compress` 的 `effective_rules` 与 `ScanContext.rules` 一致
  - 策略切换后 `select_compress` 返回「扫描已过期」; 策略不变/顺序不同(归一化后相同)不误报
  - `to_core_options` 同时处理 `filters` 与 `excludes`
- 本期不新增 `power.rs` / 电源命令测试: 关机行为不在本期范围。

### 10.3 UI 单测(`test/ui.test.mjs` 末尾新增「设置页」用例)

- 启动: `load_settings` 调用一次; 齿轮在加载完成前禁用; 加载失败 → 默认值 + 状态栏。
- 打开 / 关闭:
  - 齿轮 → `#view-settings` 显示、`#view-main` 隐藏
  - 侧栏两项; 默认「通用」; 点击「文件排除」切 panel; `aria-current` 同步
  - 返回 / Escape 关闭; 焦点回齿轮
- 通用:
  - 勾选 → `save_settings` 参数正确; 取消勾选同理
  - 保存失败 → 勾选回滚 + `status.settingsSaveFailed`
- 策略列表:
  - 两项标题 / 描述 / 开关; 默认都开
  - 切换 → `save_settings.enabledPolicies` 正确(目录顺序)
  - 未知 id 从 `load_settings` 返回 → UI 不显示 / 不回传
  - 保存中控件禁用; 保存失败回滚
- 失效 / 重扫:
  - 压缩模式已有结果, 策略改变后关闭设置 → `scan` 用新 `filters` 重扫
  - 策略改回原值 → 不重扫
  - 解压模式下改策略 → 不重扫, `scan` 参数不要求 `filters`
  - 无源路径 / 无结果 → 不重扫
- 关机控件(仅 UI, D-12 B):
  - 控件存在且 `disabled`; 默认未勾选; 描述含「即将支持」
  - 点击 / 键盘不能切换, 不产生 `save_settings` 调用
  - `load_settings` 返回 `true` → disabled + checked; 修改一个策略保存后, 请求体仍带 `shutdownAfterCompress=true`
  - 字段为 `true` 时执行压缩成功 → 不出现横幅, 不调用任何 `power_off` / 电源命令, 不创建定时器
- i18n:
  - 英文界面下设置页文案来自 en; 切换语言不丢当前 section / 开关状态
  - 关机控件描述明确含「即将支持 / 后续版本提供」语义
- 预览模式: localStorage 往返; 无电源相关调用。

### 10.4 现有测试的连带修改

- `test/ui.test.mjs`:
  - `scan` 参数断言补 `filters` 形状
  - `compress.options` 的 `jsonEq` 补 `filters`
  - `makeHarness` 的默认 invoke 处理 `load_settings` / `save_settings`(返回默认设置 / echo), 否则启动时的设置加载会拿到 `null`
  - 新增 `settings-open` 进入 `LOCK_IDS` 后, 检查现有「任务期锁控件」用例是否仍只断言 `src`(不枚举 LOCK_IDS 就不受影响)
- `test/i18n.test.mjs`: 五份词条补齐 §9.1 的 base key; 与现有复数 / 占位符规则一致。
- `package.json` 的 `test` 脚本无需新增文件: 设置页用例并入现有 `test/ui.test.mjs`。
- 壳层 Rust 测试: 所有 `CompressOptions { … }` 构造点补 `filters: None`(已同步)。

### 10.5 实机验收

- 准备夹具: 隐藏属性文件(Windows 用 `attrib +H`)、`.DS_Store`、`Thumbs.db`、`node_modules/`、`__pycache__/`、`.git/`、普通数据文件。
- 策略全开扫描: 顶层被排除项显示且默认不勾选; 目录大小扣除; `.git` 在 Windows 保留、在 Linux/macOS 被 `os-hidden` 排除(符合现有口径)。
- 逐项关闭策略: 重新扫描后对应条目恢复可选; 压缩后用 `7z l` / `unzip -l` 确认包内容与扫描口径一致。
- 设置持久化: 改开关 → 退出 → 重开, 状态保持; 手工写坏 `settings.json` → 启动默认值 + 警告。
- 关机控件: 置灰不可点击; 描述明确「即将支持」; 手工把 `settings.json` 的字段改为 `true` 后启动, 控件显示 disabled + checked, 修改策略保存后文件仍为 `true`; 执行压缩成功不发生关机 / 电源命令。
- 五语言走查: 设置页标题 / 描述 / 开关; 长描述换行不溢出。

### 10.6 回归

- `cargo test --workspace`
- `cargo test -p zip-tool-e2e`(CLI 行为不变)
- `cargo clippy --workspace --all-targets -- -D warnings` / `cargo fmt --all --check`
- `cd crates/tauri-app && npm test`
- `npm run css` 后确认 `ui/app.css` 含新增类名(开关、侧栏、设置页布局)
- 性能: §4.8 的 10 万文件基准, 记录数字

## 11. 实施步骤

| 阶段 | 内容 | 主要文件 | 完成标准 |
|---|---|---|---|
| P0 | core 策略目录 + 展开 + 校验 + 模式零分配 fast path | `crates/core/src/filter.rs`(新)、`exclude.rs`、`lib.rs` | §10.1 全绿; 内建清单与 §4.3 一致 |
| P1 | 设置持久化 + DTO + `load_settings` / `save_settings` | `src/settings.rs`(新)、`src/lib.rs`、`Cargo.toml`(`serde_json`) | 读写往返 / 损坏回退 / 版本分支测试通过 |
| P2 | 协议接入: `scan` / `compress` 的 `filters` + 保鲜 | `src/lib.rs` | 规则不一致 → 过期; 旧调用缺省可用 |
| P3 | 设置页静态结构与视图切换 + 加载 / 保存 + 回滚 | `ui/index.html`、`ui/app.js`、`tailwind/input.css` | 打开 / 侧栏 / 开关 / 持久化可用; CSS 已重生成 |
| P4 | 策略变更失效与自动重扫 | `ui/app.js`、`src/lib.rs` 测试 | §7.6 行为 + 测试通过 |
| P5 | 「压缩完成后关机」占位控件(置灰) + 字段保留 | `ui/index.html`、`ui/app.js`、`src/settings.rs` | §8 UI 契约 + 验收通过; 无 `power_off` / 电源命令 |
| P6 | i18n 五语言 + 词条完整性 + 文档同步 | `ui/locales/*.js`、`test/i18n.test.mjs`、README、`_docs/*` | 五语言完整; `npm test` 全绿; 文档更新清单完成 |

- 每个阶段独立可编译、可测试; P3 之前可以先合 P0~P2(后端能力)不影响现有 UI。
- P5 只依赖 P1(设置字段)与 P3(设置页); 不依赖任何压缩完成逻辑, 可与 P4 并行。
- 不采用「大爆炸」一次提交; 按阶段提交, 每阶段带测试。

## 12. 验收标准

- [ ] 主界面右上角有设置入口; 打开后为「边栏 + 内容区」, 边栏含「通用」「文件排除」
- [ ] 通用区有「压缩完成后关机」勾选框, **置灰不可交互**, 描述含「即将支持」, 且不产生任何实际关机副作用
- [ ] 文件排除区以列表展示两项策略: 标题 + 描述 + Switch; 默认状态见 §4.1
- [ ] 切换策略立即持久化; 写盘失败时开关回滚并提示, 不出现「显示开、实际关」
- [ ] 策略变化后, 压缩模式已有扫描结果在关闭设置页时自动重扫; 解压模式不重扫
- [ ] 修改策略后不重扫直接点压缩 → 壳层拒绝并提示扫描过期(兜底)
- [ ] 关闭 `os-hidden` 后隐藏文件可被压缩; 关闭 `lang-temp` 后 `node_modules` 等恢复可选
- [ ] 关机控件不可切换; 即使 `settings.json` 中字段为 `true`, 执行压缩成功也不发生关机 / 无倒计时横幅 / 无电源命令与日志
- [ ] 代码中不存在 `power_off` 命令、平台关机命令、`status.shutdown*`; 只有通用区 `settings.general.shutdown.*` 文案, 没有单独的 `settings.shutdown.*`
- [ ] 设置文件损坏 / 版本过新时应用正常启动并使用默认值, 状态栏提示
- [ ] 五种语言下设置页、策略描述、关机占位文案完整; 切换语言即时生效
- [ ] `cargo test --workspace`、`cargo test -p zip-tool-e2e`、`npm test`、clippy / fmt 全绿
- [ ] CLI 行为与输出不变; 现有 `_docs/exclude-rules.md` 的语义不变
- [ ] 扫描性能基准额外开销 ≤ 15%(策略全开 vs 全关, 10 万文件)

## 13. 风险与缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| 内建名单误排用户数据 | 压缩包缺文件 | 保守名单(§4.5); 被排除顶层项可见可勾选; 策略可关; 单测锁定 `.git` 等关键名不被语言模式命中 |
| Unix `os-hidden` 排除 `.git` / `.env` | 代码仓库备份缺历史 / 配置 | 现状行为; 描述明示; 建议代码备份关闭该策略; 自定义模式无法「加回」隐藏文件, 只能关策略或勾选顶层项 |
| 用户不知道 `lang-temp` 默认开 | 产物比预期小 | 设置页描述 + 扫描表排除行; README 标明默认值 |
| 策略切换后忘记重扫 | 用旧规则? | UI 自动重扫 + 壳层规则过期双保险 |
| 设置文件损坏 / 权限不足 | 设置丢失 | 默认值 + 警告; 保存失败回滚; 文件路径固定 |
| UI-only 关机控件误导用户 | 用户以为勾选后会关机 | 采用 D-12 B: 控件 disabled + 描述「即将支持」; 即使字段为 true 也不产生行为 |
| 扫描变慢 | 大盘体验退化 | 零分配 fast path; 基准门槛; 超阈值上 `PatternSet` |
| 未知策略 id(降级) | 新策略被丢弃 | 安全方向: 丢弃排除 = 多压缩, 不缺文件; 文档说明 |
| 双存储(locale / settings) | 认知不一致 | 已知取舍; 语言属 WebView 关注点, 本次不迁移 |

## 14. 文档同步

| 文档 | 修改 |
|---|---|
| `_docs/filtering_rules.md` | 本文档(目标规格) |
| `_docs/exclude-rules.md` | §1 目标补充「策略层」; §2 说明 `ExcludeRules` 可由 `filter::effective_rules` 生成; 指向本文档 |
| `_docs/ui-backend.md` | §3 命令面加 `load_settings` / `save_settings`(**不加 `power_off`**); §3.1/3.2 加 `filters`; §4 DTO; §8 选项控件加设置页入口; §11 测试 |
| `_docs/i18n.md` | §6 key 清单补 `settings.*` / `status.settings*`; **不新增** `status.shutdown*`; 说明设置页动态文案 |
| `README.md` | 功能列表加设置页 / 排除策略; 设置文件位置; 策略默认值与关闭方式; 「压缩完成后关机」标注为 UI 占位 / 后续 ticket 实现 |
| `_docs/compress.md` | 不改压缩语义; 可注明关机不在本期范围 |

## 15. 已确认与待定决策

### 15.1 已确认(2026-10-20)

| # | 决策点 | 结论 |
|---|---|---|
| D-1 | `lang-temp` 默认 | **开** |
| D-2 | `os-hidden` 默认 | **开** |
| D-3 | 设置持久化 | **`app_config_dir()/settings.json`** + `load_settings` / `save_settings` |
| D-4 | 「压缩完成后关机」范围 | **本期仅 UI 占位**, 不实现倒计时 / `power_off` / 平台命令, 不接受修改; 字段保留, 实际行为由后续 ticket 负责 |
| D-6 | 内建清单(§4.3) | **按当前表执行**, 可直接增删行 |
| D-11 | OS 元数据模式 | **并入 `os-hidden`**, 保持两项策略 |
| D-12 | UI-only 关机控件呈现 | **B: disabled + 「即将支持」**; 字段若为 `true` 只展示并原样保留 |

### 15.2 其余项(未反对则按建议)

| # | 决策点 | 建议 | 备选 / 影响 |
|---|---|---|---|
| D-5 | 策略变更后是否自动重扫 | **是(仅压缩模式)** | 只标记 = 少一次扫描, 但用户点压缩才发现过期 |
| D-7 | 是否保留高级区自定义模式 | **保留, 并集** | 移入设置页 = 更大的 UI 与持久化改造, 非本次需求 |
| D-8 | 设置入口位置 | **主界面 header 齿轮** | 状态栏 = 不够显眼; 菜单 = 多一层交互 |
| D-9 | 未知策略 id | **忽略并丢弃** | 报错 = 降级时无法启动; 保留 = 旧应用无法理解 |

> D-10(关机是否受告警影响)不再属于本期: 实际关机行为由后续 ticket 定义, 见 §8.3。

---

**当前状态**: 主要决策(§4.3 清单、D-1 / D-2 / D-3 / D-4 / D-6 / D-11 / D-12)已确认并完成 P0–P6 实现。D-5 / D-7 / D-8 / D-9 按建议实现。
