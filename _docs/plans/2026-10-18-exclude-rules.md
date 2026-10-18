# 排除规则(隐藏文件 + 自定义排除模式)Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 压缩支持排除规则: 内建隐藏文件默认集(OS 原生口径)+ 用户自定义排除模式(统一语法, 多条叠加), 扫描统计与后端命令双轨生效(不变式: 统计口径 ≡ 后端实压内容)。

**Architecture:** core 新增 `exclude` 模块(规则类型 + 统一模式匹配引擎 + 按后端归一化为命令开关); `scan_dir` 接收规则, 顶层条目永远返回并打标(`Entry.exclude`), 树内被排除项不计尺寸; Windows 属性隐藏在遍历时收集相对路径(`Entry.attr_hidden`, 预算护栏); CLI 单调用路径(`compress_dir`)丢弃被标条目, UI 分阶段路径把标记透给前端(显示但不勾选)。

**Tech Stack:** Rust workspace(crates/core, crates/cli, crates/tauri-app, test/zip-tool-e2e), clap 4, Tauri 2 + 静态 HTML/JS UI(node --test + JSDOM), 系统工具 zip/7z。

**规格:** `_docs/exclude-rules.md`(已提交 acffbb8)。

---

## 项目约定(每个任务都适用)

- 直接在 `main` 分支开发(项目惯例, 用户已确认); 每个任务结束提交一次。
- 提交信息用中文, 结尾加:
  ```
  Co-Authored-By: Claude Code <noreply@anthropic.com>
  ```
- **绝不提交 `todo.md`**(用户自己的未跟踪文件): 提交统一用 `git add -A && git reset -q todo.md`。
- 全 workspace 绿(`cargo test` 全部 crate + `cd crates/tauri-app && npm test`)才提交; e2e crate(`zip-tool-e2e`)按任务要求跑。
- bash heredoc 会吃反斜杠——写文件一律用 Write/Edit 工具, 不用 heredoc。
- Rust `format!` 没有内嵌字段访问(如 `{x.y}` 非法), 先绑定局部变量。
- 注释与文档中文, CLI 运行输出英文(与现状一致)。

## 文件结构总览

| 文件 | 动作 | 职责 |
|---|---|---|
| `_docs/exclude-rules.md` | 修改 | §2 arg 顺序勘误 + §3 归一化表补全(Task 1) |
| `crates/core/src/exclude.rs` | 新建 | 规则类型 + 校验 + 匹配引擎 + 归一化(Task 1/2/3) |
| `crates/core/src/lib.rs` | 修改 | 模块声明/导出; `Options.exclude` 字段; `InvalidExclude` 错误(Task 1) |
| `crates/core/src/scan.rs` | 修改 | `scan_dir` 收规则参数; 顶层打标; `dir_size` 内部剔除 + 属性收集(Task 4) |
| `crates/core/src/backend.rs` | 修改 | `compress_command` 注入排除开关; trait 波及(Task 3) |
| `crates/core/src/compress.rs` | 修改 | 调用点适配; `compress_dir` 顶层丢弃 + `Report.excluded`(Task 3/5) |
| `crates/cli/src/main.rs` | 修改 | `--exclude` / `--include-hidden-files` + excluded 提示(Task 6) |
| `test/tests/cli_e2e.rs` | 修改 | 排除规则 e2e 矩阵(Task 7) |
| `crates/tauri-app/src/lib.rs` | 修改 | scan/compress DTO 加 excludes; 保鲜校验(Task 8) |
| `crates/tauri-app/ui/index.html` | 修改 | 高级区排除输入(Task 9) |
| `crates/tauri-app/ui/app.js` | 修改 | parseExcludes + 默认不勾选 + 透传(Task 9) |
| `crates/tauri-app/test/ui.test.mjs` | 修改 | 排除行为/契约测试(Task 9) |
| `_docs/compress.md` / `README.md` / `_docs/ui-backend.md` | 修改 | 文档同步(Task 10) |

---

### Task 1: 规格勘误 + core 类型落地(编译绿)

**Files:**
- Modify: `_docs/exclude-rules.md`(§2 倒数第一行、§3 表格)
- Create: `crates/core/src/exclude.rs`
- Modify: `crates/core/src/lib.rs`
- Modify: `crates/core/src/scan.rs`、`crates/core/src/backend.rs`、`crates/core/src/compress.rs`、`crates/cli/src/main.rs`、`crates/tauri-app/src/lib.rs`(只为修结构体字面量, 编译绿)

- [ ] **Step 1: 勘误规格 §2 的 arg 顺序**

`_docs/exclude-rules.md` 第 61 行(§2 末行)原文:

```
- 命令构造 arg 顺序不变(格式 flags → -v/-p → 级别 → 排除开关 → `--`), 排除开关在 `--` 之前、条目名之前
```

整行替换为:

```
- 排除开关的位置**按后端可执行文件**(Info-ZIP 与 7z 语法不同):
  - `zip`(Linux zip 格式): `-x` 开关必须在**条目名之后**(Info-ZIP 语法 `zip -r out.zip A -x pat...`)
  - `7z`(Windows zip 格式 + 全部 7z 格式): `-x!` 开关在 `--` 之前、条目名之前(既有 flags → `--` → 条目名次序不变)
```

- [ ] **Step 1b: 勘误规格 §2 的 ScanReport.excluded 类型**

`_docs/exclude-rules.md` 第 56 行原文:

```
- `Entry` 增加 `exclude: Option<ExcludeReason>`; `ScanReport` 增加 `excluded: Vec<String>`(被标条目名, CLI 提示用)
```

整行替换为(名单带原因, CLI 提示与 UI 复用同一结构):

```
- `Entry` 增加 `exclude: Option<ExcludeReason>` 与 `attr_hidden: Vec<String>`(§4 属性路径); `ScanReport` 增加 `excluded: Vec<ExcludedEntry>`(name + reason, CLI 提示用)
```

- [ ] **Step 2: 勘误规格 §3 归一化表**

`_docs/exclude-rules.md` 第 74-77 行的表格整表替换为:

```
| 目标后端 | 不含 `/` 模式 p(任意深度 basename) | 含 `/` 锚定模式 | `prefix/**`(at-or-below) |
|---|---|---|---|
| `zip` 命令 | `-x "*/p"` + `-x "*/p/*"`(双开关, 见下注) | `-x "*/<p>"` + `-x "*/<p>/*"`(无 `**` 段) | `-x "*/prefix"` + `-x "*/prefix/*"` |
| `7z` 命令 | `-xr!p`(递归名排除) | `-x!<条目名>/<p>`(`\` 按平台分隔符, 精确锚定) | `-x!<条目名>/prefix`(7z 排除目录即整树) |

- 注(zip 双开关): zip 对被排除目录是否继续下钻行为不确定, `*/p` 覆盖目录自身与树内文件、`*/p/*` 兜底树内路径, 两条一起保证目录排除; 空目录条目可能残留(接受, 见 §1 非目标)。
- Windows 属性隐藏路径(非模式可表达)逐条: `zip` → `-x "<rel>"` + `-x "<rel>/*"`; `7z` → `-x!<rel>`(rel 为条目名前缀的 `/` 分隔相对路径, 目录一条即整树; zip 侧同样发双开关)。
```

- [ ] **Step 3: 写 `crates/core/src/exclude.rs`(类型 + 校验)**

新建文件, 内容如下(本任务只落类型与校验; 匹配引擎在 Task 2, 归一化在 Task 3, 各函数所在位置先占好注释分节):

```rust
//! 排除规则:内建隐藏默认集 + 用户自定义模式,叠加(并集)生效。
//! 统一语法见 _docs/exclude-rules.md §3:
//! - 不含 `/`:任意深度的文件/目录名(basename, 如 `Thumbs.db`、`*.tmp`)
//! - 含 `/`:从条目根锚定匹配相对路径(`*`/`?` 不跨 `/`,`**` 只允许在结尾)
//! - 隐藏判定仅用 OS 原生口径:Unix = `.` 前缀约定,Windows = 文件系统隐藏属性
//! - 不变式:统计口径 ≡ 后端实压内容(任何护栏回退都以"加回统计"保持一致)

/// 排除规则:UI 恒定 `include_hidden = false`(被标行显示不勾选);
/// CLI 默认排除隐藏,`--include-hidden-files` 置 true 关闭
#[derive(Debug, Default, Clone)]
pub struct ExcludeRules {
    /// true = 不排除任何隐藏项(内建默认集关闭;此时不会产生 Hidden 标记)
    pub include_hidden: bool,
    /// 用户自定义模式,多条叠加;无否定语义
    pub patterns: Vec<String>,
}

/// 顶层条目的排除标记(仅顶层;树内文件不逐个上报)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExcludeReason {
    /// 内建隐藏判定命中
    Hidden,
    /// 用户模式命中(保留命中模式, 供 UI 展示)
    Pattern(String),
}

impl std::fmt::Display for ExcludeReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExcludeReason::Hidden => write!(f, "hidden"),
            ExcludeReason::Pattern(p) => write!(f, "pattern {p:?}"),
        }
    }
}

/// 被排除的顶层条目(CLI 提示用)
#[derive(Debug, Clone)]
pub struct ExcludedEntry {
    pub name: String,
    pub reason: ExcludeReason,
}

/// Windows 属性隐藏路径收集上限(条数):超限停止收集, 未收集项不计入排除
/// (统计与后端一致地"不排除"), 并发 warning
pub const EXCLUDE_PATH_LIMIT: usize = 1000;
/// 排除开关字符预算(与 EXCLUDE_PATH_LIMIT 双护栏, 收集端同步执行)
pub const EXCLUDE_ARG_BUDGET: usize = 24_000;

/// Unix 内建隐藏默认集的统一模式(任意深度 basename 以 `.` 开头)。
/// Windows 不用名字口径(`.` 开头不算隐藏), 不注入此模式。
pub const HIDDEN_UNIX_PATTERN: &str = ".*";

/// 校验用户模式(非法即报错, 宁停不错):
/// 空串 / 空路径段(首尾或连续 `/`)/ 单独 `**` / `**` 不在结尾, 都非法
pub fn validate_patterns(patterns: &[String]) -> Result<(), String> {
    for p in patterns {
        let p = p.replace('\\', "/");
        if p.is_empty() {
            return Err("排除模式不能为空".into());
        }
        if p.starts_with('/') || p.ends_with('/') || p.contains("//") {
            return Err(format!("排除模式 {p:?} 含空路径段"));
        }
        if p == "**" {
            return Err("排除模式不能只含 **".into());
        }
        if let Some(i) = p.find("**") {
            if i + 2 != p.len() {
                return Err(format!("排除模式 {p:?} 的 ** 只允许出现在结尾"));
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 4: lib.rs 挂模块 + 导出 + Options 字段 + 新错误变体**

`crates/core/src/lib.rs`:

1) 模块声明按字母序插入(`pub mod doctor;` 之后、`pub mod extract;` 之前):

```rust
pub mod exclude;
```

2) 导出区(`pub use extract::{...}` 之后)加一行:

```rust
pub use exclude::{ExcludeReason, ExcludeRules, ExcludedEntry, validate_patterns};
```

3) `Options` 末尾加字段:

```rust
    /// 排除规则:内建隐藏默认集 + 用户模式;Default = 排除隐藏、无用户模式
    pub exclude: ExcludeRules,
```

4) `CoreError` 枚举在 `ToolMissing(String)` 变体之后加:

```rust
    /// 压缩:用户排除模式非法(空串 / 空段 / `**` 位置错误)
    InvalidExclude(String),
```

5) `Display` 实现的 `ToolMissing` 分支后加:

```rust
            CoreError::InvalidExclude(msg) => write!(f, "invalid exclude pattern: {msg}"),
```

6) `lib.rs` 底部 `mod tests` 的 `default_options_do_not_split_or_encrypt` 测试追加两行断言:

```rust
        assert!(opts.exclude.patterns.is_empty(), "默认无用户模式");
        assert!(!opts.exclude.include_hidden, "默认排除隐藏文件");
```

- [ ] **Step 5: 修全部结构体字面量(编译绿)**

以下每个位置都补上新字段, 不改其他逻辑:

1) `crates/core/src/scan.rs` — `Entry` 结构体加字段(在 `format` 字段后):

```rust
    /// 排除标记(仅顶层打标;None = 正常条目)。扫描永远返回全部条目,
    /// CLI 丢弃被标条目, UI 渲染为不勾选
    pub exclude: Option<crate::exclude::ExcludeReason>,
    /// Windows 属性隐藏的树内相对路径(条目名前缀, `/` 分隔, 见 _docs/exclude-rules.md §4);
    /// 非 Windows 恒为空
    pub attr_hidden: Vec<String>,
```

并把同文件 `scan_dir` 里生产 `Entry` 构造(`report.entries.push(Entry {` 处)补:

```rust
            exclude: None,
            attr_hidden: Vec::new(),
```

`ScanReport` 加字段(`warnings` 之后):

```rust
    /// 被排除的顶层条目名单(CLI 提示用;与 entries 中带 exclude 标记者一致)
    pub excluded: Vec<crate::exclude::ExcludedEntry>,
```

2) `crates/core/src/backend.rs` — 测试助手 `opts_fmt`(无 `..Default`)补 `exclude: ExcludeRules::default(),`(需 `use crate::ExcludeRules;` 或写全路径; 该文件 tests 模块已有 `use super::*;`,`super` 里没有, 直接写 `crate::ExcludeRules::default()`)。其余 `opts()` 助手带 `..Options::default()` 不用改。

3) `crates/core/src/compress.rs`:
   - `dst_tests::entries`(约 349 行)每个 `Entry` 字面量补 `exclude: None, attr_hidden: Vec::new(),`;
   - `staged_tests::entry`(约 878 行)同样补。

4) `crates/cli/src/main.rs` — `Options { ... }` 字面量(约 86 行)补 `exclude: Default::default(),`(CLI 真实接线在 Task 6)。

5) `crates/tauri-app/src/lib.rs`:
   - `to_core_options` 返回的 `Options { ... }` 补 `exclude: Default::default(),`;
   - 测试 `entry()`(约 589 行)与 `compress_ctx`(约 599 行)的 `Entry`/`ScanReport` 字面量补 `exclude: None, attr_hidden: Vec::new(),` 与 `excluded: Vec::new(),`。

- [ ] **Step 6: 全 workspace 编译 + 既有测试绿**

Run: `cargo test --workspace`
Expected: 全部编译通过, 既有测试全绿(新增字段未改变行为)。

- [ ] **Step 7: validate_patterns 单测(加在 exclude.rs 文件末尾)**

```rust
#[cfg(test)]
mod validate_tests {
    use super::*;

    fn ok(pats: &[&str]) {
        let v: Vec<String> = pats.iter().map(|s| s.to_string()).collect();
        assert!(validate_patterns(&v).is_ok(), "{pats:?} 应合法");
    }
    fn bad(pats: &[&str]) {
        let v: Vec<String> = pats.iter().map(|s| s.to_string()).collect();
        assert!(validate_patterns(&v).is_err(), "{pats:?} 应非法");
    }

    #[test]
    fn accepts_legal_patterns() {
        ok(&["Thumbs.db", "*.tmp", "sub/*.log", "node_modules/**", "a?c", r"win\path"]);
    }

    #[test]
    fn rejects_illegal_patterns() {
        bad(&[""]);            // 空串
        bad(&["/abs"]);        // 首段空
        bad(&["dir/"]);        // 尾段空
        bad(&["a//b"]);        // 中段空
        bad(&["**"]);          // 单独 **
        bad(&["a/**/b"]);      // ** 不在结尾
        bad(&["a**b"]);        // ** 不在结尾
        // 反斜杠只做归一, 不是非法来源
        ok(&[r"a\b", r"a\b\**"]);
    }
}
```

Run: `cargo test -p zip-tool-core exclude`
Expected: validate_tests 全 PASS。

- [ ] **Step 8: Commit**

```bash
git add -A && git reset -q todo.md
git commit -m "feat(core): 排除规则类型与模式校验 + 规格勘误(zip -x 在条目名后)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 2: exclude.rs 匹配引擎(TDD)

**Files:**
- Modify: `crates/core/src/exclude.rs`

- [ ] **Step 1: 写失败测试(加在 exclude.rs 末尾, 新 tests 模块)**

```rust
#[cfg(test)]
mod match_tests {
    use super::*;

    #[test]
    fn basename_pattern_matches_any_depth_without_crossing() {
        // 不含 / 的模式 = 任意深度的 basename(只与末段比)
        assert!(matches_pattern("Thumbs.db", "Thumbs.db"));
        assert!(matches_pattern("Thumbs.db", "a/b/Thumbs.db"));
        assert!(!matches_pattern("Thumbs.db", "a/b/Thumbs"));
        assert!(matches_pattern("*.log", "a.log"));
        assert!(matches_pattern("*.log", "sub/b.log"));
        assert!(matches_pattern("*.log", "sub/deep/c.log"));
        assert!(!matches_pattern("*.log", "a.log2"));
        // ? 单字符
        assert!(matches_pattern("a?c", "abc"));
        assert!(!matches_pattern("a?c", "ac"));
        assert!(!matches_pattern("a?c", "abbc"));
        // 大小写敏感
        assert!(!matches_pattern("Thumbs.db", "thumbs.db"));
    }

    #[test]
    fn anchored_pattern_matches_path_from_entry_root() {
        // 含 / 的模式 = 从条目根锚定的整条相对路径
        assert!(matches_pattern("sub/*.log", "sub/a.log"));
        assert!(!matches_pattern("sub/*.log", "sub/x/y.log"), "* 不跨 /");
        assert!(!matches_pattern("sub/a.log", "other/sub/a.log"), "锚定不任意深度");
        assert!(!matches_pattern("sub/a.log", "sub/a.log2"));
        // prefix/** = prefix 自身或其下
        assert!(matches_pattern("node_modules/**", "node_modules"));
        assert!(matches_pattern("node_modules/**", "node_modules/a/b.js"));
        assert!(!matches_pattern("node_modules/**", "node_modulesx"));
        assert!(!matches_pattern("node_modules/**", "a/node_modules/b"));
        // 输入侧容忍反斜杠(归一为 /)
        assert!(matches_pattern("sub\\a.log", "sub/a.log"));
        assert!(matches_pattern("sub/*.log", "sub\\a.log"));
    }

    #[test]
    fn matches_patterns_returns_first_hit() {
        let pats: Vec<String> = vec!["*.tmp".into(), "cache".into()];
        assert_eq!(matches_patterns(&pats, "x/y/tmp.tmp"), Some("*.tmp".into()));
        assert_eq!(matches_patterns(&pats, "cache"), Some("cache".into()));
        assert_eq!(matches_patterns(&pats, "keep.txt"), None);
        assert_eq!(matches_patterns(&[], "anything"), None);
    }

    #[test]
    fn hidden_name_uses_unix_convention_only() {
        if cfg!(windows) {
            // Windows 无名字口径: . 开头不算隐藏(可能是用户自命名)
            assert!(!is_hidden_name(".DS_Store"));
            assert!(!is_hidden_name(".git"));
        } else {
            assert!(is_hidden_name(".DS_Store"));
            assert!(is_hidden_name(".."));
            assert!(is_hidden_name(".git"));
            assert!(!is_hidden_name("git"));
            assert!(!is_hidden_name("a.txt"));
        }
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p zip-tool-core exclude`
Expected: 编译失败, `matches_pattern` / `matches_patterns` / `is_hidden_name` 未定义(`cannot find function`)。

- [ ] **Step 3: 实现匹配引擎(加在 exclude.rs 的 validate_patterns 之后)**

```rust
/// 单段通配匹配:`*` 任意串、`?` 单字符(都不跨 `/`),其余字面。
/// 两指针 + `*` 回溯(经典 glob 单段算法)。
fn match_seg(pat: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let mut star = usize::MAX; // 最近一个 '*' 的位置
    let mut mark = 0usize; // 回溯时 text 的推进点
    while t < text.len() {
        if p < pat.len() && (pat[p] == b'?' || pat[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pat.len() && pat[p] == b'*' {
            star = p;
            mark = t;
            p += 1;
        } else if star != usize::MAX {
            p = star + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == b'*' {
        p += 1;
    }
    p == pat.len()
}

/// 统一模式匹配相对路径(分隔符 `/`,输入侧 `\` 容忍归一):
/// - 不含 `/`:与末段(basename)比 → 任意深度
/// - 含 `/`:整条锚定,逐段比较(`*`/`?` 不跨段);`prefix/**` = prefix 自身或其下
pub fn matches_pattern(pattern: &str, rel: &str) -> bool {
    let pat = pattern.replace('\\', "/");
    let rel = rel.replace('\\', "/");
    if !pat.contains('/') {
        return rel
            .rsplit('/')
            .next()
            .is_some_and(|base| match_seg(pat.as_bytes(), base.as_bytes()));
    }
    if let Some(prefix) = pat.strip_suffix("/**") {
        let pfx = prefix.trim_end_matches('/');
        return rel == pfx
            || (rel.len() > pfx.len()
                && rel.starts_with(pfx)
                && rel.as_bytes()[pfx.len()] == b'/');
    }
    let pv: Vec<&str> = pat.split('/').collect();
    let rv: Vec<&str> = rel.split('/').collect();
    pv.len() == rv.len() && pv.iter().zip(&rv).all(|(p, r)| match_seg(p.as_bytes(), r.as_bytes()))
}

/// 命中任一模式即排除, 返回首个命中的模式(供 `ExcludeReason::Pattern`)
pub fn matches_patterns(patterns: &[String], rel: &str) -> Option<String> {
    patterns.iter().find(|p| matches_pattern(p, rel)).cloned()
}

/// 内建隐藏判定(名字口径):仅 Unix 原生约定(`.` 前缀);
/// Windows 无名字口径(`.` 开头可能是用户自命名), 恒 false, 走属性判定
pub fn is_hidden_name(name: &str) -> bool {
    !cfg!(windows) && name.starts_with('.')
}

/// Windows 文件系统隐藏属性(std 稳定 API);非 Windows 恒 false
pub fn is_hidden_attr(md: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::PermissionsExt as _;
        md.permissions().hidden()
    }
    #[cfg(not(windows))]
    {
        let _ = md;
        false
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p zip-tool-core exclude`
Expected: match_tests + validate_tests 全 PASS。

- [ ] **Step 5: Commit**

```bash
git add -A && git reset -q todo.md
git commit -m "feat(core): 统一排除模式匹配引擎(basename/锚定/** 三形态 + 隐藏口径)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 3: 归一化为后端开关(compress_command 注入 + trait 波及)

**Files:**
- Modify: `crates/core/src/exclude.rs`(归一化函数)
- Modify: `crates/core/src/backend.rs`(compress_command + CommandBackend trait + 测试)
- Modify: `crates/core/src/compress.rs`(调用点 + 3 个假后端)

- [ ] **Step 1: 写失败测试(backend.rs 的 `mod tests` 内, `opts_fmt` 之后)**

```rust
    /// 当前平台 7z 的包内路径分隔符(断言用)
    fn sep() -> char {
        if cfg!(windows) { '\\' } else { '/' }
    }

    #[test]
    fn zip_command_appends_exclude_switches_after_entry_name() {
        // Info-ZIP 语法: -x 必须在条目名之后(zip -r out.zip A -x pat...)
        let mut o = opts_fmt(Format::Zip, None);
        o.exclude.patterns = vec!["Thumbs.db".into(), "sub/*.log".into()];
        let cmd = Backend::Zip
            .compress_command(Path::new("/in"), "A", Path::new("/out/A.zip"), &o, &[])
            .unwrap();
        assert_eq!(
            args_of(&cmd),
            [
                "-r", "/out/A.zip", "A",
                // basename 模式发双开关(目录下钻行为不确定, 两条兜底)
                "-x", "*/Thumbs.db", "-x", "*/Thumbs.db/*",
                // 锚定模式同样双开关; * 不需改写(zip 的 * 天然跨 /)
                "-x", "*/sub/*.log", "-x", "*/sub/*.log/*",
            ]
        );
    }

    #[test]
    fn zip_command_maps_doublestar_suffix_to_prefix_pair() {
        let mut o = opts_fmt(Format::Zip, None);
        o.exclude.patterns = vec!["node_modules/**".into()];
        let cmd = Backend::Zip
            .compress_command(Path::new("/in"), "A", Path::new("/out/A.zip"), &o, &[])
            .unwrap();
        assert_eq!(
            args_of(&cmd),
            ["-r", "/out/A.zip", "A", "-x", "*/node_modules", "-x", "*/node_modules/*"]
        );
    }

    #[test]
    fn sevenzip_command_emits_exclude_before_separator() {
        let mut o = opts_fmt(Format::SevenZ, None);
        o.exclude.patterns = vec!["Thumbs.db".into(), "sub/*.log".into()];
        let cmd = Backend::SevenZip
            .compress_command(Path::new("D:/data"), "A", Path::new("E:/out/A.7z"), &o, &[])
            .unwrap();
        let s = sep();
        assert_eq!(
            args_of(&cmd),
            [
                "a", "-t7z", "-m0=LZMA2",
                // basename → -xr!(任意深度);锚定 → 条目名前缀精确路径
                "-xr!Thumbs.db",
                &format!("-x!A{s}sub{s}*.log"),
                "--", "E:/out/A.7z", "A",
            ]
        );
    }

    #[test]
    fn sevenzip_maps_doublestar_suffix_to_dir_switch() {
        let mut o = opts_fmt(Format::SevenZ, None);
        o.exclude.patterns = vec!["node_modules/**".into()];
        let cmd = Backend::SevenZip
            .compress_command(Path::new("D:/data"), "A", Path::new("E:/out/A.7z"), &o, &[])
            .unwrap();
        let s = sep();
        assert!(args_of(&cmd).contains(&format!("-x!A{s}node_modules")));
    }

    #[test]
    fn hidden_attribute_paths_become_exact_switches() {
        let mut o = opts_fmt(Format::SevenZ, None);
        o.exclude.patterns = vec!["*.tmp".into()];
        let cmd = Backend::SevenZip
            .compress_command(
                Path::new("D:/data"),
                "A",
                Path::new("E:/out/A.7z"),
                &o,
                &["A/old/secret.bin".to_string()],
            )
            .unwrap();
        let s = sep();
        let args = args_of(&cmd);
        assert!(args.contains(&format!("-x!A{s}old{s}secret.bin")), "{args:?}");
        assert!(args.contains(&"-xr!*.tmp"), "{args:?}");
    }

    #[test]
    #[cfg(not(windows))]
    fn unix_hidden_default_set_becomes_dot_star_pattern() {
        // 默认(include_hidden=false)注入统一隐藏模式
        let cmd = Backend::Zip
            .compress_command(
                Path::new("/in"),
                "A",
                Path::new("/out/A.zip"),
                &opts_fmt(Format::Zip, None),
                &[],
            )
            .unwrap();
        assert_eq!(
            args_of(&cmd),
            ["-r", "/out/A.zip", "A", "-x", "*/.*", "-x", "*/.*/*"]
        );
        // include_hidden=true: 无任何隐藏开关
        let mut o = opts_fmt(Format::Zip, None);
        o.exclude.include_hidden = true;
        let cmd = Backend::Zip
            .compress_command(Path::new("/in"), "A", Path::new("/out/A.zip"), &o, &[])
            .unwrap();
        assert_eq!(args_of(&cmd), ["-r", "/out/A.zip", "A"]);
    }

    #[test]
    #[cfg(windows)]
    fn windows_default_has_no_name_based_hidden_switches() {
        // Windows 隐藏走属性枚举(attr_hidden), 名字模式集不含隐藏
        let cmd = Backend::SevenZip
            .compress_command(
                Path::new("D:/data"),
                "A",
                Path::new("E:/out/A.zip"),
                &opts_fmt(Format::Zip, None),
                &[],
            )
            .unwrap();
        assert_eq!(args_of(&cmd), ["a", "-tzip", "--", "E:/out/A.zip", "A"]);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p zip-tool-core backend`
Expected: 编译失败——`compress_command` 还不接受第 5 个参数(`attr_hidden`)。

- [ ] **Step 3: exclude.rs 实现归一化**

追加在 `is_hidden_attr` 之后:

```rust
/// 7z `-x!` 的包内路径分隔符(7z 按运行平台写入/匹配本地分隔符)
#[cfg(windows)]
const SEP: char = '\\';
#[cfg(not(windows))]
const SEP: char = '/';

/// 生效模式集 = 内建隐藏默认集(include_hidden=false 且非 Windows 时)+ 用户模式
fn effective_patterns(rules: &ExcludeRules) -> Vec<String> {
    let mut v = Vec::new();
    if !rules.include_hidden && !cfg!(windows) {
        v.push(HIDDEN_UNIX_PATTERN.to_string());
    }
    v.extend(rules.patterns.iter().cloned());
    v
}

/// 追加一组 argv token;字符预算不足时整组丢弃(护栏;收集端已控量, 正常不触发)。
/// 返回 false = 预算耗尽。
fn push_tokens(args: &mut Vec<String>, budget: &mut usize, toks: Vec<String>) -> bool {
    let cost: usize = toks.iter().map(|t| t.chars().count() + 1).sum();
    if cost > *budget {
        return false;
    }
    *budget -= cost;
    args.extend(toks);
    true
}

/// 把规则归一化为后端命令的排除开关 argv 片段(见 _docs/exclude-rules.md §3):
/// - zip:开关在条目名**之后**;模式 → `-x <glob>` 双开关(`*/p` + `*/p/*`)
/// - 7z:开关在 `--` 之前;basename → `-xr!p`,锚定 → `-x!<条目名><SEP>路径`,目录 `/**` → 一条
/// - Windows 属性隐藏路径(模式不可表达)逐条追加
pub(crate) fn exclusion_args(
    backend: crate::backend::Backend,
    entry_name: &str,
    rules: &ExcludeRules,
    attr_hidden: &[String],
) -> Vec<String> {
    let mut args = Vec::new();
    let mut budget = EXCLUDE_ARG_BUDGET;
    match backend {
        crate::backend::Backend::Zip => {
            for p in effective_patterns(rules) {
                // `/**` 结尾 → 取前缀(目录 at-or-below 双开关);其余 `**` 已被校验排除
                let base = p.strip_suffix("/**").unwrap_or(&p);
                if !push_tokens(
                    &mut args,
                    &mut budget,
                    vec!["-x".into(), format!("*/{base}")],
                ) {
                    break;
                }
                if !push_tokens(
                    &mut args,
                    &mut budget,
                    vec!["-x".into(), format!("*/{base}/*")],
                ) {
                    break;
                }
            }
            for rel in attr_hidden {
                if !push_tokens(
                    &mut args,
                    &mut budget,
                    vec!["-x".into(), rel.clone(), "-x".into(), format!("{rel}/*")],
                ) {
                    break;
                }
            }
        }
        crate::backend::Backend::SevenZip => {
            for p in effective_patterns(rules) {
                let tok = if !p.contains('/') {
                    format!("-xr!{p}")
                } else if let Some(prefix) = p.strip_suffix("/**") {
                    format!("-x!{entry_name}{SEP}{}", prefix.replace('/', &SEP.to_string()))
                } else {
                    format!("-x!{entry_name}{SEP}{}", p.replace('/', &SEP.to_string()))
                };
                if !push_tokens(&mut args, &mut budget, vec![tok]) {
                    break;
                }
            }
            for rel in attr_hidden {
                let tok = format!("-x!{}", rel.replace('/', &SEP.to_string()));
                if !push_tokens(&mut args, &mut budget, vec![tok]) {
                    break;
                }
            }
        }
    }
    args
}
```

- [ ] **Step 4: backend.rs 注入排除开关**

1) `Backend::compress_command` 加第 5 个参数并注入(`crates/core/src/backend.rs:129`):

```rust
    pub fn compress_command(
        &self,
        work_dir: &Path,
        name: &str,
        out_zip: &Path,
        opts: &Options,
        attr_hidden: &[String],
    ) -> Result<Command> {
```

`Backend::Zip` 分支末尾(`cmd.arg(out_zip).arg(src);` 之后)加:

```rust
                // Info-ZIP 语法: 排除开关必须在条目名之后
                for tok in crate::exclude::exclusion_args(*self, name, &opts.exclude, attr_hidden) {
                    cmd.arg(tok);
                }
```

`Backend::SevenZip` 分支把 `cmd.arg("--").arg(out_zip).arg(name);` 改为:

```rust
                // 排除开关在 -- 之前(既有 flags 之后)
                for tok in crate::exclude::exclusion_args(*self, name, &opts.exclude, attr_hidden) {
                    cmd.arg(tok);
                }
                cmd.arg("--").arg(out_zip).arg(name);
```

2) `CommandBackend` trait 与 `impl CommandBackend for Backend` 的 `compress_command` 都加 `attr_hidden: &[String]` 参数(trait 定义在 `crates/core/src/backend.rs:314`, impl 在 326 行, 转发调用补 `attr_hidden`)。

- [ ] **Step 5: compress.rs 适配(调用点 + 假后端)**

1) 真实调用点(`compress_entries_with_backend` 内, 约 166 行):

```rust
        let result = backend
            .compress_command(&input_abs, &e.name, &out_archive, opts, &e.attr_hidden)
            .and_then(|mut cmd| backend.run(&mut cmd, cancel));
```

2) 三个假后端的 `compress_command`(`FakeBackend` 约 596 行、`CancellingBackend` 约 770 行、`Recorder` 约 924 行)签名都加 `_attr_hidden: &[String]`。

3) `backend.rs` tests 模块内既有 `compress_command` 测试调用(约 10 处)全部补第 5 参 `&[]`。

- [ ] **Step 6: 跑测试确认通过**

Run: `cargo test -p zip-tool-core`
Expected: 全 PASS(新 argv 测试 + 既有命令矩阵测试)。

- [ ] **Step 7: Commit**

```bash
git add -A && git reset -q todo.md
git commit -m "feat(core): 排除规则归一化为后端开关(zip 条目名后双开关 / 7z -x! 锚定)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 4: scan_dir 排除集成(顶层打标 + 树内剔除 + Windows 属性收集)

**Files:**
- Modify: `crates/core/src/scan.rs`(核心)
- Modify: `crates/core/src/compress.rs`、`crates/tauri-app/src/lib.rs`(`scan_dir` 调用点, 本任务先传 `&opts.exclude` / `&Default::default()`)

- [ ] **Step 1: 写失败测试(scan.rs 的 `mod tests` 内)**

```rust
    use crate::{CoreError, ExcludeReason, ExcludeRules};

    fn scan_with(root: &Path, rules: &ExcludeRules) -> ScanReport {
        scan_dir(root, rules, &|_| {}).unwrap()
    }

    #[test]
    fn scan_unix_hidden_marks_top_level_and_deducts_total() {
        if cfg!(windows) {
            return; // Windows 无名字口径, 该用例仅 Unix 语义
        }
        let root = unique_temp("scan-hidden");
        fs::create_dir_all(root.join("in/.hid")).unwrap();
        fs::write(root.join("in/.hid/x"), "xx").unwrap(); // 2 字节, 整树剔除
        fs::write(root.join("in/.file"), "fff").unwrap(); // 3 字节
        fs::write(root.join("in/keep.txt"), "k").unwrap(); // 1 字节

        let report = scan_with(&root.join("in"), &ExcludeRules::default());
        // 顶层永远返回, 但被标 Hidden
        let hid = report.entries.iter().find(|e| e.name == ".hid").unwrap();
        assert_eq!(hid.exclude, Some(ExcludeReason::Hidden));
        assert_eq!(hid.size, 2, "条目自身 size 保留真实值(展示用)");
        let f = report.entries.iter().find(|e| e.name == ".file").unwrap();
        assert_eq!(f.exclude, Some(ExcludeReason::Hidden));
        // 被标条目不计入总量
        assert_eq!(report.total_bytes, 1, "只有 keep.txt 计入");
        // excluded 名单
        let names: Vec<&str> = report.excluded.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, [".file", ".hid"]);

        // include_hidden=true: 全部保留不打标
        let rules = ExcludeRules { include_hidden: true, patterns: vec![] };
        let report = scan_with(&root.join("in"), &rules);
        assert!(report.entries.iter().all(|e| e.exclude.is_none()));
        assert_eq!(report.total_bytes, 6);
        assert!(report.excluded.is_empty());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scan_windows_attr_hidden_marks_and_collects_inner_paths() {
        if !cfg!(windows) {
            return; // attrib 仅 Windows
        }
        let root = unique_temp("scan-attr");
        fs::create_dir_all(root.join("in/outer/inner")).unwrap();
        fs::write(root.join("in/outer/inner/secret.txt"), "sss").unwrap(); // 3 字节
        fs::write(root.join("in/Thumbs.db"), "tt").unwrap(); // 2 字节
        fs::write(root.join("in/outer/plain.txt"), "pp").unwrap(); // 2 字节
        for p in ["in/Thumbs.db", "in/outer/inner"] {
            let st = std::process::Command::new("attrib")
                .args(["+H", root.join(p).to_str().unwrap()])
                .status()
                .unwrap();
            assert!(st.success(), "attrib +H 失败: {p}");
        }

        let report = scan_with(&root.join("in"), &ExcludeRules::default());
        // 顶层属性隐藏打标(名字口径不生效: . 开头不算)
        let t = report.entries.iter().find(|e| e.name == "Thumbs.db").unwrap();
        assert_eq!(t.exclude, Some(ExcludeReason::Hidden));
        // 内部隐藏目录: 整树剔除 + 收集为一条条目名前缀相对路径(/ 分隔)
        let o = report.entries.iter().find(|e| e.name == "outer").unwrap();
        assert_eq!(o.size, 2, "plain.txt 计入, inner 整树剔除");
        assert_eq!(o.attr_hidden, vec!["outer/inner".to_string()]);
        assert_eq!(report.total_bytes, 2, "隐藏的顶层与内部都不计入");

        // include_hidden=true: 不收集不打标不剔除
        let rules = ExcludeRules { include_hidden: true, patterns: vec![] };
        let report = scan_with(&root.join("in"), &rules);
        assert!(report.entries.iter().all(|e| e.exclude.is_none()));
        assert!(report.entries.iter().all(|e| e.attr_hidden.is_empty()));
        assert_eq!(report.total_bytes, 7);

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scan_applies_user_patterns_to_top_level_and_tree() {
        let root = unique_temp("scan-pattern");
        fs::create_dir_all(root.join("in/App/cache")).unwrap();
        fs::create_dir_all(root.join("in/keep")).unwrap();
        fs::write(root.join("in/App/cache/junk.tmp"), "jj").unwrap(); // 2
        fs::write(root.join("in/App/main.bin"), "mmm").unwrap(); // 3
        fs::write(root.join("in/keep/data"), "dd").unwrap(); // 2
        fs::write(root.join("in/drop.log"), "x").unwrap(); // 1

        let rules = ExcludeRules {
            include_hidden: true,
            patterns: vec!["*.log".into(), "App/cache".into()],
        };
        let report = scan_with(&root.join("in"), &rules);
        // 顶层打标(Pattern 保留命中模式)
        let d = report.entries.iter().find(|e| e.name == "drop.log").unwrap();
        assert_eq!(d.exclude, Some(ExcludeReason::Pattern("*.log".into())));
        assert!(report.entries.iter().find(|e| e.name == "keep").unwrap().exclude.is_none());
        // 目录大小扣除: App 只剩 main.bin(cache 整树不下钻);drop.log 不计入总量
        assert_eq!(report.entries.iter().find(|e| e.name == "App").unwrap().size, 3);
        assert_eq!(report.total_bytes, 5);
        assert_eq!(report.excluded.len(), 1);
        assert_eq!(report.excluded[0].reason, ExcludeReason::Pattern("*.log".into()));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scan_rejects_invalid_patterns_as_invalid_exclude() {
        let root = unique_temp("scan-badpat");
        fs::create_dir_all(root.join("in")).unwrap();
        let rules = ExcludeRules { include_hidden: true, patterns: vec!["a/**b".into()] };
        let err = scan_dir(&root.join("in"), &rules, &|_| {}).unwrap_err();
        assert!(matches!(err, CoreError::InvalidExclude(_)));
        fs::remove_dir_all(&root).unwrap();
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p zip-tool-core scan`
Expected: 编译失败——`scan_dir` 还只收 2 个参数。

- [ ] **Step 3: 改 scan_dir 签名与实现**

`crates/core/src/scan.rs` 整体改造(保留原注释风格)。头部 use 区加(原 `use crate::Result;` 一并合并):

```rust
use crate::exclude::{
    AttrCollector, ExcludeReason, ExcludeRules, ExcludedEntry, is_hidden_attr, is_hidden_name,
    matches_patterns, validate_patterns,
};
use crate::{CoreError, Result};
```

`scan_dir` 新签名与实现(整体替换原函数体;回调次数与顺序不变):

```rust
/// 扫描 input 的顶层条目并统计大小; 每完成一个顶层条目回调一次进度。
/// 排除规则(rules)双轨生效: 顶层条目永远返回并打标(`Entry.exclude`,
/// CLI 丢弃 / UI 显示不勾选), 树内被排除项不计尺寸; Windows 属性隐藏
/// 收集为 `Entry.attr_hidden` 相对路径(§4, 预算护栏超限即回退并 warning)。
pub fn scan_dir(
    input: &Path,
    rules: &ExcludeRules,
    on_progress: &dyn Fn(&Progress),
) -> Result<ScanReport> {
    // 宁停不错: 用户模式先校验(UI/CLI 入口已校验, 这里兜底库直调场景)
    validate_patterns(&rules.patterns).map_err(CoreError::InvalidExclude)?;

    // 顶层条目按名字字典序排序, 保证进度与后续产物顺序稳定、可复现
    let mut names: Vec<String> = fs::read_dir(input)?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort_by_key(|n| n.clone());

    let total = names.len();
    let mut report = ScanReport::default();
    // Windows 属性枚举仅在"启用隐藏排除"时进行(§4); 其余平台 None
    let collect_attrs = !rules.include_hidden && cfg!(windows);

    for (i, name) in names.into_iter().enumerate() {
        let path = input.join(&name);
        let mut attr_hidden = Vec::new();
        let (kind, size) = match fs::symlink_metadata(&path) {
            Ok(md) => {
                if md.file_type().is_dir() {
                    let mut collector = AttrCollector::new(name.clone());
                    // 目录: 只统计普通文件与链接自身, 不跟随目录链接(避免循环与重复计数)
                    let sz = dir_size(
                        &path,
                        &name,
                        rules,
                        &mut report.warnings,
                        collect_attrs.then_some(&mut collector),
                    );
                    attr_hidden = collector.finish(&mut report.warnings);
                    (EntryKind::Dir, sz)
                } else {
                    // 文件与符号链接都按自身大小计, 链接归为 File
                    (EntryKind::File, md.len())
                }
            }
            Err(e) => {
                report.warnings.push(format!("{}: {e}", path.display()));
                (EntryKind::File, 0)
            }
        };
        // 顶层打标: 隐藏(OS 原生口径: Unix 名字 / Windows 属性)优先, 其次用户模式
        let hidden = !rules.include_hidden
            && (is_hidden_name(&name)
                || collect_attrs
                    && fs::symlink_metadata(&path).map(|m| is_hidden_attr(&m)).unwrap_or(false));
        let exclude = if hidden {
            Some(ExcludeReason::Hidden)
        } else {
            matches_patterns(&rules.patterns, name).map(ExcludeReason::Pattern)
        };
        if let Some(reason) = &exclude {
            report.excluded.push(ExcludedEntry {
                name: name.clone(),
                reason: reason.clone(),
            });
        }
        report.entries.push(Entry {
            name,
            kind,
            size,
            dst: String::new(),
            format: String::new(),
            exclude,
            attr_hidden,
        });
        // 被标条目不计入总量(统计口径 = 默认将压缩的内容; 条目 size 保留真实值供展示)
        if report.entries[i].exclude.is_none() {
            report.total_bytes += size;
        }

        on_progress(&Progress {
            phase: Phase::Scanning,
            done: i + 1,
            total,
            done_bytes: report.total_bytes,
            total_bytes: 0, // 总量在扫描完成前未知
            entry: report.entries[i].name.clone(),
        });
    }

    Ok(report)
}
```

`dir_size` 整体替换:

```rust
/// 递归累加目录下所有普通文件/链接自身的大小; 栈式迭代避免深递归。
/// 无权限等读不了的子树计 0 并记 warning, 不中断。
/// 排除(树内): 用户模式(相对条目根)+ Unix 名字隐藏; 命中文件不计尺寸,
/// 命中目录整树不下钻。Windows 属性隐藏(attrs=Some 时)收集相对路径而非打标。
fn dir_size(
    dir: &Path,
    entry_name: &str,
    rules: &ExcludeRules,
    warnings: &mut Vec<String>,
    mut attrs: Option<&mut AttrCollector>,
) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![(dir.to_path_buf(), String::new())]; // (目录, 相对条目根的前缀)
    while let Some((d, prefix)) = stack.pop() {
        let rd = match fs::read_dir(&d) {
            Ok(rd) => rd,
            Err(e) => {
                warnings.push(format!("{}: {e}", d.display()));
                continue;
            }
        };
        for entry in rd {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warnings.push(format!("{}: {e}", d.display()));
                    continue;
                }
            };
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let md = match fs::symlink_metadata(&path) {
                Ok(md) => md,
                Err(e) => {
                    warnings.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let attr_hidden = attrs.is_some() && !rules.include_hidden && is_hidden_attr(&md);
            let excluded = attr_hidden
                || (!rules.include_hidden && is_hidden_name(&name))
                || matches_patterns(&rules.patterns, &rel).is_some();
            if excluded {
                if attr_hidden {
                    // 条目名前缀 + / 分隔, 供后端逐条开关(§4)
                    attrs
                        .as_mut()
                        .expect("attr_hidden 只在 attrs=Some 时为真")
                        .add(&format!("{entry_name}/{rel}"));
                }
                continue; // 不计尺寸; 目录不入栈(整树剔除/不下钻)
            }
            if md.file_type().is_dir() {
                stack.push((path, rel));
            } else {
                total += md.len();
            }
        }
    }
    total
}
```

- [ ] **Step 4: exclude.rs 落地 AttrCollector(加在 exclusion_args 之前)**

```rust
/// Windows 属性隐藏路径收集器(§4):条数与字符双预算, 超限停止收集并计数;
/// 未收集的项两端一致地"不排除"(统计照加、后端无开关), 不变式不破。
pub(crate) struct AttrCollector {
    entry_name: String,
    paths: Vec<String>,
    chars: usize,
    overflow: usize,
}

impl AttrCollector {
    pub(crate) fn new(entry_name: String) -> Self {
        Self { entry_name, paths: Vec::new(), chars: 0, overflow: 0 }
    }

    /// rel 已含条目名前缀(如 `Novel/sub/.hid`)
    pub(crate) fn add(&mut self, rel: &str) {
        if self.paths.len() >= EXCLUDE_PATH_LIMIT || self.chars >= EXCLUDE_ARG_BUDGET {
            self.overflow += 1;
            return;
        }
        self.chars += rel.chars().count() + 1;
        self.paths.push(rel.to_string());
    }

    /// 取收集结果;有溢出时发 warning(N 项未排除但已计入统计)
    pub(crate) fn finish(self, warnings: &mut Vec<String>) -> Vec<String> {
        if self.overflow > 0 {
            warnings.push(format!(
                "隐藏项过多, {} 项未排除(已计入大小)",
                self.overflow
            ));
        }
        let _ = &self.entry_name;
        self.paths
    }
}
```

- [ ] **Step 5: 修两处调用点**

1) `crates/core/src/compress.rs` `compress_with_backend`(约 67 行):

```rust
    let scan = scan_dir(input, &opts.exclude, &|p| {
        let _ = on_progress(p);
    })?;
```

2) `crates/tauri-app/src/lib.rs` `scan` 命令(`spawn_blocking` 内, 本任务先传默认规则, Task 8 接 DTO):

```rust
        "compress" => zip_tool_core::scan_dir(&p, &zip_tool_core::ExcludeRules::default(), &|prog| {
            let _ = on_progress.send(ProgressEvent::from(prog));
        })
        .map(ScanData::Compress),
```

3) 测试调用点同样补规则参数(默认规则, 行为不变):
   - `scan.rs` 既有测试 `scan_lists_sorted_top_level_entries_with_sizes`:`scan_dir(&root.join("in"), &ExcludeRules::default(), &|p| { ... })`(tests 模块已 `use super::*`, `ExcludeRules` 由 Step 1 的 `use crate::{..., ExcludeRules}` 覆盖——注意该 use 在 `mod tests` 内部, 既有测试同模块可用);
   - `compress.rs` `staged_tests::staged_scan_plan_execute_matches_compress_dir` 的 `scan_dir` 调用补 `&ExcludeRules::default()`(该模块加 `use crate::ExcludeRules;`)。

- [ ] **Step 6: 跑测试确认通过**

Run: `cargo test -p zip-tool-core`
Expected: scan 新用例 + 既有编排/分阶段用例全 PASS(Tauri crate 允许暂红, 由 Step 7 验证编译)。

- [ ] **Step 7: 全 workspace 编译绿**

Run: `cargo test --workspace`
Expected: tauri crate 编译通过、测试全绿(Step 5 已适配调用点)。

- [ ] **Step 8: Commit**

```bash
git add -A && git reset -q todo.md
git commit -m "feat(core): scan_dir 排除集成(顶层打标/树内剔除/Windows 属性收集+预算护栏)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 5: compress_dir 顶层丢弃 + Report.excluded(编排测试)

**Files:**
- Modify: `crates/core/src/compress.rs`

- [ ] **Step 1: 写失败测试(fake_tests 模块内)**

```rust
    #[test]
    fn compress_dir_drops_pattern_matched_top_level_entries() {
        let root = unique_temp("orch-exclude");
        fs::create_dir_all(root.join("in/keep")).unwrap();
        fs::create_dir_all(root.join("in/junk")).unwrap();
        fs::write(root.join("in/keep/a.txt"), "a").unwrap();
        fs::write(root.join("in/junk/b.tmp"), "bb").unwrap();

        let mut o = rnd();
        o.exclude.patterns = vec!["*.tmp".into()];
        let events = RefCell::new(Vec::new());
        let report = compress_with_backend(
            &FakeBackend::new(&[]),
            &root.join("in"),
            &root.join("out"),
            &o,
            &CancelFlag::new(),
            &|p| {
                events.borrow_mut().push(p.clone());
                ControlFlow::Continue(())
            },
        )
        .unwrap();

        // 被标顶层被丢弃, 不压缩
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].original, "keep");
        // 报告带出被排除名单
        assert_eq!(report.excluded.len(), 1);
        assert_eq!(report.excluded[0].name, "junk");
        assert_eq!(
            report.excluded[0].reason,
            ExcludeReason::Pattern("*.tmp".into())
        );
        // 进度权重只含保留条目(压缩阶段 total=1, 字节 1)
        let ev = events.borrow();
        let comp: Vec<_> = ev.iter().filter(|p| p.phase == Phase::Compressing).collect();
        assert_eq!(comp.len(), 1);
        assert_eq!((comp[0].done, comp[0].total, comp[0].total_bytes), (1, 1, 1));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn compress_dir_all_excluded_reports_no_entries() {
        let root = unique_temp("orch-excl-all");
        fs::create_dir_all(root.join("in/junk")).unwrap();
        fs::write(root.join("in/junk/b.tmp"), "bb").unwrap();

        let mut o = rnd();
        o.exclude.patterns = vec!["*".into()];
        let err = compress_with_backend(
            &FakeBackend::new(&[]),
            &root.join("in"),
            &root.join("out"),
            &o,
            &CancelFlag::new(),
            &|_| ControlFlow::Continue(()),
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::NoEntries(_)), "全排除 = 无条目可压");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn staged_path_keeps_marked_entries_as_advisory_only() {
        // 分阶段 API(UI): 不做顶层丢弃, 标记仅展示; 勾选名单决定压缩内容
        let root = unique_temp("staged-excl");
        fs::create_dir_all(root.join("in/junk")).unwrap();
        fs::write(root.join("in/junk/b.tmp"), "bb").unwrap();

        let rules = ExcludeRules { include_hidden: true, patterns: vec!["*.tmp".into()] };
        let scan = scan_dir(&root.join("in"), &rules, &|_| {}).unwrap();
        assert_eq!(scan.entries[0].exclude, Some(ExcludeReason::Pattern("*.tmp".into())));

        // 照常 plan + compress_entries(带标条目勾选了就压)
        let mut es = scan.entries;
        plan(&root.join("in"), &root.join("out"), &mut es, &opts()).unwrap();
        let fb = Recorder::new();
        let report = compress_entries_with_backend(
            &fb,
            &root.join("in"),
            &root.join("out"),
            &es,
            &opts(),
            &CancelFlag::new(),
            &noop,
        )
        .unwrap();
        assert_eq!(report.entries.len(), 1);
        assert_eq!(*fb.attempted.borrow(), ["junk"]);
        assert!(report.excluded.is_empty(), "分阶段路径不带出丢弃名单");

        fs::remove_dir_all(&root).unwrap();
    }
```

**放置位置:** 前两个测试(`compress_dir_drops_pattern_matched_top_level_entries`、`compress_dir_all_excluded_reports_no_entries`)放进 `mod fake_tests`(模块内加 `use crate::{ExcludeReason, ExcludeRules};`;该模块已有 `FakeBackend`/`rnd`/`unique_temp`/`Phase`);第三个测试(`staged_path_keeps_marked_entries_as_advisory_only`)放进 `mod staged_tests`(同样加 `use crate::{ExcludeReason, ExcludeRules};`;该模块已有 `scan_dir`/`plan`/`Recorder`/`opts`/`noop`)。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p zip-tool-core compress`
Expected: `Report` 无 `excluded` 字段 → 编译失败;`compress_dir_drops...` 逻辑上也未实现。

- [ ] **Step 3: 实现**

1) `Report` 加字段(`warnings` 之后):

```rust
    /// 被排除的顶层条目(仅 compress_dir 单调用路径丢弃时带出;分阶段路径为空)
    pub excluded: Vec<crate::exclude::ExcludedEntry>,
```

2) `compress_with_backend`(约 66-85 行)在扫描与 plan 之间插入丢弃逻辑, 并回填报告:

```rust
    // Step 1: 扫描(进度 Phase::Scanning;扫描阶段不支持取消, 回调返回值在此忽略)
    let scan = scan_dir(input, &opts.exclude, &|p| {
        let _ = on_progress(p);
    })?;
    // 顶层丢弃(单调用路径):被标条目不压缩。
    // 分阶段 API(UI)不走这里 —— 标记仅展示, 勾选名单决定内容。
    // include_hidden=true 时不会产生 Hidden 标记, Pattern 标记一律丢弃。
    let excluded: Vec<crate::exclude::ExcludedEntry> = scan
        .entries
        .iter()
        .filter(|e| e.exclude.is_some())
        .map(|e| crate::exclude::ExcludedEntry {
            name: e.name.clone(),
            reason: e.exclude.clone().expect("过滤后必有标记"),
        })
        .collect();
    let mut entries: Vec<Entry> = scan
        .entries
        .into_iter()
        .filter(|e| e.exclude.is_none())
        .collect();

    // Step 2/3 经分阶段 API 组合(行为与历史实现逐项一致)
    plan(input, out_dir, &mut entries, opts)?;
    let mut report = compress_entries_with_backend(
        backend,
        input,
        out_dir,
        &entries,
        opts,
        cancel,
        on_progress,
    )?;
    report.warnings = scan.warnings;
    report.excluded = excluded;
    Ok(report)
```

(注意 `Entry` 已在文件 use 列表里;`scan.excluded` 字段与本地的 `excluded` 名单等价, 这里直接从 entries 重建即可, 不必用 `scan.excluded`。)

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p zip-tool-core`
Expected: 全 PASS(含 `orchestration_scans_then_compresses...` 等既有用例——默认规则下无标记, 行为不变)。

- [ ] **Step 5: 全 workspace 回归**

Run: `cargo test --workspace`
Expected: 全绿。

- [ ] **Step 6: Commit**

```bash
git add -A && git reset -q todo.md
git commit -m "feat(core): compress_dir 丢弃被标顶层条目, Report 带出排除名单

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 6: CLI `--exclude` / `--include-hidden-files`

**Files:**
- Modify: `crates/cli/src/main.rs`

- [ ] **Step 1: Compress 子命令加参数(`level` 字段之后)**

```rust
        /// 排除模式(可重复): 不含/ = 任意深度文件名(如 Thumbs.db, *.tmp); 含/ = 从源根锚定的相对路径(如 node_modules/**)
        #[arg(long = "exclude")]
        exclude: Vec<String>,
        /// 不排除隐藏文件(默认排除: Windows=文件系统隐藏属性, Linux/mac=. 开头)
        #[arg(long)]
        include_hidden_files: bool,
```

`Command::Compress { ... }` 解构处同步加 `exclude, include_hidden_files`。

- [ ] **Step 2: 构造 Options 前校验 + 接线**

`main` 的 Compress 分支, 在 `let opts = Options { ... }` 之前加:

```rust
            // 宁停不错: 非法模式在开工前拒绝(exit 2, 与 --split 非法一致)
            if let Err(e) = zip_tool_core::validate_patterns(&exclude) {
                eprintln!("error: invalid --exclude: {e}");
                return ExitCode::from(2);
            }
```

`Options { ... }` 字面量里把 Task 1 的 `exclude: Default::default(),` 替换为:

```rust
                exclude: zip_tool_core::ExcludeRules {
                    include_hidden: include_hidden_files,
                    patterns: exclude,
                },
```

- [ ] **Step 3: 报告打印排除名单**

`Ok(report) =>` 分支, `for e in &report.entries` 之前加:

```rust
                    for x in &report.excluded {
                        eprintln!("excluded ({}): {}", x.reason, x.name);
                    }
```

(`ExcludeReason` 的 `Display` 输出 `hidden` / `pattern "*.tmp"`, CLI 输出英文与现状一致。)

- [ ] **Step 4: 手动验证**

```bash
cargo build -p zip-tool-cli
```

Windows PowerShell(验证默认排除 + include + 模式):

```powershell
$in = "$env:TEMP\zt-cli-check"; rm -r -fo $in -ErrorAction Ignore; mkdir "$in/App" | Out-Null
"v" | Out-File "$in/App/a.txt"; "v" | Out-File "$in/Thumbs.db"
attrib +H "$in/Thumbs.db"
cargo run -p zip-tool-cli -- compress -i $in -o "$env:TEMP\zt-cli-out"
# 期望 stderr: excluded (hidden): Thumbs.db;stdout 只有 App -> ...
cargo run -p zip-tool-cli -- compress -i $in -o "$env:TEMP\zt-cli-out2" --include-hidden-files
# 期望 stdout 两行(App 与 Thumbs.db)
cargo run -p zip-tool-cli -- compress -i $in -o "$env:TEMP\zt-cli-out3" --exclude "a.txt"
# 期望 stderr: excluded (pattern "a.txt"): a.txt
cargo run -p zip-tool-cli -- compress -i $in -o "$env:TEMP\zt-cli-out4" --exclude "/bad/"
# 期望 error: invalid --exclude ... 且退出码 2
```

Linux/macOS 同理用 `.hidden` 名验证。

- [ ] **Step 5: 回归 + Commit**

Run: `cargo test --workspace`
Expected: 全绿。

```bash
git add -A && git reset -q todo.md
git commit -m "feat(cli): --exclude 可重复排除模式 + --include-hidden-files + excluded 提示

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 7: e2e 排除矩阵(实机)

**Files:**
- Modify: `test/tests/cli_e2e.rs`

- [ ] **Step 1: 写测试(文件末尾追加)**

```rust
/// 排除规则 e2e: 预埋隐藏文件与垃圾文件, 验证
/// 1) 默认排除隐藏(Windows=attrib +H, Unix=. 前缀)  2) --include-hidden-files 关闭
/// 3) --exclude 模式(顶层 + 树内)与隐藏集叠加
#[test]
fn compress_exclusion_rules_matrix() {
    let fx = fixture("excl");
    let (inp, out1) = (fx.root.join("in"), fx.root.join("out1"));
    let (out2, out3) = (fx.root.join("out2"), fx.root.join("out3"));

    // 垃圾文件(*.tmp 模式可命中) + 隐藏文件(OS 原生口径)
    fill(&inp.join("Novel/notes.tmp"), 1_000, 3);
    let (hidden_top, hidden_inner) = if cfg!(windows) {
        // Windows 上 . 开头不特殊, 置文件系统隐藏属性
        let q = inp.join(".DS_Store");
        fill(&q, 1_000, 7);
        let st = Command::new("attrib").args(["+H", q.to_str().unwrap()]).status().unwrap();
        assert!(st.success());
        let p = inp.join("Novel/Thumbs.db");
        fill(&p, 1_000, 5);
        let st = Command::new("attrib").args(["+H", p.to_str().unwrap()]).status().unwrap();
        assert!(st.success());
        (q, "Novel/Thumbs.db".to_string())
    } else {
        let q = inp.join(".DS_Store");
        fill(&q, 1_000, 7);
        fill(&inp.join("Novel/.DS_Store"), 1_000, 5);
        (q, "Novel/.DS_Store".to_string())
    };

    // 1) 默认: 隐藏被排除, 其余照常 roundtrip
    let r = run_tool(&["compress", "-i", inp.to_str().unwrap(), "-o", out1.to_str().unwrap()]);
    assert!(r.ok, "压缩失败:\n{}", r.stderr);
    assert!(
        r.stderr.contains("excluded (hidden)"),
        "应打印隐藏排除提示: {}",
        r.stderr
    );
    let restore1 = fx.root.join("restore1");
    for name in ["Novel", "media", "photo.jpg"] {
        extract_archive(&out1.join(format!("{name}.zip")), &restore1, None);
    }
    assert_file_eq(&inp.join("Novel/book.txt"), &restore1.join("Novel/book.txt"));
    assert!(
        !restore1.join(&hidden_top).exists() && !restore1.join(&hidden_inner).exists(),
        "隐藏文件不应进包(顶层与树内)"
    );
    assert!(
        restore1.join("Novel/notes.tmp").exists(),
        "未命中模式的垃圾文件默认保留"
    );

    // 2) --include-hidden-files: 隐藏回来
    let r = run_tool(&[
        "compress", "-i", inp.to_str().unwrap(), "-o", out2.to_str().unwrap(),
        "--include-hidden-files",
    ]);
    assert!(r.ok, "压缩失败:\n{}", r.stderr);
    assert!(
        !r.stderr.contains("excluded"),
        "关闭隐藏排除后不应有排除提示: {}",
        r.stderr
    );
    let restore2 = fx.root.join("restore2");
    for name in ["Novel", "media", "photo.jpg"] {
        extract_archive(&out2.join(format!("{name}.zip")), &restore2, None);
    }
    assert_file_eq(&inp.join(&hidden_top), &restore2.join(&hidden_top));
    assert_file_eq(&inp.join(&hidden_inner), &restore2.join(&hidden_inner));

    // 3) --exclude 与隐藏集叠加: 树内 *.tmp + 顶层 photo.jpg
    let r = run_tool(&[
        "compress", "-i", inp.to_str().unwrap(), "-o", out3.to_str().unwrap(),
        "--exclude", "*.tmp", "--exclude", "photo.jpg",
    ]);
    assert!(r.ok, "压缩失败:\n{}", r.stderr);
    assert!(
        r.stderr.contains("excluded (pattern"),
        "应打印模式排除提示: {}",
        r.stderr
    );
    assert!(!out3.join("photo.jpg.zip").exists(), "顶层命中模式的条目不产出包");
    let restore3 = fx.root.join("restore3");
    for name in ["Novel", "media"] {
        extract_archive(&out3.join(format!("{name}.zip")), &restore3, None);
    }
    assert!(
        !restore3.join("Novel/notes.tmp").exists(),
        "*.tmp 应被模式排除"
    );
    assert_file_eq(&inp.join("media/clip.bin"), &restore3.join("media/clip.bin"));
    assert_file_eq(&inp.join("Novel/book.txt"), &restore3.join("Novel/book.txt"));
}
```

- [ ] **Step 2: 跑 e2e**

Run: `cargo test -p zip-tool-e2e`
Expected: 全 PASS(含既有 roundtrip 用例——它们不经排除开关, 默认规则下夹具无隐藏文件, 行为不变)。

- [ ] **Step 3: Commit**

```bash
git add -A && git reset -q todo.md
git commit -m "e2e: 排除规则矩阵(默认隐藏排除 / --include-hidden-files / --exclude 叠加)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 8: tauri DTO(excludes 透传 + 规则保鲜)

**Files:**
- Modify: `crates/tauri-app/src/lib.rs`

- [ ] **Step 1: 写失败测试(tests 模块内)**

1) 修既有助手(Task 1 已补字段), 现在补规则相关:

```rust
    fn compress_ctx(src: &str) -> ScanContext {
        ScanContext {
            mode: "compress",
            src: PathBuf::from(src),
            rules: ExcludeRules::default(),
            data: ScanData::Compress(ScanReport {
                entries: vec![
                    entry("A", 1, EntryKind::Dir),
                    entry("b.txt", 2, EntryKind::File),
                ],
                total_bytes: 3,
                warnings: vec!["warn1".into()],
                excluded: Vec::new(),
            }),
        }
    }
```

(顶部 `use zip_tool_core::{...}` 增加 `ExcludeRules`;`select_compress_filters_by_names_and_rejects_mismatch` 的 5 处 `select_compress` 调用补第 4 参 `&ExcludeRules::default()`;`select_extract` 调用不变——它没有规则参。既有测试 `to_core_options_maps_format_and_level` 的 `base()` 闭包也补一行 `excludes: None,`, 否则编译失败。)

2) 新测试:

```rust
    #[test]
    fn to_core_options_validates_and_carries_excludes() {
        let base = || CompressOptions {
            split_size: None,
            password: None,
            random_name: false,
            format: None,
            level: None,
            excludes: Some(vec!["Thumbs.db".into(), "sub/*.log".into()]),
        };
        let core = to_core_options(base()).unwrap();
        assert_eq!(core.exclude.patterns.len(), 2);
        assert!(!core.exclude.include_hidden, "UI 恒定排除隐藏");

        // 非法模式拒绝
        let mut o = base();
        o.excludes = Some(vec!["a/**b".into()]);
        assert!(to_core_options(o).is_err());

        // 缺省 = 无用户模式
        let mut o = base();
        o.excludes = None;
        assert!(to_core_options(o).unwrap().exclude.patterns.is_empty());
    }

    #[test]
    fn select_compress_rejects_rule_mismatch_as_expired() {
        let ctx = compress_ctx("C:\\s");
        let rules = ExcludeRules::default();
        // 规则一致 → 正常
        assert!(select_compress(Some(&ctx), "C:\\s", &s(&["A"]), &rules).is_ok());
        // 规则不一致 = 扫描过期(5.4 语义扩展)
        let other = ExcludeRules { include_hidden: false, patterns: vec!["x".into()] };
        let err = select_compress(Some(&ctx), "C:\\s", &s(&["A"]), &other).unwrap_err();
        assert!(err.contains("过期"), "{err}");
    }

    #[test]
    fn compress_scan_dto_marks_excluded_items() {
        let mut a = entry("A", 1, EntryKind::Dir);
        a.exclude = Some(zip_tool_core::ExcludeReason::Hidden);
        let mut b = entry("b.txt", 2, EntryKind::File);
        b.exclude = Some(zip_tool_core::ExcludeReason::Pattern("*.log".into()));
        let report = ScanReport {
            entries: vec![a, b],
            total_bytes: 0,
            warnings: Vec::new(),
            excluded: Vec::new(),
        };
        let dto = compress_scan_dto(&report);
        assert_eq!(dto.items[0].excluded.as_deref(), Some("隐藏文件"));
        assert_eq!(
            dto.items[1].excluded.as_deref(),
            Some("匹配排除模式: *.log")
        );
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p zip-tool-tauri`
Expected: 编译失败——`ScanContext` 无 `rules` 字段、`CompressOptions` 无 `excludes` 字段、`select_compress` 参数数不符。

- [ ] **Step 3: 实现**

1) import 区(`use zip_tool_core::{...}`)加 `ExcludeReason, ExcludeRules`。

2) `ScanContext` 加字段:

```rust
/// scan 命令缓存的扫描结果(mode + src + 规则匹配才可用)
struct ScanContext {
    mode: &'static str,
    src: PathBuf,
    /// 扫描时的排除规则(compress 模式;确认时比对, 不一致 = 过期)
    rules: ExcludeRules,
    data: ScanData,
}
```

3) `ScanItem` 加字段(`detail` 之后):

```rust
    /// compress: 排除原因(「隐藏文件」/「匹配排除模式: <p>」);行显示但默认不勾选
    excluded: Option<String>,
```

4) `CompressOptions` 加字段:

```rust
    /// 排除模式(高级区输入);None = 无用户模式
    excludes: Option<Vec<String>>,
```

5) `to_core_options` 里 `Ok(Options { ... })` 改为:

```rust
    // 宁停不错: 用户模式非法即拒(与 CLI exit 2 同语义)
    let patterns = match o.excludes {
        None => Vec::new(),
        Some(v) => match zip_tool_core::validate_patterns(&v) {
            Ok(()) => v,
            Err(e) => return Err(format!("排除模式无效: {e}")),
        },
    };
    Ok(Options {
        split_size,
        password: non_empty(o.password),
        random_name: o.random_name,
        format,
        level,
        exclude: ExcludeRules {
            // UI 恒定排除隐藏(被标行显示不勾选; 勾选了也尊重用户)
            include_hidden: false,
            patterns,
        },
    })
```

6) `select_compress` 加规则比对(签名加 `rules: &ExcludeRules`):

```rust
fn select_compress(
    scan: Option<&ScanContext>,
    src: &str,
    entries: &[String],
    rules: &ExcludeRules,
) -> Result<Vec<Entry>, String> {
    let ctx = match scan {
        Some(c)
            if c.mode == "compress"
                && c.src == Path::new(src)
                && rules_eq(&c.rules, rules) =>
        {
            c
        }
        _ => return Err(EXPIRED.into()),
    };
    // ……其余不变……
}

/// 规则等价: 开关 + 模式逐条相等(顺序敏感, UI 透传顺序即扫描顺序)
fn rules_eq(a: &ExcludeRules, b: &ExcludeRules) -> bool {
    a.include_hidden == b.include_hidden && a.patterns == b.patterns
}
```

7) `compress_scan_dto` 的 items map 里加 `excluded` 字段, 并加原因文案函数:

```rust
/// 排除原因的前端文案(ScanItem.excluded)
fn excluded_text(r: &ExcludeReason) -> String {
    match r {
        ExcludeReason::Hidden => "隐藏文件".into(),
        ExcludeReason::Pattern(p) => format!("匹配排除模式: {p}"),
    }
}
```

`compress_scan_dto` 的每个 `ScanItem` 补:

```rust
                excluded: e.exclude.as_ref().map(excluded_text),
```

`extract_scan_dto` 的所有 `ScanItem`(组行/meta/warning)补 `excluded: None,`。

8) `scan` 命令:签名加参数(放在 `path` 之后)、校验并构造规则、传给 scan_dir、缓存进 context:

```rust
#[tauri::command]
async fn scan(
    mode: String,
    path: String,
    excludes: Option<Vec<String>>,
    on_progress: Channel<ProgressEvent>,
    state: State<'_, AppState>,
) -> Result<ScanResult, String> {
    let mode: &'static str = match mode.as_str() {
        "compress" => "compress",
        "extract" => "extract",
        other => return Err(format!("未知模式: {other}")),
    };
    // 宁停不错: 非法模式在占忙之前拒绝
    if let Some(pats) = &excludes
        && let Err(e) = zip_tool_core::validate_patterns(pats)
    {
        return Err(format!("排除模式无效: {e}"));
    }
    // UI 恒定排除隐藏(被标行显示不勾选)
    let rules = ExcludeRules {
        include_hidden: false,
        patterns: excludes.unwrap_or_default(),
    };
    let p = PathBuf::from(&path);
    let src_for_ctx = p.clone();
    acquire_busy(&state.busy, "scan")?;

    // rules 既要进 move 闭包又要进缓存, 先克隆一份给闭包
    let rules_for_scan = rules.clone();
    let result = tauri::async_runtime::spawn_blocking(move || match mode {
        "compress" => zip_tool_core::scan_dir(&p, &rules_for_scan, &|prog| {
            let _ = on_progress.send(ProgressEvent::from(prog));
        })
        .map(ScanData::Compress),
        _ => zip_tool_core::scan_groups(&p).map(ScanData::Extract),
    })
    // ……余下不变……
```

缓存处 `ScanContext { mode, src: src_for_ctx, data }` 改为:

```rust
    *state.scan.lock().map_err(|_| POISONED.to_string())? = Some(ScanContext {
        mode,
        src: src_for_ctx,
        rules,
        data,
    });
```

9) `compress` 命令:`select_compress` 调用补 `&opts.exclude`:

```rust
        select_compress(scan.as_ref(), &src, &entries, &opts.exclude)
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p zip-tool-tauri && cargo test --workspace`
Expected: 全 PASS/全绿。

- [ ] **Step 5: Commit**

```bash
git add -A && git reset -q todo.md
git commit -m "tauri: scan/compress DTO 透传排除模式, 规则不一致按扫描过期拒绝

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 9: UI(排除输入 + 被标行默认不勾选)

**Files:**
- Modify: `crates/tauri-app/test/ui.test.mjs`(先写失败测试)
- Modify: `crates/tauri-app/ui/index.html`
- Modify: `crates/tauri-app/ui/app.js`
- Regenerate: `crates/tauri-app/ui/app.css`(`npm run css`)

- [ ] **Step 1: 写失败测试(ui.test.mjs 末尾追加)**

先修既有断言(选项形状多了 `excludes`;mock 表格多了被标行):
- "契约: 平衡预设的参数透传到 compress 调用" 与 "行为: 压缩全流程…" 两处 `jsonEq(comp.args.options, {...})` 各加一行 `excludes: [],`。
- "浏览器预览(无 Tauri)" 用例整体替换为(mock 压缩表变 5 行, 后两行是被标行、默认不勾选、不参与 mock 运行):

```js
test("浏览器预览(无 Tauri): mock 扫描与 mock 运行仍可用", async () => {
  const h = makeHarness(false, () => {
    throw new Error("无 Tauri 环境不应触碰 IPC");
  });
  await pickSource(h);
  assert.deepEqual(
    h.badges(),
    ["待压缩", "待压缩", "待压缩", "待压缩", "待压缩"],
    "mock 含隐藏/命中行(显示不勾选)"
  );
  const boxes = h.rows().map((tr) => tr.querySelector("input"));
  assert.ok(!boxes[3].checked && !boxes[4].checked, "被标行默认不勾选");
  assert.ok(h.doc.getElementById("msg").textContent.includes("浏览器预览假数据"));
  assert.equal(h.doc.getElementById("ver").textContent, "dev");

  h.doc.getElementById("dst").click();
  await h.flush();
  h.doc.getElementById("confirm").click();
  // mock 每 400ms 一条进度; 勾选名单 = 前 3 行(被标行未勾选), 3 条约 1.2s
  await new Promise((r) => setTimeout(r, 500));
  const mid = h.badges();
  await new Promise((r) => setTimeout(r, 1600));
  assert.deepEqual(mid, ["完成", "进行中", "待压缩", "待压缩", "待压缩"]);
  assert.deepEqual(h.badges(), ["完成", "完成", "完成", "待压缩", "待压缩"]);
  assert.ok(h.doc.getElementById("msg").textContent.includes("压缩完成: 3 个压缩包"));
});
```

再追加新用例(独立 fixture 常量, 不污染 SCAN_COMPRESS):

```js
// 带排除标记的扫描结果(与壳层 DTO 的 excluded 字段一致)
const SCAN_EXCLUDED = {
  mode: "compress",
  items: [
    ITEM("Novel", "dir", 2_600_000_000, "pending"),
    ITEM(".DS_Store", "file", 4096, "pending", { excluded: "隐藏文件" }),
    ITEM("cache", "dir", 12_000, "pending", { excluded: "匹配排除模式: cache" }),
  ],
  warnings: [],
  totalBytes: 2_600_004_096,
};

test("行为: 排除行显示但不勾选, 模式输入透传 scan/compress, 勾选可覆盖", async () => {
  const h = makeHarness(true, ({ cmd, args }) => {
    if (cmd === "plugin:dialog|open")
      return args.options.title?.includes("目标") ? "C:\\out" : "C:\\demo\\data";
    if (cmd === "scan") {
      assert.deepEqual(args.excludes, ["Thumbs.db", "*.tmp"], "扫描应携带解析后的排除模式");
      return SCAN_EXCLUDED;
    }
    if (cmd === "compress") return { cancelled: false, entries: [], warnings: [] };
    return null;
  });

  // 高级区填排除模式(换行/逗号混排, 带空白)再扫描
  h.doc.getElementById("opt-advanced").click();
  h.doc.getElementById("opt-exclude").value = " Thumbs.db,\n*.tmp ";
  await pickSource(h);

  // 被标行显示(待压缩徽章)但默认不勾选; 名字悬停看原因
  assert.deepEqual(h.badges(), ["待压缩", "待压缩", "待压缩"]);
  const boxes = h.rows().map((tr) => tr.querySelector("input"));
  assert.ok(boxes[0].checked, "普通行默认勾选");
  assert.ok(!boxes[1].checked, "隐藏行默认不勾选");
  assert.ok(!boxes[2].checked, "模式命中行默认不勾选");
  assert.match(h.rows()[1].querySelector("td:nth-child(2)").title, /隐藏文件/);
  assert.match(h.rows()[2].querySelector("td:nth-child(2)").title, /匹配排除模式: cache/);

  // 全选切换覆盖所有 pending 行(含被标行); 手动勾选了就会被压缩
  h.doc.getElementById("check-all").click();
  assert.ok(h.rows().every((tr) => tr.querySelector("input").checked));

  h.doc.getElementById("dst").click();
  await h.flush();
  await h.flush();
  h.doc.getElementById("confirm").click();
  await h.flush();
  await h.flush();
  const comp = h.calls.find((c) => c.cmd === "compress");
  assert.ok(comp, "应调用 compress");
  jsonEq(comp.args.entries, ["Novel", ".DS_Store", "cache"]);
  jsonEq(comp.args.options.excludes, ["Thumbs.db", "*.tmp"]);
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd crates/tauri-app && npm test`
Expected: 新用例失败——`args.excludes` 为 undefined、被标行默认勾选;既有两处 options 形状断言也红。

- [ ] **Step 3: index.html 加排除输入**

`opt-level-wrap` 结束(`</span>`, 约 130 行)之后、`opt-force-wrap` 之前插入:

```html
      <span id="opt-exclude-wrap" class="hidden items-center gap-1.5">
        <label for="opt-exclude" class="shrink-0">排除</label>
        <input
          id="opt-exclude"
          class="w-64 px-2 py-1 rounded-md border border-slate-300 text-slate-700 focus:outline-none focus:border-blue-400"
          placeholder="逗号分隔, 如 Thumbs.db, *.tmp"
        />
      </span>
```

- [ ] **Step 4: app.js 四处修改**

1) `LOCK_IDS` 数组加 `"opt-exclude"`。

2) `opt-advanced` 的 change 处理器列表改为:

```js
  for (const id of ["opt-format-wrap", "opt-level-wrap", "opt-exclude-wrap"]) {
```

3) `doScan` 的 invoke 改为(压缩模式才带 excludes):

```js
    const args = { mode: state.mode, path, onProgress: makeChannel() };
    if (state.mode === "compress") args.excludes = parseExcludes();
    const res = await invoke("scan", args);
```

并在 `doScan` 之前加:

```js
// 排除模式输入: 换行/逗号分隔, 去空白, 空段剔除(高级区)
function parseExcludes() {
  return $("opt-exclude").value
    .split(/[\n,]/)
    .map((s) => s.trim())
    .filter(Boolean);
}
```

4) `renderTable` 默认勾选行加排除条件:

```js
  state.rows = items.map((it, i) => ({
    ...it,
    checked: state.rows[i]?.checked ?? (it.status === "pending" && !it.excluded),
  }));
```

名字 tooltip 把排除原因并入(在 `const tip = ...` 行):

```js
    const tip = [r.excluded, r.original ? `原名 ${r.original}` : null, r.detail]
      .filter(Boolean)
      .join(", ");
```

5) 确认执行的压缩 options 加(在 `level: ...` 之后):

```js
        excludes: parseExcludes(),
```

6) `mockScan` 的 compress 分支加两个被标行(浏览器预览直观可见):

```js
        { name: ".DS_Store", kind: "file", size: 4096, status: "pending", original: null, detail: null, excluded: "隐藏文件" },
        { name: "cache", kind: "dir", size: 12_000, status: "pending", original: null, detail: null, excluded: "匹配排除模式: cache" },
```

- [ ] **Step 5: 跑测试**

Run: `cd crates/tauri-app && npm test`
Expected: 新旧用例全 PASS。

- [ ] **Step 6: 重新生成 CSS**

Run: `cd crates/tauri-app && npm run css`
Expected: `ui/app.css` 更新(git status 可见), 新类 `w-64` 等进入产物。

- [ ] **Step 7: Commit**

```bash
git add -A && git reset -q todo.md
git commit -m "ui: 排除模式输入(高级区) + 隐藏/命中行显示不勾选 + mock 同步

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 10: 文档同步 + 全量回归

**Files:**
- Modify: `_docs/compress.md`
- Modify: `README.md`
- Modify: `_docs/ui-backend.md`

- [ ] **Step 1: compress.md 加"排除规则"节**

在 `## 7. 进度与取消`(约 217 行)之前插入:

```markdown
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
```

- [ ] **Step 2: README.md 更新**

1) 支持列表(第 4 条后)加:

```markdown
5. 排除规则: 默认排除各 OS 隐藏文件(Windows=隐藏属性, Linux/mac=`.` 开头), `--exclude` 自定义模式可叠加
```

2) "构建 CLI" 的示例代码块(`zip-tool doctor` 行之前)加:

```
# 排除规则: --exclude 可重复(不含/ = 任意深度文件名, 含/ = 从源根锚定路径)
zip-tool compress -i D:\data -o E:\out --exclude Thumbs.db --exclude "*.tmp" --exclude node_modules/**

# 默认排除隐藏文件(Windows=隐藏属性, Linux/mac=. 开头); --include-hidden-files 关闭
zip-tool compress -i D:\data -o E:\out --include-hidden-files
```

3) 环境要求小节不动(排除不引入新工具依赖)。

- [ ] **Step 3: ui-backend.md 更新**

1) §4 数据结构: `struct ScanItem { ... }` 一行加 `excluded: Option<String>`, 并在其后加注释行:

```markdown
  // compress: 排除原因(「隐藏文件」/「匹配排除模式: <p>」); 该行显示但默认不勾选
```

`CompressOptions` DTO 描述处加 `excludes?: string[]`; §3.1 scan 的参数描述加 `excludes?: string[]`(可选, 非法模式返回错误字符串)。

2) §5.4 保鲜: 在 mode/src 校验描述后补一句:

```markdown
compress 模式还比对扫描时的排除规则(excludes), 与本次 compress 传参不一致同样按过期拒绝 —— 修改模式输入后必须重扫。
```

3) §8 选项控件表加一行:

```markdown
| 排除模式输入(高级区) | 多行/逗号分隔; scan 与 compress 都透传 `excludes`(恒定 `include_hidden=false`); 被标行默认不勾选, 可手动勾选 |
```

4) §11 测试策略的"壳层单测"行补: `to_core_options 模式校验; 规则不一致 = 过期; excluded 文案映射`。

- [ ] **Step 4: 全量回归**

Run: `cargo test --workspace && cd crates/tauri-app && npm test`
Expected: 全绿(core / cli / tauri / e2e + UI node tests)。

- [ ] **Step 5: Commit**

```bash
git add -A && git reset -q todo.md
git commit -m "docs: 同步排除规则至 compress/README/ui-backend(语法/归一化/双平台/DTO)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## 完成后(计划外, 提醒用户)

- GUI 真机走查: `cargo run -p zip-tool-tauri`, 验证扫描含隐藏文件时被标行默认不勾选、排除输入生效、改输入后确认提示"扫描已过期"。
- `todo.md` 的排除规则条目由用户自行勾销(该文件永不入库)。
