# 压缩格式双模式(format + level)实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 依据 `_docs/compress-modes.md` 规格,为压缩引入 Format(zip/7z)与 level(0-9)两个底层参数;解压侧支持 `.7z` 系;CLI 加 `--format/--level`;UI 加两个模式预设(最大兼容性/平衡)+ 高级微调;doctor 扩为平台工具全集。

**Architecture:** "模式"只存在于 UI 层(预设 = 参数组合);core 只有 `Format` 枚举(决定工具/参数/扩展名/加密方式)+ `Options.level`。压缩编排按格式定产物扩展名、预检与清理模式;解压按扩展名分发后端(`.7z` 系强制 7z)。全程 TDD,每任务结束 workspace 可编译、测试全绿、独立提交。

**Tech Stack:** Rust workspace(crates/core + cli + tauri-app)、clap 4、Tauri 2、Node --test + jsdom、cargo test E2E(系统 7z/NanaZip)。

**测试基线:** 当前 workspace 全绿(约 86 测试 + UI 7 用例)。每任务完成条件 = `cargo test --workspace` 全绿 + (涉及 UI 时)`cd crates/tauri-app && npm test` 全绿。

---

## 文件结构

| 文件 | 动作 | 职责 |
|---|---|---|
| `crates/core/src/backend.rs` | 修改 | `Format` 枚举(extension/backend)、`compress_command` 按格式/级别出参、`ensure_tool_available`(原 locate_extract_tool 改名复用) |
| `crates/core/src/lib.rs` | 修改 | `Options` 加 `format`/`level`、导出 `Format`、`UnsupportedArchive` 文案、crate 文档 |
| `crates/core/src/scan.rs` | 修改 | `Entry.format` 扫描后为空串(格式由 plan 填充) |
| `crates/core/src/compress.rs` | 修改 | plan/预检/清理/主卷解析按格式定扩展名;公共入口按格式选后端 + 工具预检 |
| `crates/core/src/extract.rs` | 修改 | `resolve_archive` 加 `.7z`;`scan_groups` 分类 7z 组 |
| `crates/core/src/doctor.rs` | 修改 | `platform_tools`/`check_platform_tools`(平台工具全集);删 `check_backend_tool` |
| `crates/cli/src/main.rs` | 修改 | `--format`/`--level` 参数;doctor 遍历平台工具 |
| `test/tests/cli_e2e.rs` | 修改 | 7z 格式 roundtrip 用例;`is_random_artifact`/解压辅助支持 .7z |
| `crates/tauri-app/src/lib.rs` | 修改 | `CompressOptions` DTO 加 format/level + `to_core_options` 映射 |
| `crates/tauri-app/ui/index.html` | 修改 | 预设分段按钮、高级微调(格式/强度)、密码提示 |
| `crates/tauri-app/ui/app.js` | 修改 | 预设逻辑、参数透传、提示联动 |
| `crates/tauri-app/test/ui.test.mjs` | 修改 | 预设/提示/透传用例;既有 options 断言更新 |
| `crates/tauri-app/ui/app.css` | 重新生成 | `npm run css` |
| `_docs/compress.md` `extract.md` `ui-backend.md` `README.md` `compress-modes.md` | 修改 | 文档同步(任务 10) |

---

### Task 1: core — `Format` 枚举与 `Options.format/level`

**Files:**
- Modify: `crates/core/src/backend.rs`
- Modify: `crates/core/src/lib.rs:191-213`(Options + 默认值测试)
- Modify: `crates/core/src/compress.rs`(测试辅助函数的 Options 字面量)
- Modify: `crates/cli/src/main.rs:76-80`(构造点补字段)
- Modify: `crates/tauri-app/src/lib.rs:379-383`(构造点补字段)

- [ ] **Step 1: 写失败测试**

`crates/core/src/backend.rs` 测试模块 `mod tests`(有 `use super::*;` 与 `args_of` 辅助)内新增:

```rust
#[test]
fn format_maps_extension_and_backend() {
    assert_eq!(Format::Zip.extension(), "zip");
    assert_eq!(Format::SevenZ.extension(), "7z");
    // Zip 格式跟随平台原生工具(Windows 上 zip 格式也由 7z -tzip 产出);
    // SevenZ 格式两平台统一 7z
    assert_eq!(Format::Zip.backend(), Backend::native());
    assert_eq!(Format::SevenZ.backend(), Backend::SevenZip);
}
```

`crates/core/src/lib.rs` 测试模块内,把 `default_options_do_not_split_or_encrypt` 改为:

```rust
    #[test]
    fn default_options_do_not_split_or_encrypt() {
        let opts = Options::default();
        assert!(opts.split_size.is_none());
        assert!(opts.password.is_none());
        assert!(!opts.random_name);
        assert_eq!(opts.format, Format::Zip, "默认 zip(最大兼容)");
        assert!(opts.level.is_none(), "默认不传级别, 用后端默认");
    }
```

- [ ] **Step 2: 运行确认编译失败**

Run: `cargo test -p zip-tool-core format_maps_extension`
Expected: 编译错误(`Format` 未定义 / `opts.format` 字段不存在)

- [ ] **Step 3: 实现**

`crates/core/src/backend.rs` 在 `pub enum Backend` 定义之前插入:

```rust
/// 压缩包格式:决定外部工具、命令参数、产物扩展名与加密方式。
/// zip = Deflate(兼容性最大);7z = LZMA2(压缩率更高, 需 7z 系工具解压)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Format {
    /// zip 格式:Linux 走系统 zip 命令,Windows 走 7z -tzip
    #[default]
    Zip,
    /// 7z 格式:两平台都走 7z 命令(-t7z -m0=LZMA2)
    SevenZ,
}

impl Format {
    /// 产物扩展名(.zip / .7z)
    pub fn extension(&self) -> &'static str {
        match self {
            Format::Zip => "zip",
            Format::SevenZ => "7z",
        }
    }

    /// 格式对应的外部工具:Zip 格式用平台原生;SevenZ 格式两平台统一 7z
    pub fn backend(&self) -> Backend {
        match self {
            Format::Zip => Backend::native(),
            Format::SevenZ => Backend::SevenZip,
        }
    }
}
```

`crates/core/src/lib.rs` 的 `Options` 改为(保留原字段):

```rust
/// 压缩选项
#[derive(Debug, Default, Clone)]
pub struct Options {
    /// 分包大小(字节);`None` 表示不分包
    pub split_size: Option<u64>,
    /// 压缩密码;`None` 表示不加密
    pub password: Option<String>,
    /// 是否使用随机包名,避免包名本身泄露敏感信息
    pub random_name: bool,
    /// 压缩包格式,默认 zip(最大兼容)
    pub format: Format,
    /// 压缩级别 0-9(0=仅存储);`None` = 不传级别参数,用后端默认
    pub level: Option<u8>,
}
```

同时把"压缩选项(骨架占位,字段随实现调整)"的旧文档注释替换为上面的 `/// 压缩选项`。

`crates/core/src/lib.rs` 导出:`pub use backend::parse_size;` 改为 `pub use backend::{Format, parse_size};`。

crate 顶部文档(约 9-22 行)中两处过期描述更新:
- `- Windows: \`7z\` 命令(Windows 无系统 zip 命令)` 之后补一行 `//! - 格式维度:zip(最大兼容)与 7z(LZMA2),见 backend::Format 与 _docs/compress-modes.md`
- `- 两侧输出均为 zip 格式,解压/合并由 \`extract\` 承担。` 改为 `- 输出格式由 Options.format 决定(zip / 7z),解压/合并由 \`extract\` 承担。`

- [ ] **Step 4: 修复全部 Options 字面量构造点(workspace 编译通过)**

统一用 `..Options::default()` 兜底(默认 = zip + level None):

`crates/core/src/backend.rs` `mod tests` 的 `opts`:
```rust
    fn opts(split: Option<u64>, password: Option<&str>) -> Options {
        Options {
            split_size: split,
            password: password.map(String::from),
            random_name: false,
            ..Options::default()
        }
    }
```

`crates/core/src/compress.rs` 三处同样处理:
- `mod tests` 的 `fn opts(split: Option<u64>)`(约 436 行)
- `mod fake_tests` 的 `fn rnd()`(约 558 行)
- `mod staged_tests` 的 `fn opts()`(约 774 行)

`crates/cli/src/main.rs` compress 分支的 `Options { ... }`(约 76-80 行)补:
```rust
            let opts = Options {
                split_size,
                password,
                random_name,
                format: zip_tool_core::Format::Zip,
                level: None,
            };
```

`crates/tauri-app/src/lib.rs` compress 命令的 `Options { ... }`(约 379-383 行)补:
```rust
    let opts = Options {
        split_size,
        password: non_empty(options.password),
        random_name: options.random_name,
        format: zip_tool_core::Format::Zip,
        level: None,
    };
```

- [ ] **Step 5: 全量测试**

Run: `cargo test --workspace`
Expected: 全绿(含新增 2 个用例)

- [ ] **Step 6: 提交**

```bash
git add -A
git commit -m "core: Format 枚举与 Options.format/level 参数(默认 zip)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 2: core — `compress_command` 按格式/级别出参

**Files:**
- Modify: `crates/core/src/backend.rs:100-138`(compress_command)

- [ ] **Step 1: 写失败测试**

`crates/core/src/backend.rs` `mod tests` 新增(保留既有 `opts` 辅助不动):

```rust
    fn opts_fmt(format: Format, level: Option<u8>) -> Options {
        Options {
            split_size: None,
            password: None,
            random_name: false,
            format,
            level,
        }
    }

    #[test]
    fn sevenzip_command_7z_format_emits_lzma2() {
        let cmd = Backend::SevenZip
            .compress_command(
                Path::new("D:/data"),
                "A",
                Path::new("E:/out/A.7z"),
                &opts_fmt(Format::SevenZ, None),
            )
            .unwrap();
        assert_eq!(cmd.get_program(), "7z");
        assert_eq!(
            args_of(&cmd),
            ["a", "-t7z", "-m0=LZMA2", "--", "E:/out/A.7z", "A"]
        );
        assert_eq!(cmd.get_current_dir(), Some(Path::new("D:/data")));
    }

    #[test]
    fn sevenzip_command_7z_format_with_split_password_level() {
        let mut o = opts_fmt(Format::SevenZ, Some(9));
        o.split_size = Some(2 * 1024 * 1024 * 1024);
        o.password = Some("pw".into());
        let cmd = Backend::SevenZip
            .compress_command(Path::new("D:/data"), "B", Path::new("E:/out/B.7z"), &o)
            .unwrap();
        assert_eq!(
            args_of(&cmd),
            [
                "a", "-t7z", "-m0=LZMA2", "-v2g", "-ppw", "-mx=9", "--", "E:/out/B.7z", "B"
            ]
        );
    }

    #[test]
    fn sevenzip_command_zip_format_level_uses_mx() {
        let cmd = Backend::SevenZip
            .compress_command(
                Path::new("D:/data"),
                "A",
                Path::new("E:/out/A.zip"),
                &opts_fmt(Format::Zip, Some(9)),
            )
            .unwrap();
        assert_eq!(
            args_of(&cmd),
            ["a", "-tzip", "-mx=9", "--", "E:/out/A.zip", "A"]
        );
    }

    #[test]
    fn zip_command_level_uses_digit_flag() {
        // Info-ZIP zip 的级别 flag 就是数字本身(-0 仅存储 … -9 最大)
        let cmd = Backend::Zip
            .compress_command(
                Path::new("/in"),
                "A",
                Path::new("/out/A.zip"),
                &opts_fmt(Format::Zip, Some(9)),
            )
            .unwrap();
        assert_eq!(args_of(&cmd), ["-r", "-9", "/out/A.zip", "A"]);
    }

    #[test]
    fn level_zero_means_store() {
        let cmd = Backend::Zip
            .compress_command(
                Path::new("/in"),
                "A",
                Path::new("/out/A.zip"),
                &opts_fmt(Format::Zip, Some(0)),
            )
            .unwrap();
        assert_eq!(args_of(&cmd), ["-r", "-0", "/out/A.zip", "A"]);
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p zip-tool-core sevenzip_command_7z zip_command_level level_zero`
Expected: 新用例 FAIL(参数缺 `-t7z`/级别),既有用例仍 PASS

- [ ] **Step 3: 实现 compress_command**

`Backend::compress_command` 两个分支改为(仅展示需要改的部分;参数名 `out_zip` 与 `--`/相对条目名规则不变):

```rust
            Backend::Zip => {
                cmd.arg("-r");
                if let Some(size) = opts.split_size {
                    cmd.arg("-s").arg(format_split_size(size));
                }
                if let Some(pw) = &opts.password {
                    cmd.arg("-P").arg(pw);
                }
                // 级别:Info-ZIP 的级别 flag 就是数字本身(-0 仅存储 … -9 最大)
                if let Some(n) = opts.level {
                    cmd.arg(format!("-{n}"));
                }
                // zip 无 -- 分隔符, "-" 开头的名字 ./ 化防被当参数
                let src = if name.starts_with('-') {
                    format!("./{name}")
                } else {
                    name.to_string()
                };
                cmd.arg(out_zip).arg(src);
            }
            Backend::SevenZip => {
                cmd.arg("a");
                // 格式由 Options.format 决定:SevenZip 后端可产 zip 与 7z 两种格式
                match opts.format {
                    Format::Zip => cmd.arg("-tzip"),
                    // 显式 -m0=LZMA2:防旧版 p7zip 默认 LZMA1, 保证两平台算法一致
                    Format::SevenZ => cmd.arg("-t7z").arg("-m0=LZMA2"),
                };
                if let Some(size) = opts.split_size {
                    cmd.arg(format!("-v{}", format_split_size(size)));
                }
                if let Some(pw) = &opts.password {
                    cmd.arg(format!("-p{pw}"));
                }
                if let Some(n) = opts.level {
                    cmd.arg(format!("-mx={n}"));
                }
                cmd.arg("--").arg(out_zip).arg(name);
            }
```

- [ ] **Step 4: 全量测试**

Run: `cargo test --workspace`
Expected: 全绿

- [ ] **Step 5: 提交**

```bash
git add crates/core/src/backend.rs
git commit -m "core: compress_command 按格式/级别出参(zip -N / 7z -t7z -m0=LZMA2 -mx=N)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 3: core — 编排按格式定扩展名 + 工具缺失预检

**Files:**
- Modify: `crates/core/src/scan.rs`(Entry.format 置空)
- Modify: `crates/core/src/compress.rs`(plan/cleanup/primary/target_exists/公共入口)
- Modify: `crates/core/src/backend.rs`(locate_extract_tool → ensure_tool_available)
- Modify: `crates/core/src/lib.rs`(ToolMissing 文档注释)

- [ ] **Step 1: 写失败测试**

`crates/core/src/scan.rs` 测试内,`assert_eq!(report.entries[0].format, "zip");` 改为:

```rust
        // dst 与 format 均由 Step 2 填充, 扫描不感知格式
        assert_eq!(report.entries[0].dst, "");
        assert_eq!(report.entries[0].format, "");
```

(原来的 `assert_eq!(report.entries[0].dst, "");` 与 `assert_eq!(report.entries[0].format, "zip");` 两行合并为上面三行。)

`crates/core/src/compress.rs` `mod dst_tests` 新增(`entries` 辅助的 `format` 字段在 Step 3 一并改为 `String::new()`):

```rust
    #[test]
    fn assign_dsts_fills_format_from_extension() {
        let mut es = entries(&["A"]);
        assign_dsts(&mut es, Path::new("out"), false, "7z").unwrap();
        assert_eq!(es[0].dst, "A");
        assert_eq!(es[0].format, "7z");
    }

    #[test]
    fn assign_dsts_rejects_existing_7z_target() {
        let root = crate::testutil::unique_temp("dst-exists-7z");
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("out/A.7z"), b"old").unwrap();

        let mut es = entries(&["A"]);
        let err = assign_dsts(&mut es, &root.join("out"), false, "7z").unwrap_err();
        assert!(matches!(err, CoreError::OutputExists(_)));

        std::fs::remove_dir_all(&root).unwrap();
    }
```

`mod tests`(compress.rs)新增:

```rust
    fn names_in(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn cleanup_partial_removes_7z_main_and_volumes_only() {
        let root = crate::testutil::unique_temp("cleanup-7z");
        fs::create_dir_all(&root).unwrap();
        for n in ["A.7z", "A.7z.001", "A.7z.002"] {
            fs::write(root.join(n), b"x").unwrap();
        }
        fs::write(root.join("B.7z"), b"x").unwrap();

        cleanup_partial(&root, "A", "7z");
        let left = names_in(&root);
        assert_eq!(left, ["B.7z".to_string()], "只清 A 的半成品: {left:?}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cleanup_partial_znn_volumes_only_apply_to_zip() {
        let root = crate::testutil::unique_temp("cleanup-znn");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("A.z01"), b"x").unwrap();
        fs::write(root.join("A.7z"), b"x").unwrap();

        cleanup_partial(&root, "A", "7z");
        assert!(root.join("A.z01").exists(), ".zNN 是 zip -s 专属, 7z 清理不碰");

        cleanup_partial(&root, "A", "zip");
        let left = names_in(&root);
        assert!(left.is_empty(), "zip 清理应删 .zNN: {left:?}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn resolve_primary_archive_prefers_plain_then_first_volume_for_7z() {
        let root = crate::testutil::unique_temp("primary-7z");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("A.7z.001"), b"x").unwrap();
        assert_eq!(
            resolve_primary_archive(&root, "A", "7z"),
            root.join("A.7z.001"),
            "无整包时取首卷"
        );
        fs::write(root.join("A.7z"), b"x").unwrap();
        assert_eq!(
            resolve_primary_archive(&root, "A", "7z"),
            root.join("A.7z"),
            "有整包时优先整包"
        );
        fs::remove_dir_all(&root).unwrap();
    }
```

`mod fake_tests` 新增(7z 选项的编排级冒烟):

```rust
    #[test]
    fn sevenz_options_produce_7z_artifacts() {
        let root = two_entries_fixture("orch-7z");
        let mut o = rnd();
        o.format = crate::Format::SevenZ;
        let report = compress_with_backend(
            &FakeBackend::new(&[]),
            &root.join("in"),
            &root.join("out"),
            &o,
            &CancelFlag::new(),
            &|_| ControlFlow::Continue(()),
        )
        .unwrap();
        assert_eq!(report.entries.len(), 2);
        for e in &report.entries {
            let s = e.archive.to_string_lossy().into_owned();
            assert!(s.ends_with(".7z"), "产物应为 .7z: {s}");
            assert!(e.archive.exists(), "主卷路径应真实存在: {s}");
        }
        fs::remove_dir_all(&root).unwrap();
    }
```

`crates/core/src/backend.rs` `mod tests` 新增:

```rust
    #[test]
    fn ensure_tool_available_reports_missing_tool() {
        let err = ensure_tool_available("no-such-tool-xyz").unwrap_err();
        assert!(
            matches!(err, CoreError::ToolMissing(ref p) if p == "no-such-tool-xyz"),
            "实际: {err:?}"
        );
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p zip-tool-core assign_dsts_fills cleanup_partial_removes_7z resolve_primary sevenz_options ensure_tool_available`
Expected: 编译失败(assign_dsts/cleanup_partial/resolve_primary_archive 参数数不匹配)或断言失败

- [ ] **Step 3: 实现**

**backend.rs**:把 `fn locate_extract_tool`(约 262-269 行)改名为公开(crate 内)版本,并替换两处调用(`RealExtract::list`、`RealExtract::extract` 里的 `locate_extract_tool(...)`):

```rust
/// 在 PATH 里定位压缩/解压程序, 缺失报 ToolMissing(而不是 spawn 失败的 os error 2)
pub(crate) fn ensure_tool_available(program: &str) -> Result<()> {
    let dirs: Vec<PathBuf> =
        std::env::split_paths(std::env::var_os("PATH").as_deref().unwrap_or_default()).collect();
    match crate::doctor::locate_tool(program, &dirs) {
        Some(_) => Ok(()),
        None => Err(crate::CoreError::ToolMissing(program.into())),
    }
}
```

**scan.rs**:`Entry.format` 的文档注释改为 `/// 压缩包格式("zip"/"7z"), 由 Step 2 按 Options.format 填充; 扫描后为空串`;`scan_dir` 构造处 `format: "zip".into()` 改为 `format: String::new(),`。

**compress.rs**:

1. `compress_dir`(40-55 行)与 `compress_entries`(117-134 行)公共入口改为按格式选后端并预检工具:

```rust
pub fn compress_dir(
    input: &Path,
    out_dir: &Path,
    opts: &Options,
    cancel: &CancelFlag,
    on_progress: &dyn Fn(&Progress) -> ControlFlow<()>,
) -> Result<Report> {
    let backend = opts.format.backend();
    crate::backend::ensure_tool_available(backend.program())?;
    compress_with_backend(&backend, input, out_dir, opts, cancel, on_progress)
}
```

```rust
pub fn compress_entries(
    input: &Path,
    out_dir: &Path,
    entries: &[Entry],
    opts: &Options,
    cancel: &CancelFlag,
    on_progress: &dyn Fn(&Progress) -> ControlFlow<()>,
) -> Result<Report> {
    let backend = opts.format.backend();
    crate::backend::ensure_tool_available(backend.program())?;
    compress_entries_with_backend(&backend, input, out_dir, entries, opts, cancel, on_progress)
}
```

2. `plan` 末尾(112 行)传扩展名:

```rust
    assign_dsts(entries, &out_abs, opts.random_name, opts.format.extension())
```

3. `assign_dsts` / `assign_dsts_with` 加 `ext: &str` 参数,循环里填 format 与按扩展名预检:

```rust
fn assign_dsts(entries: &mut [Entry], out_dir: &Path, random: bool, ext: &str) -> Result<()> {
    assign_dsts_with(entries, out_dir, random, ext, &mut || {
        format!("zt_{}", random_name())
    })
}

fn assign_dsts_with(
    entries: &mut [Entry],
    out_dir: &Path,
    random: bool,
    ext: &str,
    next_name: &mut dyn FnMut() -> String,
) -> Result<()> {
```

循环体(保留大小写查重逻辑,只改循环内):

```rust
    for e in entries.iter_mut() {
        e.format = ext.to_string(); // 格式由 plan 统一填充
        e.dst = if random {
            // 撞上已有产物则重新生成(碰撞概率 ~10^-12, 重试只为兜底)
            loop {
                let candidate = next_name();
                if !target_exists(out_dir, &candidate, ext) {
                    break candidate;
                }
            }
        } else {
            if target_exists(out_dir, &e.name, ext) {
                return Err(CoreError::OutputExists(
                    out_dir.join(format!("{e.name}.{ext}")),
                ));
            }
            e.name.clone()
        };
    }
    Ok(())
```

4. `target_exists` 加 ext:

```rust
/// 该 dst 的压缩包(不分卷主文件, 或分卷首卷 {dst}.{ext}.001)是否已存在
fn target_exists(out_dir: &Path, dst: &str, ext: &str) -> bool {
    out_dir.join(format!("{dst}.{ext}")).exists()
        || out_dir.join(format!("{dst}.{ext}.001")).exists()
}
```

5. `compress_entries_with_backend` 内(约 146-181 行):函数开头预检之后取 `let ext = opts.format.extension();`,循环内三处替换:

```rust
    for (i, e) in entries.iter().enumerate() {
        let out_archive = out_abs.join(format!("{}.{}", e.dst, ext));
        let result = backend
            .compress_command(&input_abs, &e.name, &out_archive, opts)
            .and_then(|mut cmd| backend.run(&mut cmd, cancel));
        if let Err(err) = result {
            // 停批并清理半成品: 半成品分卷被误当完整包比少一个包更危险
            cleanup_partial(&out_abs, &e.dst, ext);
```

与

```rust
        let archive = resolve_primary_archive(&out_abs, &e.dst, ext);
```

6. `cleanup_partial` 加 ext:

```rust
/// 删除某条目压缩失败后的半成品: 主文件 {dst}.{ext}、zip -s 的 {dst}.zNN、分卷 {dst}.{ext}.NNN
fn cleanup_partial(out_dir: &Path, dst: &str, ext: &str) {
    let Ok(rd) = fs::read_dir(out_dir) else {
        return;
    };
    let main_name = format!("{dst}.{ext}");
    let vol_prefix = format!("{dst}.{ext}.");
    let z_prefix = format!("{dst}.z"); // 仅 zip -s(Linux)产生 .zNN
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let is_ours = name == main_name
            || (ext == "zip"
                && name.starts_with(&z_prefix)
                && name.len() == dst.len() + 4
                && name[z_prefix.len()..].bytes().all(|c| c.is_ascii_digit()))
            || (name.starts_with(&vol_prefix)
                && name.len() == vol_prefix.len() + 3
                && name[vol_prefix.len()..].bytes().all(|c| c.is_ascii_digit()));
        if is_ours {
            let _ = fs::remove_file(e.path());
        }
    }
}
```

7. `resolve_primary_archive` 加 ext:

```rust
/// 返回压缩包主文件:优先 {base}.{ext}(zip -s 的末卷 / 不分卷),
/// 否则分卷首卷 {base}.{ext}.001。
fn resolve_primary_archive(out_dir: &Path, base: &str, ext: &str) -> PathBuf {
    let plain = out_dir.join(format!("{base}.{ext}"));
    if plain.exists() {
        return plain;
    }
    let first_volume = out_dir.join(format!("{base}.{ext}.001"));
    if first_volume.exists() {
        return first_volume;
    }
    plain
}
```

8. **三个假后端统一改为"写 out_archive 本体 + 按其扩展名造卷"**(它们当前硬编码 `.zip`/`.z01`,7z 选项下会产物错位):

`mod fake_tests` 的 `FakeBackend::compress_command`:

```rust
    impl CommandBackend for FakeBackend {
        fn compress_command(
            &self,
            _work: &Path,
            name: &str,
            out_archive: &Path,
            _opts: &Options,
        ) -> Result<Command> {
            self.attempted.borrow_mut().push(name.to_string());
            let stem = out_archive.file_stem().unwrap().to_string_lossy().into_owned();
            let ext = out_archive
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_default();
            let dir = out_archive.parent().unwrap().to_path_buf();
            if self.fail_on.iter().any(|f| *f == name) {
                // 模拟分卷写一半崩溃:按 out_archive 的实际扩展名留下 .001/.002
                fs::write(dir.join(format!("{stem}.{ext}.001")), b"part").unwrap();
                fs::write(dir.join(format!("{stem}.{ext}.002")), b"part").unwrap();
                return Err(CoreError::BackendFailed {
                    program: "fake".into(),
                    status: "exit status: 1".into(),
                    output: "boom".into(),
                });
            }
            fs::write(out_archive, b"fake").unwrap();
            Ok(Command::new("fake-unused"))
        }
```

同模块 `CancellingBackend::compress_command` 同样处理(cancel 分支写 `fs::write(dir.join(format!("{stem}.{ext}.001")), b"part").unwrap();`,成功分支 `fs::write(out_archive, b"fake").unwrap();`)。

`mod staged_tests` 的 `Recorder::compress_command`:

```rust
        fn compress_command(
            &self,
            _work: &Path,
            name: &str,
            out_archive: &Path,
            _opts: &Options,
        ) -> Result<Command> {
            self.attempted.borrow_mut().push(name.to_string());
            fs::write(out_archive, b"fake").unwrap();
            Ok(Command::new("fake-unused"))
        }
```

9. **测试辅助与文档注释同步**:
- `mod dst_tests` 的 `entries()`:`format: "zip".into()` → `format: String::new()`
- `mod staged_tests` 的 `entry()`:同上
- 既有用例中 `assign_dsts(&mut es, Path::new("out"), false)` 等调用补第 4 参 `"zip"`;`assign_dsts_with(..., true, &mut || ...)` 补 `"zip"`
- 模块头文档 `zt_<16hex>.zip` → `` zt_<16hex>.{zip|7z} ``
- `CompressedEntry.archive` 注释 → `/// 压缩包主文件({dst}.{ext};分卷时 zip -s 为末卷,7z -v 为首卷 .001)`
- `CoreError::ToolMissing` 的文档注释(lib.rs)→ `/// 压缩/解压:所需工具(zip / 7z / unzip)不在 PATH`

- [ ] **Step 4: 全量测试**

Run: `cargo test --workspace`
Expected: 全绿

- [ ] **Step 5: 提交**

```bash
git add -A
git commit -m "core: 压缩编排按格式定扩展名/预检/清理/主卷 + 工具缺失预检

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 4: core — `resolve_archive` 支持 `.7z` 系

**Files:**
- Modify: `crates/core/src/extract.rs:113-155`(ResolvedArchive 文档 + resolve_archive)
- Modify: `crates/core/src/lib.rs`(UnsupportedArchive 文案)
- Test: `crates/core/src/extract.rs`(含 `plain_zip_resolves_to_platform_native_backend` 的测试模块)

- [ ] **Step 1: 写失败测试**

在该测试模块(约 1177 行起,`use super::*; use crate::backend::Backend; ...`)新增:

```rust
    #[test]
    fn sevenz_single_resolves_to_sevenzip_forced() {
        let r = resolve_archive(Path::new("somewhere/B.7z")).unwrap();
        assert_eq!(r.path, Path::new("somewhere/B.7z"));
        assert_eq!(r.backend, Backend::SevenZip, "7z 格式只能 7z 系解");
    }

    #[test]
    fn sevenz_extension_is_case_insensitive() {
        let r = resolve_archive(Path::new("somewhere/B.7Z")).unwrap();
        assert_eq!(r.backend, Backend::SevenZip);
    }

    #[test]
    fn sevenz_member_volume_normalizes_to_first_volume() {
        let d = crate::testutil::unique_temp("resolve-7z");
        fs::write(d.join("B.7z.001"), b"").unwrap();
        let r = resolve_archive(&d.join("B.7z.002")).unwrap();
        assert_eq!(r.path, d.join("B.7z.001"));
        assert_eq!(r.backend, Backend::SevenZip);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn sevenz_member_volume_without_first_volume_reports_missing() {
        let err = resolve_archive(Path::new("somewhere/B.7z.002")).unwrap_err();
        assert!(matches!(err, CoreError::EntryVolumeMissing { .. }), "实际: {err:?}");
    }
```

说明:`.7z.001` 与 `.7z.NNN` 的解析经既有 `"001"`/数字分支已天然工作(stem 规则 `B.7z.002` → 找 `B.7z.001`),新用例把它们固化下来防回归;真正的新分支只有 `.7z` 单包。

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p zip-tool-core sevenz_`
Expected: `sevenz_single_resolves_to_sevenzip_forced` 与 `sevenz_extension_is_case_insensitive` FAIL(UnsupportedArchive);其余 PASS

- [ ] **Step 3: 实现**

`resolve_archive` 的 match 中,`"zip"` 分支后新增:

```rust
        // 7z 格式整包:只有 7z 系工具能解, 强制 SevenZip 后端
        "7z" => Ok(ResolvedArchive {
            path: input.to_path_buf(),
            backend: Backend::SevenZip,
        }),
```

同步文档注释:
- `ResolvedArchive.path` 注释 → `/// 入口卷路径(.zip / .7z 或分卷首卷 .zip.001 / .7z.001), Step 2/3 均以此为准`
- `ResolvedArchive.backend` 注释 → `/// .zip → 平台默认后端; .zip.001 / .7z / .7z.001 → 强制 Backend::SevenZip`
- `lib.rs` `UnsupportedArchive` 的 Display 文案与变体注释补 7z 系:

```rust
    /// 解压:扩展名不是支持的包形态(zip 系四种 / 7z 系三种)
    UnsupportedArchive(PathBuf),
```

```rust
            CoreError::UnsupportedArchive(p) => write!(
                f,
                "unsupported archive type: {} (expected .zip / .zNN / .zip.001 / .zip.NNN / .7z / .7z.001 / .7z.NNN)",
                p.display()
            ),
```

- [ ] **Step 4: 全量测试**

Run: `cargo test --workspace`
Expected: 全绿

- [ ] **Step 5: 提交**

```bash
git add -A
git commit -m "core: resolve_archive 支持 .7z 单包(强制 7z 后端), 文案与注释同步

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 5: core — `scan_groups` 分类 7z 组

**Files:**
- Modify: `crates/core/src/extract.rs:412-562`(scan_groups + member_entry_volume 文档)
- Test: `crates/core/src/extract.rs`(scan_groups 测试模块,约 1272 行起,含 `touch` 辅助)

- [ ] **Step 1: 写失败测试**

在 scan_groups 的测试模块 `batch_tests`(约 1274 行起,已有 `touch`/`entry_names`/`file_name_of`/`unique_temp` 辅助与 `use crate::backend::Backend;`)新增:

```rust
    /// 组名(入口卷文件名)排序收集(复用既有 entry_names)
    fn sorted_group_names(scan: &ExtractScan) -> Vec<String> {
        let mut v = entry_names(&scan.groups);
        v.sort();
        v
    }

    #[test]
    fn scan_groups_classifies_7z_entries_and_mixed_dirs() {
        let d = unique_temp("sg-7z");
        fs::create_dir_all(&d).unwrap();
        touch(&d, "Novel.7z"); // 7z 整包
        touch(&d, "media.7z.001"); // 7z 分卷首卷
        touch(&d, "media.7z.002"); // 成员卷(静默)
        touch(&d, "old.zip"); // zip 组混合
        let scan = scan_groups(&d).unwrap();

        assert_eq!(
            sorted_group_names(&scan),
            ["Novel.7z", "media.7z.001", "old.zip"]
        );
        for g in &scan.groups {
            let n = file_name_of(&g.resolved.path);
            let want = if n.ends_with(".zip") {
                Backend::native()
            } else {
                Backend::SevenZip
            };
            assert_eq!(g.resolved.backend, want, "{n} 的后端");
        }
        // media 组大小 = 两个卷之和
        let media = scan
            .groups
            .iter()
            .find(|g| file_name_of(&g.resolved.path) == "media.7z.001")
            .unwrap();
        assert_eq!(media.size, 0, "touch 的文件为空, 大小 0");
        assert!(scan.issues.is_empty(), "成员卷应静默: {:?}", scan.issues);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn scan_groups_reports_orphan_7z_volume() {
        let d = unique_temp("sg-7z-orphan");
        fs::create_dir_all(&d).unwrap();
        touch(&d, "media.7z.002"); // 无 .7z.001 → 孤儿卷
        touch(&d, "Novel.7z");
        let scan = scan_groups(&d).unwrap();
        assert_eq!(sorted_group_names(&scan), ["Novel.7z"]);
        assert!(
            scan.issues
                .iter()
                .any(|i| i.name == "media.7z.002" && i.message.contains("media.7z.001")),
            "{:?}",
            scan.issues
        );
        fs::remove_dir_all(&d).unwrap();
    }
```

(`touch`/`entry_names`/`file_name_of`/`unique_temp` 均为 `batch_tests` 既有辅助,直接使用。)

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p zip-tool-core scan_groups_classifies_7z scan_groups_reports_orphan_7z`
Expected: FAIL(`.7z`/.7z.001 现落 others → "skipped non-archive file",组缺失)

- [ ] **Step 3: 实现**

`scan_groups` 内:

1. `sev_entries` 声明注释与语义扩展(变量名不变):

```rust
    let mut sev_entries: Vec<String> = Vec::new(); // X.zip.001 / X.7z / X.7z.001(强制 7z 系后端)
```

2. 分类分支(在 `.zip.001` 判断处扩展):

```rust
        } else if lower.ends_with(".zip") {
            zip_entries.push(name.clone());
            *sizes.entry(lower).or_default() += size;
        } else if lower.ends_with(".zip.001")
            || lower.ends_with(".7z")
            || lower.ends_with(".7z.001")
        {
            sev_entries.push(name.clone());
            *sizes.entry(lower).or_default() += size;
        } else if let Some(expected) = member_entry_volume(&lower) {
```

(顺序无歧义:`X.7z.001` 不以 `.7z` 结尾;`member_entry_volume` 的数字分支已把 `X.7z.NNN` 归到 `X.7z.001`,无需改动其逻辑,仅补注释 `x.7z.nnn(nnn≠001)→ x.7z.001`。)

3. 组装循环的后端判定反转(zip → native,其余 → SevenZip):

```rust
    for name in zip_entries.iter().chain(&sev_entries) {
        let backend = if name.to_lowercase().ends_with(".zip") {
            Backend::native()
        } else {
            Backend::SevenZip
        };
```

4. 函数头文档注释(412-414 行)改为:

```rust
/// 7.3 Step B1 扫描分组: 输入目录顶层(不递归)分类出压缩包组(UI 分阶段入口)。
/// `X.zip` / `X.zip.001` / `X.7z` / `X.7z.001` 是入口卷; `.zNN` / `.zip.NNN` / `.7z.NNN` 是
/// 成员卷(入口卷存在则静默, 否则为孤儿卷记 issue); `mapping.csv` 是元数据; 其余文件记 issue。
```

- [ ] **Step 4: 全量测试**

Run: `cargo test --workspace`
Expected: 全绿

- [ ] **Step 5: 提交**

```bash
git add -A
git commit -m "core: scan_groups 分类 7z 组(入口卷/成员卷/孤儿卷, 与 zip 组可混处)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 6: CLI — `--format`/`--level` 与 doctor 平台工具全集

**Files:**
- Modify: `crates/cli/src/main.rs`
- Modify: `crates/core/src/doctor.rs`

- [ ] **Step 1: 写失败测试**

`crates/core/src/doctor.rs` 测试模块新增:

```rust
    #[test]
    fn platform_tools_cover_both_formats_and_extract() {
        let tools = platform_tools();
        if cfg!(windows) {
            // Windows 压缩(zip 与 7z 格式)与解压全走 7z
            assert_eq!(tools, ["7z"]);
        } else {
            // Linux: 压缩 zip 格式用 zip, 7z 格式与解 .7z 用 7z, 解 zip 用 unzip
            assert_eq!(tools, ["zip", "7z", "unzip"]);
        }
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p zip-tool-core platform_tools`
Expected: 编译失败(函数不存在)

- [ ] **Step 3: 实现 doctor**

`crates/core/src/doctor.rs`:模块头文档第 2 行 `定位当前平台后端工具(zip / 7z)的完整路径` 改为 `定位当前平台涉及的全部工具(见 platform_tools)的完整路径`。`check_backend_tool` 整体删除(唯一调用方是 CLI,本任务同步替换),新增:

```rust
/// 当前平台压缩/解压涉及的完整工具列表:
/// Windows 压缩(zip 与 7z 格式)与解压全走 7z;Linux 压缩用 zip / 7z, 解压用 unzip / 7z。
pub fn platform_tools() -> &'static [&'static str] {
    if cfg!(windows) {
        &["7z"]
    } else {
        &["zip", "7z", "unzip"]
    }
}

/// 检查平台全部工具(从 PATH 定位 + 试运行), CLI doctor 用
pub fn check_platform_tools() -> Vec<ToolCheck> {
    let dirs: Vec<PathBuf> =
        std::env::split_paths(std::env::var_os("PATH").as_deref().unwrap_or_default()).collect();
    platform_tools().iter().map(|p| check_tool(p, &dirs)).collect()
}
```

(`use crate::backend::Backend;` 若因此不再被使用则删除该行。)

- [ ] **Step 4: 实现 CLI**

`crates/cli/src/main.rs`:

1. import 行加 `Format`:

```rust
use zip_tool_core::{CancelFlag, Format, Options, Phase, Progress, compress_dir, extract, parse_size};
```

2. Compress 子命令加两个参数(放在 `random_name` 之后):

```rust
        /// 压缩包格式:zip(最大兼容)或 7z(LZMA2, 压缩率更高);缺省 zip
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(["zip", "7z"]))]
        format: Option<String>,
        /// 压缩级别 0-9(0=仅存储, 9=最大);缺省用后端默认
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=9))]
        level: Option<u8>,
```

3. match 分支解构加 `format, level`,构造处替换:

```rust
        Command::Compress {
            input,
            output,
            split,
            password,
            random_name,
            format,
            level,
        } => {
            // ...split_size 解析不变...
            let opts = Options {
                split_size,
                password,
                random_name,
                format: match format.as_deref() {
                    Some("7z") => Format::SevenZ,
                    _ => Format::Zip, // Some("zip") / None
                },
                level,
            };
```

4. Doctor 分支替换为遍历平台工具:

```rust
        Command::Doctor => {
            let checks = zip_tool_core::doctor::check_platform_tools();
            for c in &checks {
                match (&c.path, &c.probe) {
                    (Some(p), Ok(Some(line))) => {
                        println!("[ok] {}: {} ({line})", c.program, p.display())
                    }
                    (Some(p), Ok(None)) => println!("[ok] {}: {}", c.program, p.display()),
                    (Some(p), Err(e)) => println!("[FAIL] {}: {} ({e})", c.program, p.display()),
                    (None, _) => println!("[FAIL] {}: not found in PATH", c.program),
                }
            }
            // 规格 §7: Linux 上 7z 缺失仅影响 7z 格式与解 .7z, 标注适用范围
            #[cfg(not(windows))]
            if checks.iter().any(|c| c.program == "7z" && !c.is_ok()) {
                println!("note: 7z 只影响 7z 格式压缩与解 .7z(zip 格式压缩/解压不受影响)");
            }
            if checks.iter().all(|c| c.is_ok()) {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
```

- [ ] **Step 5: 全量测试**

Run: `cargo test --workspace && cargo run -p zip-tool-cli -- doctor`
Expected: 测试全绿;doctor 本机(Windows)打印 `[ok] 7z: ...` 且退出码 0

- [ ] **Step 6: 提交**

```bash
git add -A
git commit -m "cli: compress --format/--level 参数; doctor 扩为平台工具全集(删 check_backend_tool)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 7: E2E — 7z 格式 roundtrip

**Files:**
- Modify: `test/tests/cli_e2e.rs`

- [ ] **Step 1: 更新测试辅助**

1. 文件头注释(第 5 行)依赖说明改为:

```rust
//! 依赖: Windows 需 7z(NanaZip 兼容);Linux 需 zip / unzip;--format 7z 用例另需 p7zip 的 7z。
```

2. `extract_archive`(约 142 行)与 `extract_with_wrong_password_must_fail`(约 203 行)的分支条件加 `.7z` 判定(Linux 的 unzip 不认识 7z,7z 格式的包在 Linux 上也用 7z 解):

```rust
/// .7z 系(.7z / .7z.001)交系统 7z;zip 系按平台(zip -s 分卷 Linux 走合并流程)
fn is_7z_archive(archive: &Path) -> bool {
    archive
        .extension()
        .map(|e| e.to_string_lossy().eq_ignore_ascii_case("7z"))
        .unwrap_or(false)
}
```

`extract_archive` 开头 `if cfg!(windows) {` 改为 `if cfg!(windows) || is_7z_archive(archive) {`;`extract_with_wrong_password_must_fail` 同样,把两个分支的静态 if 换成运行时 if:

```rust
fn extract_with_wrong_password_must_fail(archive: &Path, dest: &Path) {
    fs::create_dir_all(dest).unwrap();
    let out = if cfg!(windows) || is_7z_archive(archive) {
        Command::new("7z")
            .arg("x")
            .arg("-y")
            .arg("-pWRONG-PW")
            .arg(format!("-o{}", dest.display()))
            .arg("--")
            .arg(archive)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    } else {
        Command::new("unzip")
            .arg("-o")
            .arg("-P")
            .arg("WRONG-PW")
            .arg(archive)
            .arg("-d")
            .arg(dest)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    assert!(
        !out.status.success(),
        "错误密码不应解压成功: {}",
        archive.display()
    );
}
```

3. `is_random_artifact`(约 244-249 行)的 match 加 7z 形态:

```rust
    match &rest[16..] {
        ".zip" | ".7z" => true,
        s if s.starts_with(".zip.") => s.len() == 8 && s[5..].bytes().all(|b| b.is_ascii_digit()),
        s if s.starts_with(".7z.") => s.len() == 7 && s[4..].bytes().all(|b| b.is_ascii_digit()),
        s if s.starts_with(".z") => s.len() == 4 && s[3..].bytes().all(|b| b.is_ascii_digit()),
        _ => false,
    }
```

(注意 `.7z.001` 共 7 字符,与 `.zip.001` 的 8 字符不同。)

- [ ] **Step 2: 写两个新用例**

文件末尾追加:

```rust
#[test]
fn compress_7z_format_roundtrip() {
    let fx = fixture("fmt-7z");
    let (inp, out) = (fx.root.join("in"), fx.root.join("out"));

    let r = run_tool(&[
        "compress",
        "-i",
        inp.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--format",
        "7z",
    ]);
    assert!(r.ok, "7z 格式压缩失败:\n{}", r.stderr);

    // 产物扩展名 .7z, 包名规则不变(原名追加)
    assert_eq!(list_dir(&out), ["Novel.7z", "media.7z", "photo.jpg.7z"]);

    let restore = fx.root.join("restore");
    for name in ["Novel", "media", "photo.jpg"] {
        extract_archive(&out.join(format!("{name}.7z")), &restore, None);
    }
    assert_tree_matches_input(&fx);
}

#[test]
fn compress_7z_split_password_random_then_tool_extracts_back() {
    let fx = fixture("fmt-7z-split");
    compress_fixture(
        &fx,
        &[
            "--format",
            "7z",
            "--split",
            "1m",
            "-p",
            "secret123",
            "--random-name",
        ],
    );

    let out = fx.root.join("out");
    let names = list_dir(&out);
    assert!(names.contains(&"mapping.csv".to_string()), "{names:?}");
    let archives: Vec<&String> = names.iter().filter(|n| *n != "mapping.csv").collect();
    assert!(!archives.is_empty(), "{names:?}");
    for n in &archives {
        assert!(is_random_artifact(n), "产物应是 zt_ 随机名: {n}");
    }

    // clip.bin 3MB / 1m 至少 2 卷(.7z.001/.002/...)
    let csv = fs::read_to_string(out.join("mapping.csv")).unwrap();
    let rows: Vec<(String, String)> = csv
        .lines()
        .skip(1)
        .map(|l| {
            let (a, o) = l.split_once(',').unwrap();
            (a.to_string(), o.to_string())
        })
        .collect();
    assert_eq!(rows.len(), 3, "{csv:?}");
    let media_primary = rows.iter().find(|(_, o)| o == "media").unwrap().0.clone();
    let id: String = media_primary.chars().take("zt_".len() + 16).collect();
    assert!(
        names.iter().filter(|n| n.starts_with(&id)).count() >= 2,
        "media 应被分包: {names:?}"
    );

    // 加密生效(7z 格式 = AES-256):错误密码被系统 7z 拒绝
    let (first, _) = &rows[0];
    extract_with_wrong_password_must_fail(&out.join(first), &fx.root.join("badpw"));

    // 本工具批量解压:覆盖 .7z.001 入口卷归一 + scan_groups 7z 分组 + mapping 还原
    let r = run_tool(&[
        "extract",
        "-i",
        out.to_str().unwrap(),
        "-o",
        fx.root.join("restore").to_str().unwrap(),
        "-p",
        "secret123",
    ]);
    assert!(r.ok, "批量 extract 失败:\n{}\n{}", r.stderr, r.stdout);
    assert_tree_matches_input(&fx);
    for orig in ["Novel", "media", "photo.jpg"] {
        assert!(
            r.stdout.contains(&format!("[{orig}]")),
            "报告应含原条目名 {orig}: {}",
            r.stdout
        );
    }
}
```

- [ ] **Step 3: 运行**

Run: `cargo test -p zip-tool-e2e --test cli_e2e`
Expected: 全部用例 PASS(本机 NanaZip 验证 7z 格式全链路)

- [ ] **Step 4: 全量回归**

Run: `cargo test --workspace`
Expected: 全绿

- [ ] **Step 5: 提交**

```bash
git add -A
git commit -m "e2e: 7z 格式 roundtrip(单包/分卷/密码/本工具批量还原)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 8: tauri 壳层 — DTO 透传 format/level

**Files:**
- Modify: `crates/tauri-app/src/lib.rs`

- [ ] **Step 1: 写失败测试**

`crates/tauri-app/src/lib.rs` 测试模块新增(并保持既有用例编译):

```rust
    #[test]
    fn to_core_options_maps_format_and_level() {
        let base = || CompressOptions {
            split_size: None,
            password: None,
            random_name: false,
            format: None,
            level: None,
        };
        let mut o = base();
        o.format = Some("7z".into());
        o.level = Some(5);
        let core = to_core_options(o).unwrap();
        assert_eq!(core.format, zip_tool_core::Format::SevenZ);
        assert_eq!(core.level, Some(5));

        // 缺省 = zip + 后端默认级别
        let core = to_core_options(base()).unwrap();
        assert_eq!(core.format, zip_tool_core::Format::Zip);
        assert!(core.level.is_none());

        // 非法值拒绝
        let mut o = base();
        o.format = Some("rar".into());
        assert!(to_core_options(o).is_err());
        let mut o = base();
        o.level = Some(10);
        assert!(to_core_options(o).is_err());
    }
```

同时既有 `fn entry(...)` 辅助中 `format: "zip".into()` 改为 `format: String::new()`(与 scan 语义一致)。

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p zip-tool-tauri to_core_options`
Expected: 编译失败(`to_core_options`/DTO 字段不存在)

- [ ] **Step 3: 实现**

1. import 行加 `Format`:

```rust
use zip_tool_core::{
    CancelFlag, Entry, EntryKind, ExtractGroup, ExtractScan, Format, Options, Phase, Progress,
    ScanReport,
};
```

2. `CompressOptions` DTO 加字段:

```rust
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompressOptions {
    /// "2g"/"500m"/字节数; None = 不分包
    split_size: Option<String>,
    /// 空串按 None(不加密)
    password: Option<String>,
    random_name: bool,
    /// "zip" | "7z"; None = zip(最大兼容)
    format: Option<String>,
    /// 0-9; None = 后端默认
    level: Option<u8>,
}
```

3. 新增映射辅助(放在 `non_empty` 附近):

```rust
/// UI DTO → core Options;format/level 的取值校验在这里(宁停不错)
fn to_core_options(o: CompressOptions) -> Result<Options, String> {
    let format = match o.format.as_deref() {
        None | Some("zip") => Format::Zip,
        Some("7z") => Format::SevenZ,
        Some(other) => return Err(format!("未知压缩格式: {other}")),
    };
    let level = match o.level {
        None => None,
        Some(n) if n <= 9 => Some(n),
        Some(n) => return Err(format!("压缩级别超出 0-9: {n}")),
    };
    let split_size = match o.split_size.as_deref().map(zip_tool_core::parse_size) {
        Some(Ok(v)) => Some(v),
        Some(Err(e)) => return Err(e),
        None => None,
    };
    Ok(Options {
        split_size,
        password: non_empty(o.password),
        random_name: o.random_name,
        format,
        level,
    })
}
```

4. `compress` 命令体内原有的 split_size 解析 + Options 构造(374-383 行)整体替换为一行(位置在 `acquire_busy` 之前,错误先行返回的语义不变):

```rust
    let opts = to_core_options(options)?;
```

- [ ] **Step 4: 全量测试**

Run: `cargo test --workspace`
Expected: 全绿

- [ ] **Step 5: 提交**

```bash
git add -A
git commit -m "tauri: 压缩 DTO 透传 format/level(to_core_options 统一校验)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 9: UI — 模式预设 + 高级微调 + 密码提示

**Files:**
- Modify: `crates/tauri-app/ui/index.html`
- Modify: `crates/tauri-app/ui/app.js`
- Modify: `crates/tauri-app/test/ui.test.mjs`
- 重新生成: `crates/tauri-app/ui/app.css`

- [ ] **Step 1: 写失败测试**

`crates/tauri-app/test/ui.test.mjs` 末尾追加三个用例:

```js
test("行为: 压缩模式预设 — 默认最大兼容性, 预设联动参数, 手动改参数变自定义", async () => {
  const h = makeHarness(false, () => null);
  // 默认 = 最大兼容性预设(zip + 强度 9)
  assert.equal(h.doc.getElementById("opt-format").value, "zip");
  assert.equal(h.doc.getElementById("opt-level").value, "9");
  assert.ok(h.doc.getElementById("opt-preset-custom").classList.contains("hidden"));
  assert.ok(h.doc.getElementById("preset-compat").className.includes("bg-blue-600"));

  // 切平衡预设 → 底层参数联动
  h.doc.getElementById("preset-balanced").click();
  assert.equal(h.doc.getElementById("opt-format").value, "7z");
  assert.equal(h.doc.getElementById("opt-level").value, "5");
  assert.ok(h.doc.getElementById("preset-balanced").className.includes("bg-blue-600"));
  assert.ok(!h.doc.getElementById("preset-compat").className.includes("bg-blue-600"));

  // 手动改强度 → 自定义(高亮清除)
  const lvl = h.doc.getElementById("opt-level");
  lvl.value = "2";
  lvl.dispatchEvent(new h.window.Event("input", { bubbles: true }));
  assert.ok(!h.doc.getElementById("opt-preset-custom").classList.contains("hidden"));
  assert.ok(!h.doc.getElementById("preset-balanced").className.includes("bg-blue-600"));

  // 高级区显隐开关
  assert.ok(h.doc.getElementById("opt-format-wrap").classList.contains("hidden"));
  h.doc.getElementById("opt-advanced").click();
  assert.ok(!h.doc.getElementById("opt-format-wrap").classList.contains("hidden"));
  assert.ok(!h.doc.getElementById("opt-level-wrap").classList.contains("hidden"));

  // 解压模式下预设/高级控件隐藏, 切回压缩恢复
  h.doc.getElementById("mode-extract").click();
  assert.ok(h.doc.getElementById("opt-preset-wrap").classList.contains("hidden"));
  h.doc.getElementById("mode-compress").click();
  assert.ok(!h.doc.getElementById("opt-preset-wrap").classList.contains("hidden"));
});

test("契约: 平衡预设的参数透传到 compress 调用", async () => {
  const h = makeHarness(true, ({ cmd, args }) => {
    if (cmd === "plugin:dialog|open")
      return args.options.title?.includes("目标") ? "C:\\out" : "C:\\demo\\data";
    if (cmd === "scan") return SCAN_COMPRESS;
    if (cmd === "compress") return { cancelled: false, entries: [], warnings: [] };
    return null;
  });
  await pickSource(h);
  h.doc.getElementById("dst").click();
  await h.flush();
  await h.flush();
  h.doc.getElementById("preset-balanced").click();
  h.doc.getElementById("confirm").click();
  await h.flush();
  await h.flush();
  const comp = h.calls.find((c) => c.cmd === "compress");
  assert.ok(comp, "应调用 compress");
  jsonEq(comp.args.options, {
    splitSize: null,
    password: null,
    randomName: false,
    format: "7z",
    level: 5,
  });
});

test("行为: 密码提示随格式切换(zip=ZipCrypto / 7z=AES-256)", async () => {
  const h = makeHarness(false, () => null);
  const hint = h.doc.getElementById("pw-hint");
  h.doc.getElementById("opt-password").value = "pw";
  h.doc.getElementById("opt-password").dispatchEvent(
    new h.window.Event("input", { bubbles: true })
  );
  assert.ok(!hint.classList.contains("hidden"), "zip+密码应有提示");
  assert.ok(hint.textContent.includes("ZipCrypto"), hint.textContent);

  h.doc.getElementById("preset-balanced").click();
  h.doc.getElementById("opt-format").dispatchEvent(
    new h.window.Event("change", { bubbles: true })
  );
  assert.ok(hint.textContent.includes("AES-256"), hint.textContent);

  h.doc.getElementById("opt-password").value = "";
  h.doc.getElementById("opt-password").dispatchEvent(
    new h.window.Event("input", { bubbles: true })
  );
  assert.ok(hint.classList.contains("hidden"), "无密码时提示隐藏");
});
```

同时**更新既有全流程用例的选项断言**(约 184 行):

```js
  jsonEq(comp.args.options, {
    splitSize: "2g",
    password: "pw",
    randomName: true,
    format: "zip",
    level: 9,
  });
```

- [ ] **Step 2: 运行确认失败**

Run: `cd crates/tauri-app && npm test`
Expected: 新用例 FAIL(`opt-format` 等元素为 null),既有 options 断言 FAIL

- [ ] **Step 3: 实现 index.html**

选项行(`#options` 内)在「随机包名」label 之后、「覆盖已有文件」label 之前插入:

```html
      <span id="opt-preset-wrap" class="hidden items-center gap-1.5">
        <label class="shrink-0">模式</label>
        <span class="flex rounded-md border border-slate-300 overflow-hidden">
          <button
            type="button"
            id="preset-compat"
            title="zip 格式 + 最大压缩强度; 所有系统都能打开"
            class="px-2 py-1 bg-blue-600 text-white"
          >
            最大兼容性
          </button>
          <button
            type="button"
            id="preset-balanced"
            title="7z 格式(LZMA2), 压缩率更高; 需 7z 系工具解压"
            class="px-2 py-1 bg-white text-slate-600 border-l border-slate-300"
          >
            平衡
          </button>
        </span>
        <span id="opt-preset-custom" class="hidden text-slate-400">自定义</span>
      </span>
      <label id="opt-advanced-wrap" class="hidden items-center gap-1.5 cursor-pointer">
        <input id="opt-advanced" type="checkbox" class="accent-blue-600 align-middle" />
        高级
      </label>
      <span id="opt-format-wrap" class="hidden items-center gap-1.5">
        <label for="opt-format" class="shrink-0">格式</label>
        <select
          id="opt-format"
          class="px-2 py-1 rounded-md border border-slate-300 text-slate-700 focus:outline-none focus:border-blue-400"
        >
          <option value="zip">zip</option>
          <option value="7z">7z</option>
        </select>
      </span>
      <span id="opt-level-wrap" class="hidden items-center gap-1.5">
        <label for="opt-level" class="shrink-0">强度</label>
        <input
          id="opt-level"
          type="number"
          min="0"
          max="9"
          value="9"
          class="w-14 px-2 py-1 rounded-md border border-slate-300 text-slate-700 focus:outline-none focus:border-blue-400"
        />
      </span>
```

密码输入的 `</span>`(`#opt-password-wrap`)之后插入:

```html
      <span id="pw-hint" class="hidden text-amber-600"></span>
```

- [ ] **Step 4: 实现 app.js**

1. `LOCK_IDS` 补新控件:

```js
const LOCK_IDS = [
  "mode-compress", "mode-extract", "src", "dst",
  "opt-split", "opt-password", "opt-random", "opt-force",
  "preset-compat", "preset-balanced", "opt-advanced", "opt-format", "opt-level",
];
```

2. `BADGE` 常量之后加预设常量与两个函数:

```js
// 压缩模式预设 = 底层参数组合(格式 + 强度);core/CLI 无"模式"概念(_docs/compress-modes.md)
const PRESETS = {
  compat: { format: "zip", level: 9 },
  balanced: { format: "7z", level: 5 },
};
// 预设按钮两种形态(选中蓝 / 未选白; 平衡钮恒有左边框)
const PRESET_ON = "px-2 py-1 bg-blue-600 text-white";
const PRESET_OFF = "px-2 py-1 bg-white text-slate-600 hover:bg-slate-50";

function highlightPreset(name) {
  $("opt-preset-custom").classList.toggle("hidden", name !== "custom");
  $("preset-compat").className =
    name === "compat" ? PRESET_ON : PRESET_OFF;
  $("preset-balanced").className =
    name === "balanced" ? PRESET_ON : PRESET_OFF + " border-l border-slate-300";
}

// 选预设 = 写入底层参数; 手动改参数 = 离开预设(自定义)
function applyPreset(name) {
  const p = PRESETS[name];
  $("opt-format").value = p.format;
  $("opt-level").value = p.level;
  highlightPreset(name);
}
```

3. `setMode` 内选项显隐区(`opt-force-wrap` 两行之后)补:

```js
  $("opt-preset-wrap").classList.toggle("hidden", mode !== "compress");
  $("opt-preset-wrap").classList.toggle("flex", mode === "compress");
  $("opt-advanced-wrap").classList.toggle("hidden", mode !== "compress");
  $("opt-advanced-wrap").classList.toggle("flex", mode === "compress");
```

4. 确认流程的 compress options(约 286-290 行)加两个字段:

```js
  const options = isCompress
    ? {
        splitSize: $("opt-split").value.trim() || null,
        password: $("opt-password").value || null,
        randomName: $("opt-random").checked,
        format: $("opt-format").value,
        level: Number($("opt-level").value),
      }
```

5. 文件底部事件绑定区(`$("dst").addEventListener(...)` 之后)加:

```js
// —— 压缩模式预设与高级微调 ——
$("preset-compat").addEventListener("click", () => applyPreset("compat"));
$("preset-balanced").addEventListener("click", () => applyPreset("balanced"));
$("opt-advanced").addEventListener("change", (e) => {
  const show = e.target.checked;
  for (const id of ["opt-format-wrap", "opt-level-wrap"]) {
    $(id).classList.toggle("hidden", !show);
    $(id).classList.toggle("flex", show);
  }
});
// 手动改底层参数 = 离开预设
for (const id of ["opt-format", "opt-level"]) {
  $(id).addEventListener("input", () => highlightPreset("custom"));
}
// 密码语义随格式: zip=ZipCrypto(兼容优先, 强度弱), 7z=AES-256
function updatePwHint() {
  const hint = $("pw-hint");
  const pw = $("opt-password").value;
  if (!pw || state.mode !== "compress") {
    hint.classList.add("hidden");
    return;
  }
  hint.textContent =
    $("opt-format").value === "7z"
      ? "7z 格式: AES-256 加密"
      : "zip 格式: ZipCrypto 加密(兼容性最好, 强度弱)";
  hint.classList.remove("hidden");
}
$("opt-password").addEventListener("input", updatePwHint);
$("opt-format").addEventListener("change", updatePwHint);
// 初始状态 = 最大兼容性预设
applyPreset("compat");
```

- [ ] **Step 5: 测试与 CSS 重生成**

Run: `cd crates/tauri-app && npm test && npm run css`
Expected: node 测试全绿(新 3 用例 + 既有用例含更新后的 options 断言);`app.css` 重新生成(git 会显示更新,提交它)

- [ ] **Step 6: 全量回归**

Run: `cargo test --workspace`
Expected: 全绿(UI 改动不影响 Rust 侧)

- [ ] **Step 7: 提交**

```bash
git add -A
git commit -m "ui: 压缩模式预设(最大兼容性/平衡)+ 高级微调 + 密码语义提示

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 10: 文档同步

**Files:**
- Modify: `_docs/compress.md` `_docs/extract.md` `_docs/ui-backend.md` `README.md` `_docs/compress-modes.md`

- [ ] **Step 1: compress.md**

1. §1 非目标(第 19 行):

```markdown
- 不自带压缩实现, 只封装外部命令 (Linux: `zip`/`7z`, Windows: `7z`); 输出格式支持 zip 与 7z (LZMA2), 见 _docs/compress-modes.md
```

2. §6.2 命令表(第 175 行 Windows 行之后)加一行,Windows 行补级别参数;下方 `- 不暴露压缩级别, 用后端默认`(第 179 行)替换:

```markdown
| Windows | 7z | `7z a -tzip [-v<size>] [-p<pw>] [-mx=<0-9>] -- <out_abs>/<dst>.zip <name>` |
| 7z 格式 | 7z | `7z a -t7z -m0=LZMA2 [-v<size>] [-p<pw>] [-mx=<0-9>] -- <out_abs>/<dst>.7z <name>` |
```

```markdown
- 压缩级别 0-9 经 `Options.level` 透传 (zip: `-<n>`; 7z: `-mx=<n>`, 0 = 仅存储); 缺省 = 后端默认。格式维度见 _docs/compress-modes.md
```

3. §6.3 产物表(第 181-184 行)加 7z 行,主卷说明补 `.7z`:

```markdown
| 7z 格式 | `B.7z` | `B.7z.001, B.7z.002, ...`(首卷 `.001`) |
```

主卷句改为:`优先 \`B.zip\`/\`B.7z\`(zip 末卷 / 不分卷), 否则分卷首卷 \`B.zip.001\`/\`B.7z.001\`.`

4. §6.4 清理 bullet:在 `精确匹配 \`<dst>.zip\`、\`<dst>.z[0-9][0-9]\`、\`<dst>.zip.[0-9][0-9][0-9]\` 并删除` 后补 `; 7z 格式对应 \`<dst>.7z\`、\`<dst>.7z.[0-9][0-9][0-9]\``。

5. §11(非目标与后续改进):通读该节,删除/改述其中"压缩级别"相关条目(已由本特性落地),并在节首补一句 `格式维度(zip/7z)与压缩级别已于 2026-10 落地, 见 _docs/compress-modes.md。`。

- [ ] **Step 2: extract.md**

先读 `_docs/extract.md` 的 §3.1(约 52-61 行)、§4.1(约 71-83 行)、§7.3(约 188-201 行),按既有表格风格插入:

§3.1 表格追加两行:

```markdown
| 7z 格式整包 | `B.7z` | 本体 `B.7z` | 仅 7z 系(unzip 不认识 7z) |
| 7z 格式分卷 | `B.7z.001 ...` | **首卷 B.7z.001** | 仅 7z 系 |
```

§4.1 判定规则表追加(并同步 §1 非目标中"入口卷为 `.zip.001` 时强制 7z"的措辞为"`.zip.001` / `.7z` 系强制 7z"):

```markdown
| `.7z` | 7z 格式整包 | 本体, 强制 `Backend::SevenZip` |
| `.7z.001` | 7z 分卷首卷 | 本体, 强制 `Backend::SevenZip` |
| `.7z.NNN`(NNN>001) | 7z 分卷成员卷 | 归一化到同目录 `B.7z.001`, 缺失报 `EntryVolumeMissing` |
```

§7.3 B1 表格追加:

```markdown
| `X.7z` | 7z 格式整包, 入口卷 |
| `X.7z.001` | 7z 格式分卷首卷, 入口卷 |
| `X.7z.NNN`(NNN>001) | 成员卷: `X.7z.001` 存在 → 静默忽略; 不存在 → 孤儿卷, 同上 |
```

并在 §7.3 下方要点列表补一条 `- zip 组与 7z 组可混处同一目录, 各自独立解压`。

- [ ] **Step 3: ui-backend.md**

先读 §4(数据结构 DTO,约 89-113 行)与 §8(选项控件,约 237-250 行),补:

§4 `CompressOptions` 描述处追加字段说明:

```markdown
- `format`: `"zip" | "7z"`, 缺省 `"zip"`(UI 预设总是传具体值)
- `level`: `0-9` 整数, UI 预设总是传具体值; CLI 不传时 core 侧为 `None` = 后端默认
```

§8 选项控件追加:

```markdown
压缩模式预设(UI 层概念, core 无"模式"):
- 最大兼容性 = (zip, 强度 9); 平衡 = (7z, 强度 5); 初始选中最大兼容性
- 「高级」展开区可手动改格式与强度; 手动改动后预设区显示「自定义」
- 密码提示随格式联动: zip+密码 → ZipCrypto(兼容优先, 强度弱); 7z+密码 → AES-256
```

- [ ] **Step 4: README.md**

1. 第 16-21 行后端段替换:

```markdown
Zip Tool 调用系统压缩命令, 不自带压缩实现. 压缩格式二选一:

- **zip**(默认, 最大兼容): Deflate 压缩, 所有系统都能打开. Linux 用系统 `zip` 命令, Windows 用 `7z -tzip`
- **7z**(压缩率更高): LZMA2 算法, 通常比 zip 再小 20-40%, 但需 7z 系工具解压. 两平台都用 `7z` 命令

分包使用各后端原生能力, 保持默认命名:
- Linux `zip -s`: `B.z01, B.z02, ..., B.zip`(最后一卷是 .zip)
- Windows `7z -v`(zip 格式): `B.zip.001, B.zip.002, ...`(Windows 没有系统 zip 命令)
- 7z 格式 `7z -v`: `B.7z.001, B.7z.002, ...`

密码语义随格式: zip 格式 = ZipCrypto(处处能解但强度弱); 7z 格式 = AES-256.
```

2. 例子(第 24-28 行)后补一行 `D (7z 格式) -> D.7z`,包名规则句补:`7z 格式产物为原名 + ".7z"`。

3. CLI 例子(第 60-74 行)`compress` 段后加:

```markdown
# 7z 格式(压缩率更高; 可与 --split/-p/--random-name 组合)
zip-tool compress -i D:\data -o E:\out --format 7z

# 压缩级别 0-9(0=仅存储, 9=最大; 缺省用后端默认)
zip-tool compress -i D:\data -o E:\out --level 9
```

解压注释(第 63 行)改为:`# 解压 (支持 .zip / .7z 与分卷 .zip.001 / .7z.001; 传成员卷自动归一到入口卷)`。

4. 环境要求(第 112 行)替换:

```markdown
- 压缩/解压通过外部命令完成: Windows 需 7z (7-Zip 或 NanaZip 均可); Linux 的 zip 格式需系统 zip / unzip, 7z 格式与解 .7z 需 p7zip 的 7z
```

- [ ] **Step 5: compress-modes.md 勘误**

第 4 节工具要求行:

旧:`- **工具要求**: Linux 解 \`.7z\` 需 p7zip 的 \`7z\` 命令; spawn 失败(不在 PATH)走既有 BackendFailed 路径, 错误信息列出所需工具名. README 工具矩阵与 doctor 体现`

新:`- **工具要求**: Linux 解 \`.7z\` 需 p7zip 的 \`7z\` 命令; 压缩/解压统一在 PATH 预定位工具, 缺失报 \`ToolMissing\`(与解压既有行为一致). README 工具矩阵与 doctor 体现`

- [ ] **Step 6: 最终回归与提交**

Run: `cargo test --workspace && cd crates/tauri-app && npm test`
Expected: 全绿

```bash
git add -A
git commit -m "docs: 同步 compress/extract/ui-backend/README 至格式双模式, 规格勘误

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## 收尾(计划外, 提醒执行者)

- GUI 真机走查(用户实跑 `cargo run -p zip-tool-tauri`):压缩两预设各跑一次 + 密码提示 + 解压一个 7z 格式产物目录。此项属用户手测, 不在自动化范围。
- Linux 实机回归(有条件时):zip 格式既有用例 + doctor 三工具输出。
