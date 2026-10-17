//! 批量压缩编排:input 下每个顶层条目(目录或文件)各压缩为一个包。
//!
//! - 进度:每完成一个条目回调一次 `Progress`(done 从 1 递增,total 为条目总数)
//! - 取消:回调返回 `ControlFlow::Break` 后不再开始下一个条目,已完成的保留
//! - 随机包名:`zt_<16hex>.zip`,同时在 out_dir 写 `mapping.csv` 记录对应关系

use std::fs;
use std::io::Write;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use crate::backend::{Backend, CommandBackend};
use crate::naming::random_name;
use crate::progress::{Phase, Progress};
use crate::scan::{Entry, scan_dir};
use crate::{CoreError, Options, Result};

/// 一个完成的条目:压缩包路径与原始名字(随机命名时二者不同)
#[derive(Debug, Clone)]
pub struct CompressedEntry {
    /// 压缩包主文件(.zip;分卷时 zip -s 为末卷 .zip,7z -v 为首卷 .zip.001)
    pub archive: PathBuf,
    /// 原始条目名
    pub original: String,
}

/// 批量压缩结果
#[derive(Debug, Default)]
pub struct Report {
    pub entries: Vec<CompressedEntry>,
    /// 回调要求取消而提前结束时为 true
    pub cancelled: bool,
    /// 扫描阶段的非致命问题(如无权限子树)
    pub warnings: Vec<String>,
}

/// 批量压缩:把 input 目录下的每个顶层条目各压缩为一个包,输出到 out_dir。
pub fn compress_dir(
    input: &Path,
    out_dir: &Path,
    opts: &Options,
    on_progress: &dyn Fn(&Progress) -> ControlFlow<()>,
) -> Result<Report> {
    compress_with_backend(&Backend::native(), input, out_dir, opts, on_progress)
}

pub(crate) fn compress_with_backend<B: CommandBackend + ?Sized>(
    backend: &B,
    input: &Path,
    out_dir: &Path,
    opts: &Options,
    on_progress: &dyn Fn(&Progress) -> ControlFlow<()>,
) -> Result<Report> {
    if !input.is_dir() {
        return Err(CoreError::InputNotFound(input.to_path_buf()));
    }

    // 输出目录不得是输入目录自身或其子目录(边压边把半成品扫进包里, 产物不可预期)
    let input_abs = absolutize(input);
    let out_abs = absolutize(out_dir);
    if path_within(&out_abs, &input_abs) {
        return Err(CoreError::OutInsideInput(out_dir.to_path_buf()));
    }
    backend.check_split_size(opts.split_size)?;
    fs::create_dir_all(&out_abs)?;

    // Step 1: 扫描(进度 Phase::Scanning;扫描阶段不支持取消, 回调返回值在此忽略)
    let scan = scan_dir(input, &|p| {
        let _ = on_progress(p);
    })?;
    if scan.entries.is_empty() {
        return Err(CoreError::NoEntries(input.to_path_buf()));
    }

    // Step 2: 生成 dst + 预检, 全部在任何压缩开始之前
    let mut entries = scan.entries;
    assign_dsts(&mut entries, &out_abs, opts.random_name)?;

    // Step 3: 逐条目压缩(顺序执行; 随机命名时 mapping.csv 逐条目追加, 中断不丢)
    if opts.random_name {
        mapping_create(&out_abs)?;
    }
    let total = entries.len();
    let mut report = Report {
        entries: Vec::new(),
        cancelled: false,
        warnings: scan.warnings,
    };
    let mut done_bytes = 0u64;

    for (i, e) in entries.iter().enumerate() {
        let out_zip = out_abs.join(format!("{}.zip", e.dst));
        let result = backend
            .compress_command(&input_abs, &e.name, &out_zip, opts)
            .and_then(|mut cmd| backend.run(&mut cmd));
        if let Err(err) = result {
            // 停批并清理半成品: 半成品分卷被误当完整包比少一个包更危险
            cleanup_partial(&out_abs, &e.dst);
            return Err(err);
        }

        let archive = resolve_primary_archive(&out_abs, &e.dst);
        if opts.random_name {
            let fname = archive
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            mapping_append(&out_abs, &fname, &e.name)?;
        }
        report.entries.push(CompressedEntry {
            archive,
            original: e.name.clone(),
        });
        done_bytes += e.size;

        if let ControlFlow::Break(()) = on_progress(&Progress {
            phase: Phase::Compressing,
            done: i + 1,
            total,
            done_bytes,
            total_bytes: scan.total_bytes,
            entry: e.name.clone(),
        }) {
            report.cancelled = true;
            break;
        }
    }

    Ok(report)
}

/// 随机命名时创建 mapping.csv 并写表头(Step 3 开始前调用)
fn mapping_create(out_dir: &Path) -> Result<()> {
    let mut f = fs::File::create(out_dir.join("mapping.csv"))?;
    f.write_all(b"archive,original\n")?;
    Ok(())
}

/// 每个条目完成后追加一行并落盘, 中断/断电时已完成部分的对应关系不丢
fn mapping_append(out_dir: &Path, archive: &str, original: &str) -> Result<()> {
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(out_dir.join("mapping.csv"))?;
    f.write_all(format!("{archive},{original}\n").as_bytes())?;
    f.sync_data()?;
    Ok(())
}

/// 删除某条目压缩失败后的半成品: <dst>.zip / <dst>.zNN(zip 分卷) / <dst>.zip.NNN(7z 分卷)
fn cleanup_partial(out_dir: &Path, dst: &str) {
    let Ok(rd) = fs::read_dir(out_dir) else {
        return;
    };
    let z_prefix = format!("{dst}.z");
    let vol_prefix = format!("{dst}.zip.");
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let is_ours = name == format!("{dst}.zip")
            || (name.starts_with(&z_prefix)
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

/// Step 2: 生成每个条目的压缩包基础名(不带扩展名)。
/// 随机名 = "zt_" + 64 bit 随机量(16 hex); 非随机 = 原条目名。
fn assign_dsts(entries: &mut [Entry], out_dir: &Path, random: bool) -> Result<()> {
    assign_dsts_with(entries, out_dir, random, &mut || {
        format!("zt_{}", random_name())
    })
}

/// 带可注入名字生成器的版本(测试用); next_name 返回完整的随机候选名。
fn assign_dsts_with(
    entries: &mut [Entry],
    out_dir: &Path,
    random: bool,
    next_name: &mut dyn FnMut() -> String,
) -> Result<()> {
    if !random {
        // 大小写不敏感查重: 源盘可能是大小写敏感的 ext4, 输出盘可能是 NTFS/FAT
        let mut seen: Vec<String> = Vec::new();
        let mut dup: Vec<String> = Vec::new();
        for e in entries.iter() {
            let lower = e.name.to_lowercase();
            if seen.iter().any(|s| *s == lower) && !dup.iter().any(|s| *s == lower) {
                dup.push(e.name.clone());
            }
            seen.push(lower);
        }
        if !dup.is_empty() {
            return Err(CoreError::DuplicateDst(dup));
        }
    }
    for e in entries.iter_mut() {
        e.dst = if random {
            // 撞上已有产物则重新生成(碰撞概率 ~10^-12, 重试只为兜底)
            loop {
                let candidate = next_name();
                if !target_exists(out_dir, &candidate) {
                    break candidate;
                }
            }
        } else {
            if target_exists(out_dir, &e.name) {
                return Err(CoreError::OutputExists(
                    out_dir.join(format!("{}.zip", e.name)),
                ));
            }
            e.name.clone()
        };
    }
    Ok(())
}

/// 该 dst 的压缩包(不分卷 .zip, 或 7z 分卷首卷 .zip.001)是否已存在
fn target_exists(out_dir: &Path, dst: &str) -> bool {
    out_dir.join(format!("{dst}.zip")).exists() || out_dir.join(format!("{dst}.zip.001")).exists()
}

/// 相对路径转为绝对路径(基于当前工作目录)。
/// 不用 canonicalize: Windows 上它产生 \\?\ verbatim 路径, 部分外部命令不认。
fn absolutize(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    }
}

/// child 是否等于 parent 或位于其下; 按组件比较(避免字符串前缀误判), Windows 下大小写不敏感
fn path_within(child: &Path, parent: &Path) -> bool {
    let fold = |s: &std::ffi::OsStr| -> String {
        let s = s.to_string_lossy().into_owned();
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    let pc: Vec<String> = parent.components().map(|c| fold(c.as_os_str())).collect();
    let cc: Vec<String> = child.components().map(|c| fold(c.as_os_str())).collect();
    cc.len() >= pc.len() && cc[..pc.len()] == pc[..]
}

/// 返回压缩包主文件:优先 base.zip(zip -s 的末卷 / 不分卷),
/// 否则是 7z -v 的首卷 base.zip.001。
fn resolve_primary_archive(out_dir: &Path, base: &str) -> PathBuf {
    let plain = out_dir.join(format!("{base}.zip"));
    if plain.exists() {
        return plain;
    }
    let first_volume = out_dir.join(format!("{base}.zip.001"));
    if first_volume.exists() {
        return first_volume;
    }
    plain
}

/// 随机命名时写 mapping.csv(archive 名,原始名),供解包端还原
#[cfg(test)]
mod dst_tests {
    use super::*;
    use crate::scan::{Entry, EntryKind};

    fn entries(names: &[&str]) -> Vec<Entry> {
        names
            .iter()
            .map(|n| Entry {
                name: n.to_string(),
                kind: if n.contains('.') {
                    EntryKind::File
                } else {
                    EntryKind::Dir
                },
                size: 1,
                dst: String::new(),
                format: "zip".into(),
            })
            .collect()
    }

    #[test]
    fn assign_dsts_keeps_original_names_without_random() {
        let mut es = entries(&["Novel", "C.jpg"]);
        assign_dsts(&mut es, Path::new("out"), false).unwrap();
        assert_eq!(es[0].dst, "Novel");
        assert_eq!(es[1].dst, "C.jpg");
    }

    #[test]
    fn assign_dsts_generates_zt_prefixed_64bit_random_names() {
        let mut es = entries(&["A", "B"]);
        assign_dsts(&mut es, Path::new("out"), true).unwrap();
        for e in &es {
            assert!(e.dst.starts_with("zt_"), "随机名应有 zt_ 前缀: {e:?}");
            let rand = &e.dst[3..];
            assert_eq!(rand.len(), 16, "随机部分应为 64 bit = 16 个 hex 字符");
            assert!(rand.chars().all(|c| c.is_ascii_hexdigit()));
        }
        assert_ne!(es[0].dst, es[1].dst);
    }

    #[test]
    fn assign_dsts_rejects_case_insensitive_duplicate_names() {
        // 源盘可能是大小写敏感的 ext4, 输出盘可能是 NTFS; 在源头报错优于压到一半失败
        let mut es = entries(&["Novel", "novel"]);
        let err = assign_dsts(&mut es, Path::new("out"), false).unwrap_err();
        assert!(matches!(err, CoreError::DuplicateDst(_)));
    }

    #[test]
    fn assign_dsts_rejects_when_target_archive_already_exists() {
        // 重复运行不覆盖: zip 对已有包"更新"会产生脏包, 比失败更危险
        let root = crate::testutil::unique_temp("dst-exists");
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("out/A.zip"), b"old").unwrap();

        let mut es = entries(&["A"]);
        let err = assign_dsts(&mut es, &root.join("out"), false).unwrap_err();
        assert!(matches!(err, CoreError::OutputExists(_)));

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn assign_dsts_random_mode_regenerates_on_existing_target() {
        // 随机名与已有产物撞名时重新生成, 而不是报错(注入可控的候选序列)
        use crate::naming::random_name;
        let root = crate::testutil::unique_temp("dst-retry");
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("out/zt_aaaaaaaaaaaaaaaa.zip"), b"old").unwrap();

        let mut es = entries(&["A"]);
        // 生产路径的闭包无法定向撞名, 这里直接测带生成器的内部版本
        let mut candidates = ["aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"].iter();
        let err_free = assign_dsts_with(&mut es, &root.join("out"), true, &mut || {
            candidates
                .next()
                .map(|c| format!("zt_{c}"))
                .unwrap_or_else(random_name)
        });
        err_free.unwrap();
        assert_eq!(es[0].dst, "zt_bbbbbbbbbbbbbbbb", "撞名候选应被跳过");

        std::fs::remove_dir_all(&root).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Options;

    fn opts(split: Option<u64>) -> Options {
        Options {
            split_size: split,
            password: None,
            random_name: false,
        }
    }

    fn noop(_: &Progress) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }

    #[test]
    fn rejects_output_directory_inside_input() {
        // 边压边把半成品扫进包里, 产物不可预期, 必须在压缩开始前报错
        let root = crate::testutil::unique_temp("out-inside");
        fs::create_dir_all(root.join("in/data")).unwrap();
        fs::write(root.join("in/data/f.txt"), "v").unwrap();

        let err = compress_with_backend(
            &Backend::Zip,
            &root.join("in"),
            &root.join("in/out"),
            &opts(None),
            &noop,
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::OutInsideInput(_)));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn zip_backend_rejects_split_below_64k() {
        let root = crate::testutil::unique_temp("split-64k");
        fs::create_dir_all(root.join("in/x")).unwrap();
        fs::write(root.join("in/x/f.txt"), "v").unwrap();

        let err = compress_with_backend(
            &Backend::Zip,
            &root.join("in"),
            &root.join("out"),
            &opts(Some(1024)),
            &noop,
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::SplitSizeTooSmall(1024)));
        fs::remove_dir_all(&root).unwrap();
    }
}

/// 编排测试:注入假后端, 断言 Step1→Step2→Step3 的串联行为
#[cfg(test)]
mod fake_tests {
    use super::*;
    use crate::backend::CommandBackend;
    use crate::progress::Phase;
    use crate::testutil::unique_temp;
    use std::cell::RefCell;
    use std::process::Command;

    /// 假后端:成功时写出 {dst}.zip;fail_on 中的条目写出半成品分卷后报错
    struct FakeBackend {
        fail_on: Vec<String>,
        attempted: RefCell<Vec<String>>,
    }

    impl FakeBackend {
        fn new(fail_on: &[&str]) -> Self {
            Self {
                fail_on: fail_on.iter().map(|s| s.to_string()).collect(),
                attempted: RefCell::new(Vec::new()),
            }
        }
    }

    impl CommandBackend for FakeBackend {
        fn compress_command(
            &self,
            _work: &Path,
            name: &str,
            out_zip: &Path,
            _opts: &Options,
        ) -> Result<Command> {
            self.attempted.borrow_mut().push(name.to_string());
            let stem = out_zip.file_stem().unwrap().to_string_lossy().into_owned();
            let dir = out_zip.parent().unwrap().to_path_buf();
            if self.fail_on.iter().any(|f| *f == name) {
                // 模拟分卷写一半崩溃:留下 .z01/.z02
                fs::write(dir.join(format!("{stem}.z01")), b"part").unwrap();
                fs::write(dir.join(format!("{stem}.z02")), b"part").unwrap();
                return Err(CoreError::BackendFailed {
                    program: "fake".into(),
                    status: "exit status: 1".into(),
                    output: "boom".into(),
                });
            }
            fs::write(dir.join(format!("{stem}.zip")), b"fake").unwrap();
            Ok(Command::new("fake-unused"))
        }

        fn run(&self, _cmd: &mut Command) -> Result<()> {
            Ok(())
        }

        fn check_split_size(&self, _size: Option<u64>) -> Result<()> {
            Ok(())
        }
    }

    /// in/A/(1+2=3 字节) 与 in/B/(4 字节), 总计 7 字节
    fn two_entries_fixture(tag: &str) -> PathBuf {
        let root = unique_temp(tag);
        fs::create_dir_all(root.join("in/A")).unwrap();
        fs::create_dir_all(root.join("in/B")).unwrap();
        fs::write(root.join("in/A/a.txt"), "a").unwrap();
        fs::write(root.join("in/A/b.txt"), "bb").unwrap();
        fs::write(root.join("in/B/c.txt"), "cccc").unwrap();
        root
    }

    fn rnd() -> Options {
        Options {
            split_size: None,
            password: None,
            random_name: true,
        }
    }

    #[test]
    fn orchestration_scans_then_compresses_with_weighted_progress_and_mapping() {
        let root = two_entries_fixture("orch");
        let events = RefCell::new(Vec::new());
        let report = compress_with_backend(
            &FakeBackend::new(&[]),
            &root.join("in"),
            &root.join("out"),
            &rnd(),
            &|p| {
                events.borrow_mut().push(p.clone());
                ControlFlow::Continue(())
            },
        )
        .unwrap();

        // 进度:先 2 条 Scanning, 后 2 条 Compressing(字节加权)
        let ev = events.borrow();
        assert_eq!(ev.len(), 4);
        assert_eq!(ev[0].phase, Phase::Scanning);
        assert_eq!((ev[0].done, ev[0].total), (1, 2));
        assert_eq!(ev[1].phase, Phase::Scanning);
        assert_eq!(ev[2].phase, Phase::Compressing);
        assert_eq!(
            (ev[2].done, ev[2].done_bytes, ev[2].total_bytes),
            (1, 3, 7),
            "A 完成后应为 3/7 字节"
        );
        assert_eq!((ev[3].done, ev[3].done_bytes, ev[3].total_bytes), (2, 7, 7));

        // mapping.csv:表头 + 按完成顺序逐条追加
        let csv = fs::read_to_string(root.join("out/mapping.csv")).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "archive,original");
        assert_eq!(lines.len(), 3);
        assert!(lines[1].ends_with(",A"), "{csv}");
        assert!(lines[2].ends_with(",B"), "{csv}");
        // 行内 archive 名对应真实产物
        let arc1 = lines[1].split(',').next().unwrap();
        assert!(root.join("out").join(arc1).exists(), "{arc1}");

        assert!(report.warnings.is_empty());
        assert_eq!(report.entries.len(), 2);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn backend_failure_stops_batch_and_cleans_partial_artifacts() {
        let root = two_entries_fixture("fail");
        let err = compress_with_backend(
            &FakeBackend::new(&["B"]),
            &root.join("in"),
            &root.join("out"),
            &rnd(),
            &|_| ControlFlow::Continue(()),
        );
        assert!(err.is_err(), "后端失败应上抛");

        let names: Vec<String> = fs::read_dir(root.join("out"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().all(|n| !n.starts_with("B.")),
            "B 的半成品应被清理: {names:?}"
        );
        assert_eq!(
            names
                .iter()
                .filter(|n| n.starts_with("zt_") && n.ends_with(".zip"))
                .count(),
            1,
            "A 的产物保留: {names:?}"
        );
        let csv = fs::read_to_string(root.join("out/mapping.csv")).unwrap();
        assert_eq!(csv.lines().count(), 2, "失败的 B 不应进 mapping: {csv}");
        assert!(csv.ends_with(",A\n"), "{csv}");

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cancel_break_stops_before_next_entry() {
        let root = two_entries_fixture("cancel");
        let fb = FakeBackend::new(&[]);
        let report =
            compress_with_backend(&fb, &root.join("in"), &root.join("out"), &rnd(), &|p| {
                if p.phase == Phase::Compressing && p.done >= 1 {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            })
            .unwrap();

        assert!(report.cancelled);
        assert_eq!(*fb.attempted.borrow(), ["A"], "取消后 B 不应开始");
        assert_eq!(report.entries.len(), 1);
        fs::remove_dir_all(&root).unwrap();
    }
}
