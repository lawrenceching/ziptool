# 进度流(7z 百分比 + 解压条目信号)Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 压缩(7z 系)条目内出现平滑百分比进度,解压组内出现按预扫大小加权的字节增量进度;CLI/Tauri 透传,壳层零改动。

**Architecture:** 新增私有模块 `sevenz_progress` 解析 7z `-bso0 -bsp1 -bse1` 的 stdout 进度流(`\r` 分段帧)。`Backend::run_streaming` 用 mpsc 把读者线程的帧汇合回主轮询线程再回调(公共 API 的 `&dyn Fn` 不需要 Sync)。压缩端经 `CommandBackend::run_with_percent` 把百分比 × 扫描期大小映射进现有字节加权;解压端 `ExtractBackend::extract` 改名 `extract_with_entries`(回调条目名),`extract_groups` 用预扫清单(`ArchiveListing.sizes`)加权。unzip/zip(Info-ZIP)无流式进度,全部路径退化为现状(组级),行为不变。

**Tech Stack:** Rust(core/cli crate)、7z/NanaZip 实测协议、既有 TDD 设施(FakeBackend/FakeExtract)。

---

## 背景实测结论(已验证,写代码前必读)

1. `7z a/x -bso0 -bsp1 -bse1` 下 stdout **只剩进度段**,以 `\r` 分隔(整流可无 `\n`),形如 `  45% 2 + Novel\book.txt`(`+`=添加 / `-`=解出;名字可含空格、可缺省),约 100ms 一帧。
2. `-tzip` 压缩收尾百分比会**回退**(实测 98%→37%),必须钳位单调。
3. 7z 解压 zip 的百分比随版本/形态波动(NanaZip 只出 0%、96%),**不可当硬依赖**;解压用"预扫大小 + 条目名首见累计"。
4. **WARNING/ERROR 即使 `-bse1` 也可能走 stdout**(实测 `7z a ... no_such_file` 的 WARNING 在 stdout,stderr 为空)→ 解析器收集非进度尾巴,失败时 stderr 为空则兜底用它。
5. Info-ZIP zip/unzip 管道下无任何流式进度(实测),维持条目级现状。
6. CI 的 p7zip 已在 `verify_command` 使用 `-bso0/-bsp0`,证明 unix 支持 `-bs*` 家族;NanaZip 成功路径 stdout 纯进度、stderr 空(已实测)。

## 文件结构

| 文件 | 动作 | 职责 |
|---|---|---|
| `_docs/progress.md` | 新建 | 进度协议规格(实测结论 + 线程模型) |
| `crates/core/src/sevenz_progress.rs` | 新建 | 进度流字节 → `SevenZTick` 解析器(纯函数,可单测) |
| `crates/core/src/lib.rs` | 改 1 行 | `mod sevenz_progress;`(私有) |
| `crates/core/src/backend.rs` | 改 | `run_streaming`、命令加 `-bs*` flags、`run_with_percent`、`ArchiveListing.sizes`、`extract` → `extract_with_entries` |
| `crates/core/src/compress.rs` | 改 | 压缩循环 tick → 字节加权进度 |
| `crates/core/src/extract.rs` | 改 | `extract_groups` 条目信号 → 跨组字节加权 |
| `crates/core/src/progress.rs` | 改注释 | `Phase::Extracting` 文档 |
| `crates/cli/src/main.rs` | 改 | 解压进度显示字节 |
| `test/tests/cli_e2e.rs` | 改 | 进度流 e2e 回归 |

工作在 `main` 分支直接提交(项目惯例)。

---

### Task 1: 规格文档 + `sevenz_progress` 解析器

**Files:**
- Create: `_docs/progress.md`
- Create: `crates/core/src/sevenz_progress.rs`
- Modify: `crates/core/src/lib.rs`(模块声明区)

- [ ] **Step 1.1: 写规格文档 `_docs/progress.md`**

```markdown
# 进度流协议(7z -bso0 -bsp1 -bse1)

## 实测协议(NanaZip 25.01 / 7-Zip 23.01,2026-10)

- `7z a|x` 加 `-bso0 -bsp1 -bse1` 后 stdout 只剩进度段,以 `\r` 分隔(整流可无 `\n`):
  `  45% 2 + Novel\book.txt`(`+`=添加 / `-`=解出;名字可含空格、可缺省),约 100ms 一帧。
- 百分比在 `-tzip` 压缩收尾会回退(98%→37%):解析器钳位单调。
- 7z 解压 zip 的百分比随版本波动(NanaZip 对 .zip 只报 0%、96%):解压不依赖百分比,
  用"预扫大小(l -ba -slt 的 Size =)+ stdout 条目名首见累计"。
- WARNING/ERROR 即使 -bse1 也可能走 stdout:解析器收集最近 4KB 非进度文本,
  失败且 stderr 为空时兜底作错误输出(镜像 run_impl 的 stdout fallback 语义)。

## 线程模型

读者线程解析 stdout 并把 tick 发进 mpsc;主轮询线程(100ms POLL_INTERVAL)drain
channel 后在本线程回调。公共 API 的 `&dyn Fn` 回调因此不需要 `Sync`。
取消仍走 `CancelFlag` 轮询 kill(tick 不响应回调 Break)。

## 映射

- 压缩(7z 系):百分比 × 扫描期条目大小 → `Progress.done_bytes` 字节加权(见 compress.md §7)。
- 解压(7z 系):`l -ba -slt` 预扫每条目大小为权重,stdout 条目名首见 → 累加并上报。
- zip/unzip(Info-ZIP):无流式进度,维持条目/组级现状。
```

- [ ] **Step 1.2: 写失败测试 — 新建 `crates/core/src/sevenz_progress.rs`,先只有测试骨架**

```rust
//! 7z 进度流解析(协议见 _docs/progress.md):
//! `-bso0 -bsp1 -bse1` 下 stdout 只剩 `\r` 分段的进度帧
//! `  45% 2 + Novel\book.txt`。两个实测坑:压缩收尾百分比回退(钳位单调);
//! WARNING/ERROR 走 stdout(非进度尾巴兜底报错)。

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(p: &mut SevenZProgressParser, text: &str) -> Vec<SevenZTick> {
        let mut ticks = Vec::new();
        p.feed(text.as_bytes(), &mut |t| ticks.push(t.clone()));
        ticks
    }

    #[test]
    fn parses_percent_and_entry_from_carriage_return_stream() {
        let mut p = SevenZProgressParser::new();
        let ticks = feed_all(&mut p, "  45% 2 + Novel\\book.txt\r  46% 2 + Novel\\book.txt\r");
        assert_eq!(
            ticks,
            [
                SevenZTick { percent: Some(45), entry: Some("Novel/book.txt".into()) },
                SevenZTick { percent: Some(46), entry: Some("Novel/book.txt".into()) },
            ]
        );
    }

    #[test]
    fn keeps_incomplete_tail_until_complete() {
        let mut p = SevenZProgressParser::new();
        assert!(feed_all(&mut p, "  4").is_empty());
        let ticks = feed_all(&mut p, "5% 1 + a.txt\r");
        assert_eq!(ticks, [SevenZTick { percent: Some(45), entry: Some("a.txt".into()) }]);
    }

    #[test]
    fn clamps_percent_regression_monotonically() {
        // 实测: -tzip 压缩收尾 98% → 37%, 钳位后不倒退
        let mut p = SevenZProgressParser::new();
        let ticks = feed_all(&mut p, "  98% 1 + a.bin\r  37% 1 + a.bin\r");
        assert_eq!(
            ticks,
            [
                SevenZTick { percent: Some(98), entry: Some("a.bin".into()) },
                SevenZTick { percent: Some(98), entry: Some("a.bin".into()) },
            ]
        );
    }

    #[test]
    fn percent_without_entry_yields_none_entry() {
        let mut p = SevenZProgressParser::new();
        let ticks = feed_all(&mut p, "  45%\r");
        assert_eq!(ticks, [SevenZTick { percent: Some(45), entry: None }]);
    }

    #[test]
    fn minus_marker_and_trailing_spaces_parse() {
        // 解压帧: "  30% 5 - Novel/book.txt"(名字后的空格丢弃)
        let mut p = SevenZProgressParser::new();
        let ticks = feed_all(&mut p, "  30% 5 - Novel/book.txt  \r");
        assert_eq!(ticks, [SevenZTick { percent: Some(30), entry: Some("Novel/book.txt".into()) }]);
    }

    #[test]
    fn banner_and_error_text_collected_in_other_tail() {
        let mut p = SevenZProgressParser::new();
        feed_all(&mut p, "0M Scan\r  45% 1 + a.txt\r");
        feed_all(&mut p, "WARNING: no such file\r");
        let tail = p.finish(&mut |_| {});
        assert!(tail.contains("0M Scan"), "{tail}");
        assert!(tail.contains("WARNING: no such file"), "{tail}");
    }

    #[test]
    fn non_utf8_bytes_become_lossy_without_panic() {
        let mut p = SevenZProgressParser::new();
        let ticks = feed_all(&mut p, b"  45% 1 + \xff\xfe.bin\r");
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].percent, Some(45));
        assert!(ticks[0].entry.as_deref().unwrap_or("").contains('\u{FFFD}'));
    }

    #[test]
    fn percent_over_100_is_treated_as_other_text() {
        let mut p = SevenZProgressParser::new();
        feed_all(&mut p, "  145% 1 + x\r");
        let tail = p.finish(&mut |_| {});
        assert!(tail.contains("145%"), "{tail}");
    }

    #[test]
    fn empty_segments_are_ignored() {
        let mut p = SevenZProgressParser::new();
        assert!(feed_all(&mut p, "\r\r\r").is_empty());
        assert!(p.finish(&mut |_| {}).trim().is_empty());
    }

    #[test]
    fn finish_flushes_remaining_segment_without_cr() {
        let mut p = SevenZProgressParser::new();
        feed_all(&mut p, "  45% 1 + a.txt");
        let mut ticks = Vec::new();
        p.finish(&mut |t| ticks.push(t.clone()));
        assert_eq!(ticks, [SevenZTick { percent: Some(45), entry: Some("a.txt".into()) }]);
    }
}
```

并在 `crates/core/src/lib.rs` 模块声明区(其他 `mod` 旁边)加:

```rust
mod sevenz_progress;
```

- [ ] **Step 1.3: 跑测试确认编译失败**

Run: `cargo test -p zip-tool-core sevenz_progress`
Expected: FAIL — `SevenZProgressParser`/`SevenZTick` 未定义

- [ ] **Step 1.4: 实现解析器(文件顶部,tests 模块之前)**

```rust
/// 一帧进度:百分比与条目名都可有可无(7z 只在变化时输出对应部分)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SevenZTick {
    pub percent: Option<u8>,
    pub entry: Option<String>,
}

/// 非进度尾巴(WARNING/ERROR 兜底)保留上限
const OTHER_TAIL_CAP: usize = 4096;

pub(crate) struct SevenZProgressParser {
    buf: Vec<u8>,
    /// 已见最大百分比(压缩收尾会回退, 钳位保证进度单调不倒退)
    max_percent: Option<u8>,
    /// 最近 4KB 的非进度文本(WARNING/ERROR 走 stdout 时兜底报错用)
    other_tail: String,
}

impl SevenZProgressParser {
    pub(crate) fn new() -> Self {
        Self { buf: Vec::new(), max_percent: None, other_tail: String::new() }
    }

    /// 喂一段 stdout;完整的 `\r` 段立即结算,不完整尾部留待下次
    pub(crate) fn feed(&mut self, chunk: &[u8], on_tick: &mut dyn FnMut(&SevenZTick)) {
        self.buf.extend_from_slice(chunk);
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\r') {
            let seg: Vec<u8> = self.buf.drain(..=pos).collect();
            self.take_segment(&seg[..seg.len() - 1], on_tick);
        }
    }

    /// 流结束:结算最后一段(可能不带 `\r`),返回非进度尾巴供失败兜底
    pub(crate) fn finish(&mut self, on_tick: &mut dyn FnMut(&SevenZTick)) -> String {
        let rest = std::mem::take(&mut self.buf);
        self.take_segment(&rest, on_tick);
        std::mem::take(&mut self.other_tail)
    }

    fn take_segment(&mut self, seg: &[u8], on_tick: &mut dyn FnMut(&SevenZTick)) {
        let text = String::from_utf8_lossy(seg).into_owned();
        match parse_fields(&text) {
            Some(mut tick) => {
                // 钳位单调: -tzip 压缩收尾实测会回退(98%→37%)
                tick.percent = tick.percent.map(|p| {
                    let m = self.max_percent.unwrap_or(0).max(p);
                    self.max_percent = Some(m);
                    m
                });
                on_tick(&tick);
            }
            None => {
                if !text.trim().is_empty() {
                    self.other_tail.push_str(&text);
                    self.other_tail.push('\n');
                    let len = self.other_tail.len();
                    if len > OTHER_TAIL_CAP {
                        let cut = len - OTHER_TAIL_CAP;
                        let boundary = (cut..=len)
                            .find(|i| self.other_tail.is_char_boundary(*i))
                            .unwrap_or(len);
                        self.other_tail.drain(..boundary);
                    }
                }
            }
        }
    }
}

/// 解析一帧:`  45% 2 + Novel/book.txt` → (45, "Novel/book.txt");不是进度帧返回 None
fn parse_fields(seg: &str) -> Option<SevenZTick> {
    let seg = seg.trim_start();
    let pct_end = seg.find('%')?;
    let digits = seg[..pct_end].trim();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let percent = digits.parse::<u8>().ok().filter(|p| *p <= 100)?;
    // 剩余形如 "2 + Novel/book.txt" / "2 -" / "":取 +/- 标记后的名字
    let rest = seg[pct_end + 1..].trim();
    let entry = rest
        .split_once(['+', '-'])
        .and_then(|(_, name)| {
            let name = name.trim();
            (!name.is_empty()).then(|| name.replace('\\', "/"))
        });
    Some(SevenZTick { percent: Some(percent), entry })
}
```

- [ ] **Step 1.5: 跑测试确认通过**

Run: `cargo test -p zip-tool-core sevenz_progress`
Expected: PASS(11 个用例)

- [ ] **Step 1.6: 提交**

```bash
git add _docs/progress.md crates/core/src/sevenz_progress.rs crates/core/src/lib.rs
git commit -m "feat(core): 7z 进度流解析器(实测协议, 百分比钳位单调)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 2: `run_streaming` + 命令进度 flags + `run_with_percent`

**Files:**
- Modify: `crates/core/src/backend.rs`(顶部 use、`run` 附近加 `run_streaming`、`compress_command`/`extract_command` SevenZip 分支、`CommandBackend` trait + impl、`run_tests`/`streaming_tests`)

- [ ] **Step 2.1: 写失败测试 — `backend.rs` 文件末尾(`run_tests` 模块之后)加新测试模块,并把 `run_tests` 里的三个命令 helper 提为 `pub(super)`(561/574/587 行的 `fn long_running` / `fn exit_ok` / `fn echo_boom_exit_2` 改为 `pub(super) fn`,供兄弟模块复用)**

```rust
#[cfg(test)]
mod streaming_tests {
    //! 进度流执行:成功帧回调 / 失败归因(含 stdout 兜底)/ 取消 kill。
    use super::*;
    use super::run_tests::{echo_boom_exit_2, exit_ok, long_running};
    use crate::CancelFlag;
    use std::time::{Duration, Instant};

    /// 往 stdout 打一帧进度后成功退出(Windows echo 会补 \n, 由 finish 结算)
    fn print_progress_frames() -> Command {
        if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "echo  42% 1 + a.txt"]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", "printf '  42%% 1 + a.txt\\r'"]);
            c
        }
    }

    /// 往 stdout 打 WARNING 后以 2 退出(stderr 空):验证 other_tail 兜底
    fn warn_to_stdout_exit_2() -> Command {
        if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "echo WARNING: no such file & exit 2"]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", "printf 'WARNING: no such file\\n'; exit 2"]);
            c
        }
    }

    #[test]
    fn streaming_success_emits_ticks() {
        let mut ticks = Vec::new();
        let r = Backend::SevenZip.run_streaming(
            &mut print_progress_frames(),
            &CancelFlag::new(),
            &mut |t| ticks.push(t.clone()),
        );
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(
            ticks,
            [SevenZTick { percent: Some(42), entry: Some("a.txt".into()) }]
        );
    }

    #[test]
    fn streaming_failure_attributes_stderr() {
        // 复用 run_tests 的 echo_boom_exit_2():boom 走 stderr
        let r = Backend::SevenZip.run_streaming(&mut echo_boom_exit_2(), &CancelFlag::new(), &mut |_| {});
        match r {
            Err(CoreError::BackendFailed { program, output, .. }) => {
                assert_eq!(program, echo_boom_exit_2().get_program().to_string_lossy());
                assert!(output.contains("boom"), "{output}");
            }
            o => panic!("应为 BackendFailed: {o:?}"),
        }
    }

    #[test]
    fn streaming_failure_falls_back_to_stdout_tail() {
        // -bse1 也拦不住 WARNING 走 stdout(实测):stderr 空时用 other_tail 兜底
        let r = Backend::SevenZip.run_streaming(&mut warn_to_stdout_exit_2(), &CancelFlag::new(), &mut |_| {});
        match r {
            Err(CoreError::BackendFailed { output, .. }) => {
                assert!(output.contains("WARNING: no such file"), "{output}");
            }
            o => panic!("应为 BackendFailed: {o:?}"),
        }
    }

    #[test]
    fn streaming_cancel_kills_child_and_returns_cancelled() {
        let cancel = CancelFlag::new();
        let c = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            c.cancel();
        });
        let start = Instant::now();
        let r = Backend::SevenZip.run_streaming(&mut long_running(), &cancel, &mut |_| {});
        assert!(matches!(r, Err(CoreError::Cancelled)), "实际: {r:?}");
        assert!(start.elapsed() < Duration::from_secs(30), "取消应远早于命令自然结束");
    }

    #[test]
    fn zip_backend_run_with_percent_emits_no_ticks() {
        let mut ticks = Vec::new();
        let r = Backend::Zip.run_with_percent(&mut exit_ok(), &CancelFlag::new(), &mut |p| ticks.push(p));
        assert!(r.is_ok(), "{r:?}");
        assert!(ticks.is_empty(), "Info-ZIP 无流式进度: {ticks:?}");
    }
}
```

- [ ] **Step 2.2: 跑测试确认编译失败**

Run: `cargo test -p zip-tool-core streaming_tests`
Expected: FAIL — `run_streaming`/`run_with_percent` 未定义

- [ ] **Step 2.3: 实现 `run_streaming` — `backend.rs` 顶部 use 区加导入,`run_impl` 之后加方法**

use 区(现有 `use` 语句之后)加:

```rust
use crate::sevenz_progress::{SevenZProgressParser, SevenZTick};
```

`Backend::run_impl` 之后(`verify_program` 之前)加:

```rust
    /// 流式执行外部命令:stdout 按 7z 进度流协议解析(命令需带 -bso0 -bsp1 -bse1,
    /// 见 _docs/progress.md),每帧回调 on_tick;非零退出与 [`Backend::run`] 同语义,
    /// stderr 为空时用解析器的非进度尾巴兜底(WARNING 走 stdout 的实测坑)。
    /// 线程模型:读者线程只把 tick 发进 mpsc,回调在本次轮询线程触发 ——
    /// 公共 API 的 `&dyn Fn` 回调因此不需要 Sync。
    pub(crate) fn run_streaming(
        &self,
        cmd: &mut Command,
        cancel: &CancelFlag,
        on_tick: &mut dyn FnMut(&SevenZTick),
    ) -> Result<()> {
        no_window(cmd);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        let t_err = drain_pipe(child.stderr.take());
        let (tx, rx) = std::sync::mpsc::channel::<SevenZTick>();
        let stdout = child.stdout.take();
        let reader = std::thread::spawn(move || {
            let mut parser = SevenZProgressParser::new();
            if let Some(mut out) = stdout {
                let mut buf = [0u8; 8192];
                loop {
                    match out.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => parser.feed(&buf[..n], &mut |t| {
                            let _ = tx.send(t.clone());
                        }),
                    }
                }
            }
            parser.finish(&mut |t| {
                let _ = tx.send(t.clone());
            })
        });
        loop {
            if cancel.is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                let _ = t_err.join();
                let _ = reader.join();
                return Err(CoreError::Cancelled);
            }
            // 读者线程攒的帧在本线程回调(回调方拿到的 &dyn Fn 无需跨线程)
            while let Ok(tick) = rx.try_recv() {
                on_tick(&tick);
            }
            match child.try_wait()? {
                Some(status) => {
                    // EOF 已到, join 不会久等;残余帧回调完再归因
                    let other_tail = reader.join().unwrap_or_default();
                    while let Ok(tick) = rx.try_recv() {
                        on_tick(&tick);
                    }
                    let err_buf = t_err.join().unwrap_or_default();
                    if !status.success() {
                        let mut text = String::from_utf8_lossy(&err_buf).into_owned();
                        if text.trim().is_empty() {
                            text = other_tail;
                        }
                        return Err(CoreError::BackendFailed {
                            program: cmd.get_program().to_string_lossy().into_owned(),
                            status: status.to_string(),
                            output: last_lines(&text, 5),
                        });
                    }
                    return Ok(());
                }
                None => std::thread::sleep(POLL_INTERVAL),
            }
        }
    }
```

- [ ] **Step 2.4: 压缩命令加进度 flags — `compress_command` 的 `Backend::SevenZip` 分支,`-mx=` level 块之后、排除开关循环之前插入**

```rust
                // 进度流: 关 stdout 文件清单/错误流, 开进度(实测协议见 _docs/progress.md)
                cmd.args(["-bso0", "-bsp1", "-bse1"]);
```

- [ ] **Step 2.5: 解压命令加进度 flags — `extract_command` 的 `Backend::SevenZip` 分支,`-o{dest}` 之后、`--` 之前插入**

```rust
                cmd.arg(format!("-o{}", dest.display()));
                // 进度流(同压缩端): x 模式下 stdout 只剩 "  N% - name" 帧
                cmd.args(["-bso0", "-bsp1", "-bse1"]);
                cmd.arg("--").arg(archive);
```

- [ ] **Step 2.6: `CommandBackend` trait 加 `run_with_percent` 默认方法(`check_split_size` 之前),`impl CommandBackend for Backend` 加 SevenZip 覆写**

trait 内:

```rust
    /// 执行压缩命令;7z 系后端把百分比进度帧(0-100, 已钳位单调)回调给 on_percent,
    /// 其他实现退化为一次性 run(无条目内进度)
    fn run_with_percent(
        &self,
        cmd: &mut Command,
        cancel: &crate::CancelFlag,
        on_percent: &mut dyn FnMut(u8),
    ) -> Result<()> {
        let _ = on_percent;
        self.run(cmd, cancel)
    }
```

impl 内(`run` 之后):

```rust
    fn run_with_percent(
        &self,
        cmd: &mut Command,
        cancel: &crate::CancelFlag,
        on_percent: &mut dyn FnMut(u8),
    ) -> Result<()> {
        match self {
            Backend::SevenZip => self.run_streaming(cmd, cancel, &mut |t| {
                if let Some(p) = t.percent {
                    on_percent(p);
                }
            }),
            Backend::Zip => Backend::run(self, cmd, cancel),
        }
    }
```

- [ ] **Step 2.7: 更新既有命令断言(全部在 `backend.rs` tests;新 flags 统一插在"级别/`-o` 之后、排除开关/`--` 之前")**

| 测试 | `args_of(&cmd)` 新期望 |
|---|---|
| `sevenzip_command_emits_exclude_before_separator` | `["a","-t7z","-m0=LZMA2","-bso0","-bsp1","-bse1","-xr!Thumbs.db","-x!A{s}sub{s}*.log","--","E:/out/A.7z","A"]`(`{s}`=分隔符,沿用该测试现有的分隔符插值写法) |
| `sevenzip_command_7z_format_emits_lzma2` | `["a","-t7z","-m0=LZMA2","-bso0","-bsp1","-bse1","--","E:/out/A.7z","A"]` |
| `sevenzip_command_7z_format_with_split_password_level` | `["a","-t7z","-m0=LZMA2","-v2g","-ppw","-mx=9","-bso0","-bsp1","-bse1","--","E:/out/B.7z","B"]` |
| `sevenzip_command_zip_format_level_uses_mx` | `["a","-tzip","-mx=9","-bso0","-bsp1","-bse1","--","E:/out/A.zip","A"]` |
| `windows_default_has_no_name_based_hidden_switches` | `["a","-tzip","-bso0","-bsp1","-bse1","--","E:/out/A.zip","A"]` |
| `sevenzip_compress_command_runs_in_work_dir_with_relative_name` | `["a","-tzip","-bso0","-bsp1","-bse1","--","E:/out/A.zip","A"]` |
| `sevenzip_compress_command_with_password_and_split` | `["a","-tzip","-v2g","-ppw","-bso0","-bsp1","-bse1","--","E:/out/B.zip","B"]` |
| `sevenzip_extract_command_on_first_volume` | `["x","-aoa","-oD:\\d","-bso0","-bsp1","-bse1","--","B.zip.001"]` |

基于 contains/`iter().any()` 断言的用例(如 `sevenzip_maps_doublestar_suffix_to_dir_switch`、`hidden_attribute_paths_become_exact_switches`)无需改动。

- [ ] **Step 2.8: 跑测试,修掉残余的命令断言**

Run: `cargo test --workspace`
Expected: PASS。若 compress.rs 等处还有精确断言 7z args 的用例失败,按 Step 2.7 的位置规则补三个 flags 后重跑。

- [ ] **Step 2.9: 提交**

```bash
git add crates/core/src/backend.rs
git commit -m "feat(core): 流式执行后端命令, 7z 压缩/解压命令走进度流

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 3: 压缩条目内进度(百分比 × 扫描权重)

**Files:**
- Modify: `crates/core/src/compress.rs:174-187`(主循环)与 tests 模块(新 `PercentBackend` + 1 用例)

- [ ] **Step 3.1: 写失败测试 — `compress.rs` 的 `mod fake_tests`(`FakeBackend` impl 之后、`two_entries_fixture` 之前)加假后端与用例**

```rust
    /// run 时发一帧 50% 的假后端:验证百分比 → 字节加权映射(委托其余行为给 FakeBackend)
    struct PercentBackend(FakeBackend);

    impl PercentBackend {
        fn new(fail_on: &[&str]) -> Self {
            Self(FakeBackend::new(fail_on))
        }
    }

    impl CommandBackend for PercentBackend {
        fn compress_command(
            &self,
            work: &Path,
            name: &str,
            out_archive: &Path,
            opts: &Options,
            attr_hidden: &[String],
        ) -> Result<Command> {
            self.0.compress_command(work, name, out_archive, opts, attr_hidden)
        }

        fn run(&self, cmd: &mut Command, cancel: &crate::CancelFlag) -> Result<()> {
            self.0.run(cmd, cancel)
        }

        fn verify(&self, entry: &Path, opts: &Options, cancel: &crate::CancelFlag) -> Result<()> {
            self.0.verify(entry, opts, cancel)
        }

        fn check_split_size(&self, size: Option<u64>) -> Result<()> {
            self.0.check_split_size(size)
        }

        fn run_with_percent(
            &self,
            _cmd: &mut Command,
            _cancel: &crate::CancelFlag,
            on_percent: &mut dyn FnMut(u8),
        ) -> Result<()> {
            on_percent(50);
            Ok(())
        }
    }

    #[test]
    fn percent_ticks_map_into_byte_weighted_progress() {
        // two_entries_fixture: A=3 字节, B=4 字节, 总 7
        let root = two_entries_fixture("percent-tick");
        let entries = [
            Entry {
                name: "A".into(),
                kind: crate::scan::EntryKind::Dir,
                size: 3,
                dst: "A".into(),
                format: String::new(),
                exclude: None,
                attr_hidden: Vec::new(),
            },
            Entry {
                name: "B".into(),
                kind: crate::scan::EntryKind::Dir,
                size: 4,
                dst: "B".into(),
                format: String::new(),
                exclude: None,
                attr_hidden: Vec::new(),
            },
        ];
        let events = RefCell::new(Vec::new());
        let report = compress_entries_with_backend(
            &PercentBackend::new(&[]),
            &root.join("in"),
            &root.join("out"),
            &entries,
            &rnd(),
            &CancelFlag::new(),
            &|p| {
                events.borrow_mut().push(p.clone());
                ControlFlow::Continue(())
            },
        )
        .unwrap();
        assert_eq!(report.entries.len(), 2);
        let ev = events.borrow();
        let compressing: Vec<_> =
            ev.iter().filter(|p| p.phase == Phase::Compressing).collect();
        // A 50% → 3*50/100=1;A 完成 → 3;B 50% → 3+4*50/100=5;B 完成 → 7
        assert_eq!(
            compressing
                .iter()
                .map(|p| (p.done, p.done_bytes, p.total_bytes))
                .collect::<Vec<_>>(),
            [(0, 1, 7), (1, 3, 7), (1, 5, 7), (2, 7, 7)],
            "{ev:?}"
        );
        fs::remove_dir_all(&root).unwrap();
    }
```

(`Entry` 是 `crate::scan::Entry`,7 个字段全列如上;`fake_tests` 模块经 `use super::*` 已有 `Entry`/`RefCell`/`ControlFlow`,与既有编排用例一致。)

- [ ] **Step 3.2: 跑测试确认失败**

Run: `cargo test -p zip-tool-core percent_ticks_map`
Expected: FAIL — 压缩循环还在调 `backend.run(...)`,50% tick 没人消费;实际只有 2 帧 `[(1,3,7),(2,7,7)]`,断言期望 4 帧

- [ ] **Step 3.3: 改压缩主循环 — `compress_entries_with_backend` 中 `let out_archive = ...` 与 `.and_then` 链替换为**

```rust
    for (i, e) in entries.iter().enumerate() {
        let out_archive = out_abs.join(format!("{}.{}", e.dst, ext));
        let base_bytes = done_bytes;
        let entry_size = e.size;
        let result = backend
            .compress_command(&input_abs, &e.name, &out_archive, opts, &e.attr_hidden)
            .and_then(|mut cmd| {
                backend.run_with_percent(&mut cmd, cancel, &mut |pct| {
                    // 条目内进度: 扫描期大小 × 百分比(7z 帧约 10Hz; 0 字节条目无帧, 跳过)。
                    // tick 不响应回调 Break: 条目内取消仍走 CancelFlag(kill 子进程)
                    if entry_size > 0 {
                        let _ = on_progress(&Progress {
                            phase: Phase::Compressing,
                            done: i,
                            total,
                            done_bytes: base_bytes + entry_size * u64::from(pct) / 100,
                            total_bytes,
                            entry: e.name.clone(),
                        });
                    }
                })
            })
            .and_then(|()| {
                // 压缩成功后、记入产物前做结构校验:坏包不进入 Report/mapping
                let archive = resolve_primary_archive(&out_abs, &e.dst, ext);
                if opts.verify {
                    backend.verify(&archive, opts, cancel).map(|()| archive)
                } else {
                    Ok(archive)
                }
            });
```

(循环其余部分 — 错误处理/mapping/完成后 `on_progress` — 保持不变。)

- [ ] **Step 3.4: 跑测试确认通过(含既有编排用例不回归 — FakeBackend 走默认 `run_with_percent`,无额外帧)**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 3.5: 提交**

```bash
git add crates/core/src/compress.rs
git commit -m "feat(core): 压缩条目内字节级进度(百分比×扫描权重)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 4: 解压组内进度(预扫清单加权 + 条目名信号)

**Files:**
- Modify: `crates/core/src/backend.rs:759-807`(`ArchiveListing` + `parse_listing` + listing_tests)、`ExtractBackend` trait 与 `RealExtract`(extract → extract_with_entries)
- Modify: `crates/core/src/extract.rs:320-385`(`extract_groups`)与 orch_tests(`FakeExtract` + 2 用例)
- Modify: `crates/core/src/progress.rs:10-11`(注释)

- [ ] **Step 4.1: 写失败测试 — `backend.rs` `listing_tests` 更新与新增**

更新 `parses_7z_slt_blocks_and_normalizers_separators` 用例(fixture 文本已含 `Size =` 行),在其 `assert_eq!(l.files, 2);` 后加:

```rust
        assert_eq!(l.sizes, [0, 2_500_000, 200_000], "目录为 0, 文件取 Size =");
```

更新 `parses_unzip_z1_names_with_trailing_slash_dirs`,在 `assert_eq!(l.files, 3);` 后加:

```rust
        assert_eq!(l.sizes, [0, 0, 0, 0, 0], "zipinfo 清单无大小");
```

更新 `empty_listing_text_yields_empty`,加:

```rust
        assert!(l.sizes.is_empty());
```

新增用例:

```rust
    #[test]
    fn unparsable_size_falls_back_to_zero() {
        let text = "Path = x\nFolder = -\nSize = abc\n";
        let l = parse_listing(&Backend::SevenZip, text);
        assert_eq!(l.sizes, [0]);
    }
```

- [ ] **Step 4.2: 跑测试确认失败**

Run: `cargo test -p zip-tool-core listing_tests`
Expected: FAIL — `sizes` 字段不存在

- [ ] **Step 4.3: 实现 `sizes` — `ArchiveListing` 结构加字段,`parse_listing` 捕获 `Size =`**

```rust
/// 包内条目清单(解压预检的数据源, 见 _docs/extract.md 5.1)
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveListing {
    /// 全部条目的包内相对路径(含目录), 分隔符归一为 `/`, 目录名以 `/` 结尾
    pub names: Vec<String>,
    /// 非目录条目数
    pub files: usize,
    /// 与 names 平行的原始大小(目录为 0; unzip -Z1 清单无大小, 全 0)。
    /// 解压进度的预扫加权基准, 见 _docs/progress.md
    pub sizes: Vec<u64>,
}
```

`parse_listing` 整体替换为:

```rust
pub(crate) fn parse_listing(backend: &Backend, text: &str) -> ArchiveListing {
    let mut names: Vec<String> = Vec::new();
    let mut sizes: Vec<u64> = Vec::new();
    match backend {
        Backend::Zip => {
            for line in text.lines() {
                let name = line.replace('\\', "/");
                if !name.trim().is_empty() {
                    names.push(name);
                    sizes.push(0);
                }
            }
        }
        Backend::SevenZip => {
            let mut cur: Option<(String, bool, u64)> = None;
            let flush = |cur: &mut Option<(String, bool, u64)>,
                         names: &mut Vec<String>,
                         sizes: &mut Vec<u64>| {
                if let Some((path, is_dir, size)) = cur.take() {
                    names.push(if is_dir { format!("{path}/") } else { path });
                    sizes.push(if is_dir { 0 } else { size });
                }
            };
            for line in text.lines() {
                let line = line.trim_end();
                if let Some(v) = line.strip_prefix("Path = ") {
                    flush(&mut cur, &mut names, &mut sizes);
                    cur = Some((v.trim().replace('\\', "/"), false, 0));
                } else if line == "Folder = +" {
                    if let Some((_, is_dir, _)) = &mut cur {
                        *is_dir = true;
                    }
                } else if let Some(v) = line.strip_prefix("Size = ") {
                    // "Packed Size = " 前缀不同, 不会误配
                    if let Some((_, _, size)) = &mut cur {
                        *size = v.trim().parse().unwrap_or(0);
                    }
                } else if line.is_empty() {
                    flush(&mut cur, &mut names, &mut sizes);
                }
            }
            flush(&mut cur, &mut names, &mut sizes);
        }
    }
    let files = names.iter().filter(|n| !n.ends_with('/')).count();
    ArchiveListing { names, files, sizes }
}
```

- [ ] **Step 4.4: 跑测试确认通过**

Run: `cargo test -p zip-tool-core listing_tests`
Expected: listing_tests 全 PASS。此时 extract.rs 会报编译错(`FakeExtract::list` 构造 `ArchiveListing` 缺 `sizes`、`extract` 签名不符)—— 属 Step 4.5-4.8 的连带改动,继续往下做;若想先看绿,可临时给 `FakeExtract::list` 补 `sizes: vec![0; self.listing.len()]`。

- [ ] **Step 4.5: `ExtractBackend` trait 改名签名(`backend.rs:331-338`)**

```rust
    /// 把入口卷解到 dest(冲突判断在编排层, 覆盖开关恒带);
    /// cancel 透传给进程等待, 取消即 kill;每解出一个条目回调一次名字
    /// (7z 系来自进度流帧, Info-ZIP 无信号则不回调), 供编排层做字节加权
    fn extract_with_entries(
        &self,
        kind: Backend,
        entry: &Path,
        dest: &Path,
        password: Option<&str>,
        cancel: &crate::CancelFlag,
        on_entry: &mut dyn FnMut(&str),
    ) -> Result<()>;
```

`RealExtract` 的 impl(`backend.rs:373-384`)对应替换:

```rust
    fn extract_with_entries(
        &self,
        kind: Backend,
        entry: &Path,
        dest: &Path,
        password: Option<&str>,
        cancel: &crate::CancelFlag,
        on_entry: &mut dyn FnMut(&str),
    ) -> Result<()> {
        ensure_tool_available(self.program(kind))?;
        let mut cmd = kind.extract_command(entry, dest, password);
        match kind {
            // 7z 系: 进度流帧里的条目名 → on_entry(名字已由解析器归一为 /)
            Backend::SevenZip => kind.run_streaming(&mut cmd, cancel, &mut |t| {
                if let Some(name) = &t.entry {
                    on_entry(name);
                }
            }),
            // Info-ZIP: unzip 无流式信号, 与旧行为一致
            Backend::Zip => kind.run(&mut cmd, cancel),
        }
    }
```

- [ ] **Step 4.6: `extract.rs` 的 `FakeExtract` 适配(orch_tests)**

结构体加两个字段,构造器初始化为空,加 builder;`list` 补 `sizes`;`extract` 改名并回调 ticks:

```rust
    struct FakeExtract {
        listing: Vec<String>,
        /// 与 listing 平行的条目大小(空 = 全 0), 进度加权用
        sizes: Vec<u64>,
        /// extract 时逐个回调的条目名(模拟 7z 进度流的名字信号)
        ticks: Vec<String>,
        writes: Vec<String>,
        fail: bool,
        /// extract 时写出文件后返回 Err(Cancelled), 模拟"进程被 kill"
        cancel: bool,
        calls: RefCell<Vec<&'static str>>,
    }

    impl FakeExtract {
        fn new(listing: &[&str], writes: &[&str], fail: bool) -> Self {
            Self {
                listing: s(listing),
                sizes: Vec::new(),
                ticks: Vec::new(),
                writes: s(writes),
                fail,
                cancel: false,
                calls: RefCell::new(Vec::new()),
            }
        }

        fn new_cancelling(listing: &[&str], writes: &[&str]) -> Self {
            Self {
                listing: s(listing),
                sizes: Vec::new(),
                ticks: Vec::new(),
                writes: s(writes),
                fail: false,
                cancel: true,
                calls: RefCell::new(Vec::new()),
            }
        }

        fn with_sizes(mut self, sizes: &[u64]) -> Self {
            self.sizes = sizes.to_vec();
            self
        }

        fn with_ticks(mut self, ticks: &[&str]) -> Self {
            self.ticks = s(ticks);
            self
        }
    }
```

`list` 改为:

```rust
        fn list(&self, _kind: Backend, _entry: &Path) -> Result<ArchiveListing> {
            self.calls.borrow_mut().push("list");
            let files = self.listing.iter().filter(|n| !n.ends_with('/')).count();
            let sizes = if self.sizes.is_empty() {
                vec![0; self.listing.len()]
            } else {
                self.sizes.clone()
            };
            Ok(ArchiveListing { names: self.listing.clone(), files, sizes })
        }
```

`extract` 改名(`fn extract_with_entries(`),参数表末尾加 `on_entry: &mut dyn FnMut(&str),`,函数体在 `self.calls.borrow_mut().push("extract");` 之后、写文件循环之前插入:

```rust
            for t in &self.ticks {
                on_entry(t);
            }
```

既有十余处 `FakeExtract::new/new_cancelling` 调用点**无需改动**(builder 默认无 ticks/无 sizes → 行为同旧)。

- [ ] **Step 4.7: `extract_groups` 字节加权(`extract.rs:336-383`) — B3 段整体替换**

```rust
    // B3 逐组解压(顺序执行: 机械盘/USB 上顺序 IO 优先)
    let total = groups.len();
    // 全量预扫的字节加权基准(清单无大小/无信号时退化为组级计数, 字节恒 0)
    let total_bytes: u64 = listings.iter().map(|l| l.sizes.iter().sum::<u64>()).sum();
    let mut report = ExtractReport {
        warnings,
        ..ExtractReport::default()
    };
    let mut done_bytes = 0u64;
    for (i, g) in groups.iter().enumerate() {
        let entry = g
            .original
            .clone()
            .unwrap_or_else(|| file_name_of(&g.resolved.path));
        let fire = |done: usize, bytes: u64| -> ControlFlow<()> {
            on_progress(&Progress {
                phase: Phase::Extracting,
                done,
                total,
                done_bytes: bytes,
                total_bytes,
                entry: entry.clone(),
            })
        };
        if fire(i, done_bytes) == ControlFlow::Break(()) {
            report.cancelled = true;
            break;
        }
        // 组内字节账本: 首见条目名才计重(去重靠 remove; 目录名/未知名字自动忽略)
        let mut pending: std::collections::HashMap<String, u64> = listings[i]
            .names
            .iter()
            .zip(&listings[i].sizes)
            .filter(|(_, s)| **s > 0)
            .map(|(n, s)| (n.trim_end_matches('/').to_string(), *s))
            .collect();
        let mut partial = 0u64;
        let result = be.extract_with_entries(
            g.resolved.backend,
            &g.resolved.path,
            dest,
            password,
            cancel,
            &mut |name| {
                if let Some(sz) = pending.remove(name.trim_end_matches('/')) {
                    partial += sz;
                    // tick 不响应回调 Break: 组内取消仍走 CancelFlag(kill 子进程)
                    let _ = fire(i, done_bytes + partial);
                }
            },
        );
        if let Err(err) = result {
            // 失败按清单清理本次解出的文件(--force 无法区分新旧, 不清理)
            if !force {
                cleanup_extracted(&listings[i].names, dest);
            }
            if matches!(err, crate::CoreError::Cancelled) {
                // 进程内取消: 本组回滚(不计进度/不进报告), 剩余组不再开始
                report.cancelled = true;
                break;
            }
            return Err(err);
        }
        let listing = &listings[i];
        report.groups.push(ExtractedGroup {
            entry: file_name_of(&g.resolved.path),
            original: g.original.clone(),
            files: listing.files,
        });
        done_bytes += listing.sizes.iter().sum::<u64>();
        if fire(i + 1, done_bytes) == ControlFlow::Break(()) {
            report.cancelled = true;
            break;
        }
    }
    Ok(report)
```

- [ ] **Step 4.8: 写进度新用例 — `extract.rs` orch_tests(`single_group_happy_path_reports_files_and_progress` 之后)**

```rust
    #[test]
    fn extract_reports_byte_progress_from_entry_signals() {
        let (root, g) = one_group("ex-bytes");
        let be = FakeExtract::new(
            &["Novel/", "Novel/book.txt", "photo.jpg"],
            &["Novel/book.txt", "photo.jpg"],
            false,
        )
        .with_sizes(&[0, 100, 40])
        .with_ticks(&["Novel/book.txt", "photo.jpg"]);
        let dest = root.join("restore");
        let events = RefCell::new(Vec::new());
        let report = extract_groups(
            &be,
            &[g],
            &dest,
            None,
            false,
            Vec::new(),
            &CancelFlag::new(),
            &|p| {
                events.borrow_mut().push(p.clone());
                ControlFlow::Continue(())
            },
        )
        .unwrap();
        assert_eq!(report.groups.len(), 1);
        let ev = events.borrow();
        // 预扫总 140;首见名字逐个加 size;目录(0)与组完成帧不重复计
        assert_eq!(
            ev.iter()
                .map(|p| (p.done, p.done_bytes, p.total_bytes))
                .collect::<Vec<_>>(),
            [(0, 0, 140), (0, 100, 140), (0, 140, 140), (1, 140, 140)],
            "{ev:?}"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn duplicate_entry_signals_are_not_double_counted() {
        let (root, g) = one_group("ex-dup");
        let be = FakeExtract::new(&["a.txt"], &["a.txt"], false)
            .with_sizes(&[50])
            .with_ticks(&["a.txt", "a.txt"]);
        let events = RefCell::new(Vec::new());
        extract_groups(
            &be,
            &[g],
            &root.join("r"),
            None,
            false,
            Vec::new(),
            &CancelFlag::new(),
            &|p| {
                events.borrow_mut().push(p.clone());
                ControlFlow::Continue(())
            },
        )
        .unwrap();
        let ev = events.borrow();
        assert_eq!(
            ev.iter().map(|p| p.done_bytes).collect::<Vec<_>>(),
            [0, 50, 50],
            "同一名字第二次信号不再计重: {ev:?}"
        );
        fs::remove_dir_all(&root).unwrap();
    }
```

- [ ] **Step 4.9: 更新 `progress.rs` 注释(`Phase::Extracting`)**

```rust
    /// 解压(单包或批量;7z 系组内按预扫大小加权出增量, unzip 退化为组级计数)
    Extracting,
```

- [ ] **Step 4.10: 跑全量测试(既有解压用例:无 ticks/无 sizes → 字节恒 0,期望不变)**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 4.11: 提交**

```bash
git add crates/core/src/backend.rs crates/core/src/extract.rs crates/core/src/progress.rs
git commit -m "feat(core): 解压组内条目级进度(预扫清单加权)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 5: CLI 解压字节显示 + 全量验证

**Files:**
- Modify: `crates/cli/src/main.rs:179-184`(extract 回调)

- [ ] **Step 5.1: 解压回调显示字节(与压缩分支同一格式)**

```rust
                &|p: &Progress| {
                    if let Phase::Extracting = p.phase {
                        let size = if p.total_bytes > 0 {
                            format!(
                                " ({}/{})",
                                format_bytes(p.done_bytes),
                                format_bytes(p.total_bytes)
                            )
                        } else {
                            String::new()
                        };
                        eprint!("\r[extract {}/{}] {}{}        ", p.done, p.total, p.entry, size);
                    }
                    ControlFlow::Continue(())
                },
```

(压缩分支的 `Phase::Compressing` 已有同样的字节显示,不动。)

- [ ] **Step 5.2: 全量验证**

Run:
```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd crates/tauri-app && npm test
```
Expected: 全部通过。Tauri 壳层与 UI 零改动(`ProgressEvent` 已带 doneBytes/totalBytes,app.js 已无条件渲染)。

- [ ] **Step 5.3: 提交**

```bash
git add crates/cli/src/main.rs
git commit -m "feat(cli): 解压进度显示字节(与压缩同格式)

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 6: e2e 回归 + 手动验证 + 文档交叉引用

**Files:**
- Modify: `test/tests/cli_e2e.rs`(文件末尾加 1 用例)
- Modify: `_docs/compress.md` / `_docs/extract.md`(各加一行交叉引用)

- [ ] **Step 6.1: 写 e2e 用例(复用文件头既有 helpers:`fixture`/`fill`/`run_tool`,签名以文件内定义为准)**

```rust
#[test]
fn compress_emits_progress_stream_frames() {
    // 两个 64MB 不可压条目: 7z 压缩各约 1s+, 进度帧约 10Hz → 几十帧;
    // 无进度流时整批只有个位数 \r(每条目 scan+compress 各一帧)。阈值 12 留足余量。
    let fx = fixture("progress");
    fill(&fx.root.join("in/Novel/book.txt"), 64 * 1024 * 1024, 7);
    fill(&fx.root.join("in/media/clip.bin"), 64 * 1024 * 1024, 8);
    let (inp, out) = (fx.root.join("in"), fx.root.join("out"));
    let r = run_tool(&["compress", "-i", inp.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert!(r.ok, "压缩失败:\n{}", r.stderr);
    let frames = r.stderr.matches('\r').count();
    assert!(
        frames >= 12,
        "进度帧过少({frames}), 进度流可能未生效:\n{}",
        r.stderr
    );
}
```

(`run_tool` 直接返回 `Run`,不走 `io::Result`,与既有用例一致;`fill(&Path, usize, seed: u8)`。)

- [ ] **Step 6.2: 跑 e2e**

Run: `cargo test -p zip-tool-e2e --target-dir target/e2e compress_emits_progress_stream_frames -- --nocapture`
Expected: PASS(本机 NanaZip;观察 stderr 帧里有 `NN%` 语义的连续更新)

- [ ] **Step 6.3: 跑全量 e2e 确认无回归**

Run: `cargo test -p zip-tool-e2e --target-dir target/e2e`
Expected: PASS(解压 stderr 新增 ` (x/y)` 字节段,若有用例对解压 stderr 做精确断言而失败,把断言放宽为 contains 后重跑)

- [ ] **Step 6.4: 手动验证 Tauri UI(压缩 + 解压各一次)**

Run: `cd crates/tauri-app && npm run tauri dev`
手动检查:压缩 7z/zip 格式时进度条平滑走百分比、字节计数递增;解压 .zip(Windows 走 7z 系)组内字节递增;取消按钮在条目内仍然即时生效。

- [ ] **Step 6.5: 文档交叉引用**

先 `grep -n "进度" _docs/compress.md _docs/extract.md` 定位各自的进度小节,在最近的进度说明段落后各加一行:

```markdown
进度流协议与实测结论见 [progress.md](progress.md)(7z 百分比流 + 解压条目信号加权)。
```

- [ ] **Step 6.6: 提交**

```bash
git add test/tests/cli_e2e.rs _docs/compress.md _docs/extract.md
git commit -m "test(e2e): 进度流回归用例 + 文档交叉引用

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## 验收清单(全部完成后)

- [ ] `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings` 干净
- [ ] `cargo test --workspace` 全绿
- [ ] `cargo test -p zip-tool-e2e --target-dir target/e2e` 全绿(含新进度用例)
- [ ] `cd crates/tauri-app && npm test` 全绿
- [ ] 手动:7z 压缩百分比平滑、解压字节递增、取消即时
