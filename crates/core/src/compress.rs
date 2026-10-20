//! 批量压缩编排:input 下每个顶层条目(目录或文件)各压缩为一个包。
//!
//! - 进度:每完成一个条目回调一次 `Progress`(done 从 1 递增,total 为条目总数)
//! - 取消两个生效点:条目间由回调 `ControlFlow::Break`(已完成的保留);
//!   条目内由 `CancelFlag` 置位 kill 后端进程, 当前条目作废并清理半成品
//! - 随机包名:`zt_<16hex>.{zip|7z}`,同时在 out_dir 写 `mapping.csv` 记录对应关系

use std::fs;
use std::io::Write;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use crate::CancelFlag;
use crate::backend::CommandBackend;
use crate::naming::random_name;
use crate::progress::{Phase, Progress};
use crate::scan::{Entry, scan_dir};
use crate::{CoreError, Options, Result};

/// 一个完成的条目:压缩包路径与原始名字(随机命名时二者不同)
#[derive(Debug, Clone)]
pub struct CompressedEntry {
    /// 压缩包主文件({dst}.{ext};分卷时 zip -s 为末卷,7z -v 为首卷 .001)
    pub archive: PathBuf,
    /// 原始条目名
    pub original: String,
}

/// 批量压缩结果
#[derive(Debug, Default)]
pub struct Report {
    pub entries: Vec<CompressedEntry>,
    /// 取消而提前结束时为 true(条目间 Break, 或条目内 kill 后端进程)
    pub cancelled: bool,
    /// 扫描阶段的非致命问题(如无权限子树)
    pub warnings: Vec<String>,
    /// 被排除的顶层条目(仅 compress_dir 单调用路径丢弃时带出;分阶段路径为空)
    pub excluded: Vec<crate::exclude::ExcludedEntry>,
}

/// 批量压缩:把 input 目录下的每个顶层条目各压缩为一个包,输出到 out_dir。
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

pub(crate) fn compress_with_backend<B: CommandBackend + ?Sized>(
    backend: &B,
    input: &Path,
    out_dir: &Path,
    opts: &Options,
    cancel: &CancelFlag,
    on_progress: &dyn Fn(&Progress) -> ControlFlow<()>,
) -> Result<Report> {
    if !input.is_dir() {
        return Err(CoreError::InputNotFound(input.to_path_buf()));
    }
    // 保持现状: 分包下限在扫描前快速失败
    backend.check_split_size(opts.split_size)?;

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
}

/// Step 2(独立阶段, _docs/ui-backend.md 6): 输出目录校验 + 生成 dst + 全部预检,
/// 在任何压缩开始之前。UI 分阶段入口: 外壳缓存扫描结果, 确认时对筛选后的条目
/// 先调本函数、再调 [`compress_entries`], 不重复扫描。
/// 筛选后为空 → `NoEntries`(条目存在性复核在 compress_entries)。
pub fn plan(input: &Path, out_dir: &Path, entries: &mut [Entry], opts: &Options) -> Result<()> {
    if !input.is_dir() {
        return Err(CoreError::InputNotFound(input.to_path_buf()));
    }
    // 输出目录不得是输入目录自身或其子目录(边压边把半成品扫进包里, 产物不可预期)
    let input_abs = absolutize(input);
    let out_abs = absolutize(out_dir);
    if path_within(&out_abs, &input_abs) {
        return Err(CoreError::OutInsideInput(out_dir.to_path_buf()));
    }
    fs::create_dir_all(&out_abs)?;
    if entries.is_empty() {
        return Err(CoreError::NoEntries(input.to_path_buf()));
    }

    // 生成 dst + 预检(DuplicateDst / OutputExists), 全部在任何压缩开始之前
    assign_dsts(entries, &out_abs, opts.random_name, opts.format.extension())
}

/// Step 3(独立阶段): 逐条目压缩, 开始前对每个条目做顶层存在性复核 ——
/// 扫描到确认之间条目可能被删, 缺失即整批不开始(宁停不错, `EntryMissing`)。
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

pub(crate) fn compress_entries_with_backend<B: CommandBackend + ?Sized>(
    backend: &B,
    input: &Path,
    out_dir: &Path,
    entries: &[Entry],
    opts: &Options,
    cancel: &CancelFlag,
    on_progress: &dyn Fn(&Progress) -> ControlFlow<()>,
) -> Result<Report> {
    backend.check_split_size(opts.split_size)?;
    let input_abs = absolutize(input);
    let out_abs = absolutize(out_dir);
    for e in entries {
        if fs::symlink_metadata(input_abs.join(&e.name)).is_err() {
            return Err(CoreError::EntryMissing {
                name: e.name.clone(),
            });
        }
    }

    // 逐条目压缩(顺序执行; 随机命名时 mapping.csv 逐条目追加, 中断不丢)
    if opts.random_name {
        mapping_create(&out_abs)?;
    }
    let total = entries.len();
    let total_bytes: u64 = entries.iter().map(|e| e.size).sum();
    let ext = opts.format.extension();
    let mut report = Report::default();
    let mut done_bytes = 0u64;

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
        let archive = match result {
            Ok(archive) => archive,
            Err(err) => {
                // 停批并清理半成品: 半成品分卷被误当完整包比少一个包更危险
                cleanup_partial(&out_abs, &e.dst, ext);
                if matches!(err, CoreError::Cancelled) {
                    // 进程内取消: 当前条目作废(不计进度/不进 mapping), 已完成的保留
                    report.cancelled = true;
                    break;
                }
                return Err(err);
            }
        };

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
            total_bytes,
            entry: e.name.clone(),
        }) {
            report.cancelled = true;
            break;
        }
    }

    Ok(report)
}

/// 随机命名时创建 mapping.csv 并写表头(Step 3 开始前调用)
pub(crate) fn mapping_create(out_dir: &Path) -> Result<()> {
    let mut f = fs::File::create(out_dir.join("mapping.csv"))?;
    f.write_all(b"archive,original\n")?;
    Ok(())
}

/// 每个条目完成后追加一行并落盘, 中断/断电时已完成部分的对应关系不丢
pub(crate) fn mapping_append(out_dir: &Path, archive: &str, original: &str) -> Result<()> {
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(out_dir.join("mapping.csv"))?;
    f.write_all(format!("{archive},{original}\n").as_bytes())?;
    f.sync_data()?;
    Ok(())
}

/// 删除某条目压缩失败后的半成品: 主文件 {dst}.{ext}、zip -s 的 {dst}.zNN、分卷 {dst}.{ext}.NNN
pub(crate) fn cleanup_partial(out_dir: &Path, dst: &str, ext: &str) {
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

/// Step 2: 生成每个条目的压缩包基础名(不带扩展名)并填充格式。
/// 随机名 = "zt_" + 64 bit 随机量(16 hex); 非随机 = 原条目名。
fn assign_dsts(entries: &mut [Entry], out_dir: &Path, random: bool, ext: &str) -> Result<()> {
    assign_dsts_with(entries, out_dir, random, ext, &mut || {
        format!("zt_{}", random_name())
    })
}

/// 带可注入名字生成器的版本(测试用); next_name 返回完整的随机候选名。
fn assign_dsts_with(
    entries: &mut [Entry],
    out_dir: &Path,
    random: bool,
    ext: &str,
    next_name: &mut dyn FnMut() -> String,
) -> Result<()> {
    if !random {
        // 大小写不敏感查重: 源盘可能是大小写敏感的 ext4, 输出盘可能是 NTFS/FAT
        let mut seen: Vec<String> = Vec::new();
        let mut dup: Vec<String> = Vec::new();
        for e in entries.iter() {
            let lower = e.name.to_lowercase();
            if seen.contains(&lower) && !dup.contains(&lower) {
                dup.push(e.name.clone());
            }
            seen.push(lower);
        }
        if !dup.is_empty() {
            return Err(CoreError::DuplicateDst(dup));
        }
    }
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
                    out_dir.join(format!("{}.{}", e.name, ext)),
                ));
            }
            e.name.clone()
        };
    }
    Ok(())
}

/// 该 dst 的压缩包(不分卷主文件, 或分卷首卷 {dst}.{ext}.001)是否已存在
pub(crate) fn target_exists(out_dir: &Path, dst: &str, ext: &str) -> bool {
    out_dir.join(format!("{dst}.{ext}")).exists()
        || out_dir.join(format!("{dst}.{ext}.001")).exists()
}

/// 相对路径转为绝对路径(基于当前工作目录)。
/// 不用 canonicalize: Windows 上它产生 \\?\ verbatim 路径, 部分外部命令不认。
pub(crate) fn absolutize(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    }
}

/// child 是否等于 parent 或位于其下; 按组件比较(避免字符串前缀误判), Windows 下大小写不敏感
pub(crate) fn path_within(child: &Path, parent: &Path) -> bool {
    let fold = |s: &std::ffi::OsStr| -> String {
        let s = s.to_string_lossy().into_owned();
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    let pc: Vec<String> = parent.components().map(|c| fold(c.as_os_str())).collect();
    let cc: Vec<String> = child.components().map(|c| fold(c.as_os_str())).collect();
    cc.len() >= pc.len() && cc[..pc.len()] == pc[..]
}

/// 返回压缩包主文件:优先 {base}.{ext}(zip -s 的末卷 / 不分卷),
/// 否则分卷首卷 {base}.{ext}.001。
pub(crate) fn resolve_primary_archive(out_dir: &Path, base: &str, ext: &str) -> PathBuf {
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
                entry_count: 1,
                dst: String::new(),
                format: String::new(),
                exclude: None,
                attr_hidden: Vec::new(),
            })
            .collect()
    }

    #[test]
    fn assign_dsts_keeps_original_names_without_random() {
        let mut es = entries(&["Novel", "C.jpg"]);
        assign_dsts(&mut es, Path::new("out"), false, "zip").unwrap();
        assert_eq!(es[0].dst, "Novel");
        assert_eq!(es[1].dst, "C.jpg");
    }

    #[test]
    fn assign_dsts_generates_zt_prefixed_64bit_random_names() {
        let mut es = entries(&["A", "B"]);
        assign_dsts(&mut es, Path::new("out"), true, "zip").unwrap();
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
        let err = assign_dsts(&mut es, Path::new("out"), false, "zip").unwrap_err();
        assert!(matches!(err, CoreError::DuplicateDst(_)));
    }

    #[test]
    fn assign_dsts_rejects_when_target_archive_already_exists() {
        // 重复运行不覆盖: zip 对已有包"更新"会产生脏包, 比失败更危险
        let root = crate::testutil::unique_temp("dst-exists");
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("out/A.zip"), b"old").unwrap();

        let mut es = entries(&["A"]);
        let err = assign_dsts(&mut es, &root.join("out"), false, "zip").unwrap_err();
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
        let err_free = assign_dsts_with(&mut es, &root.join("out"), true, "zip", &mut || {
            candidates
                .next()
                .map(|c| format!("zt_{c}"))
                .unwrap_or_else(random_name)
        });
        err_free.unwrap();
        assert_eq!(es[0].dst, "zt_bbbbbbbbbbbbbbbb", "撞名候选应被跳过");

        std::fs::remove_dir_all(&root).unwrap();
    }

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Options;
    use crate::backend::Backend;

    fn opts(split: Option<u64>) -> Options {
        Options {
            split_size: split,
            password: None,
            random_name: false,
            ..Options::default()
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
            &CancelFlag::new(),
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
            &CancelFlag::new(),
            &noop,
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::SplitSizeTooSmall(1024)));
        fs::remove_dir_all(&root).unwrap();
    }

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
        assert!(
            root.join("A.z01").exists(),
            ".zNN 是 zip -s 专属, 7z 清理不碰"
        );

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
}

/// 编排测试:注入假后端, 断言 Step1→Step2→Step3 的串联行为
#[cfg(test)]
mod fake_tests {
    use super::*;
    use crate::ExcludeReason;
    use crate::backend::CommandBackend;
    use crate::progress::{Phase, Progress};
    use crate::testutil::unique_temp;
    use std::cell::RefCell;
    use std::process::Command;

    /// 假后端:成功时写出 {dst}.zip;fail_on 中的条目写出半成品分卷后报错
    struct FakeBackend {
        fail_on: Vec<String>,
        verify_mode: VerifyMode,
        attempted: RefCell<Vec<String>>,
        verify_calls: RefCell<usize>,
    }

    /// 结构校验的模拟结果
    enum VerifyMode {
        Pass,
        Fail,
        /// 第 n 次校验返回 Cancelled(模拟校验期间取消)
        CancelAt(usize),
    }

    impl FakeBackend {
        fn new(fail_on: &[&str]) -> Self {
            Self {
                fail_on: fail_on.iter().map(|s| s.to_string()).collect(),
                verify_mode: VerifyMode::Pass,
                attempted: RefCell::new(Vec::new()),
                verify_calls: RefCell::new(0),
            }
        }

        fn verify_failing() -> Self {
            Self {
                verify_mode: VerifyMode::Fail,
                ..Self::new(&[])
            }
        }

        fn verify_cancelling_at(call: usize) -> Self {
            Self {
                verify_mode: VerifyMode::CancelAt(call),
                ..Self::new(&[])
            }
        }
    }

    impl CommandBackend for FakeBackend {
        fn compress_command(
            &self,
            _work: &Path,
            name: &str,
            out_archive: &Path,
            _opts: &Options,
            _attr_hidden: &[String],
        ) -> Result<Command> {
            self.attempted.borrow_mut().push(name.to_string());
            let stem = out_archive
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned();
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

        fn run(&self, _cmd: &mut Command, _cancel: &crate::CancelFlag) -> Result<()> {
            Ok(())
        }

        fn verify(&self, entry: &Path, _opts: &Options, _cancel: &crate::CancelFlag) -> Result<()> {
            let n = {
                let mut calls = self.verify_calls.borrow_mut();
                *calls += 1;
                *calls
            };
            match self.verify_mode {
                VerifyMode::Pass => Ok(()),
                VerifyMode::Fail => Err(CoreError::VerifyFailed {
                    archive: entry.to_path_buf(),
                    status: "exit code: 2".into(),
                    output: "Unexpected end of archive".into(),
                }),
                VerifyMode::CancelAt(k) if n == k => Err(CoreError::Cancelled),
                VerifyMode::CancelAt(_) => Ok(()),
            }
        }

        fn check_split_size(&self, _size: Option<u64>) -> Result<()> {
            Ok(())
        }
    }

    /// run 时发 50%、100% 两帧的假后端:验证百分比 → 字节加权映射(委托其余行为给 FakeBackend)
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
            self.0
                .compress_command(work, name, out_archive, opts, attr_hidden)
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
            on_percent(100);
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
                entry_count: 1,
                dst: "A".into(),
                format: String::new(),
                exclude: None,
                attr_hidden: Vec::new(),
            },
            Entry {
                name: "B".into(),
                kind: crate::scan::EntryKind::Dir,
                size: 4,
                entry_count: 1,
                dst: "B".into(),
                format: String::new(),
                exclude: None,
                attr_hidden: Vec::new(),
            },
        ];
        let events = RefCell::new(Vec::new());
        // 直调 compress_entries_with_backend(不经 plan), mapping.csv 需要输出目录已存在
        fs::create_dir_all(root.join("out")).unwrap();
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
        let compressing: Vec<_> = ev
            .iter()
            .filter(|p| p.phase == Phase::Compressing)
            .collect();
        // A 50% → 3*50/100=1;A 100% → 3(== 完成值);A 完成 → 3;
        // B 50% → 3+4*50/100=5;B 100% → 7(== 完成值);B 完成 → 7
        // 同条目多帧每次都重算自 base, 不得用增量累加
        assert_eq!(
            compressing
                .iter()
                .map(|p| (p.done, p.done_bytes, p.total_bytes))
                .collect::<Vec<_>>(),
            [
                (0, 1, 7),
                (0, 3, 7),
                (1, 3, 7),
                (1, 5, 7),
                (1, 7, 7),
                (2, 7, 7)
            ],
            "{ev:?}"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn zero_size_entry_emits_no_tick_frame() {
        // size=0 条目: tick 被 entry_size > 0 guard 跳过, 只留完成帧, 完成字节正常累计
        let root = two_entries_fixture("zero-size");
        let entries = [
            Entry {
                name: "A".into(),
                kind: crate::scan::EntryKind::Dir,
                size: 0,
                entry_count: 1,
                dst: "A".into(),
                format: String::new(),
                exclude: None,
                attr_hidden: Vec::new(),
            },
            Entry {
                name: "B".into(),
                kind: crate::scan::EntryKind::Dir,
                size: 4,
                entry_count: 1,
                dst: "B".into(),
                format: String::new(),
                exclude: None,
                attr_hidden: Vec::new(),
            },
        ];
        let events = RefCell::new(Vec::new());
        // 直调 compress_entries_with_backend(不经 plan), mapping.csv 需要输出目录已存在
        fs::create_dir_all(root.join("out")).unwrap();
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
        let compressing: Vec<_> = ev
            .iter()
            .filter(|p| p.phase == Phase::Compressing)
            .collect();
        // A(0 字节): 50%/100% 两帧 tick 均被跳过 → 只有完成帧;
        // B: 50% → 0+4*50/100=2, 100% → 4(== 完成值), 完成 → 4
        assert_eq!(
            compressing
                .iter()
                .map(|p| (p.done, p.done_bytes, p.total_bytes))
                .collect::<Vec<_>>(),
            [(1, 0, 4), (1, 2, 4), (1, 4, 4), (2, 4, 4)],
            "{ev:?}"
        );
        fs::remove_dir_all(&root).unwrap();
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
            ..Options::default()
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
            &CancelFlag::new(),
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
            &CancelFlag::new(),
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
    fn verify_disabled_skips_structure_check() {
        let root = two_entries_fixture("verify-off");
        let be = FakeBackend::verify_failing();
        let mut o = rnd();
        o.verify = false;
        let report = compress_with_backend(
            &be,
            &root.join("in"),
            &root.join("out"),
            &o,
            &CancelFlag::new(),
            &|_| ControlFlow::Continue(()),
        )
        .unwrap();

        assert_eq!(report.entries.len(), 2, "关闭校验后两个条目都应完成");
        assert_eq!(*be.verify_calls.borrow(), 0, "关闭后不应调用校验");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn verify_failure_stops_batch_cleans_artifact_and_skips_report_mapping() {
        let root = two_entries_fixture("verify-fail");
        let be = FakeBackend::verify_failing();
        let events: RefCell<Vec<Progress>> = RefCell::new(Vec::new());
        let err = compress_with_backend(
            &be,
            &root.join("in"),
            &root.join("out"),
            &rnd(),
            &CancelFlag::new(),
            &|p| {
                events.borrow_mut().push(p.clone());
                ControlFlow::Continue(())
            },
        )
        .unwrap_err();

        match err {
            CoreError::VerifyFailed { archive, .. } => {
                assert!(archive.to_string_lossy().ends_with(".zip"), "{archive:?}");
            }
            other => panic!("应为 VerifyFailed, 实际: {other:?}"),
        }
        assert_eq!(*be.attempted.borrow(), ["A"], "校验失败后 B 不应开始");
        assert_eq!(*be.verify_calls.borrow(), 1);

        let names: Vec<String> = fs::read_dir(root.join("out"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().all(|n| !n.ends_with(".zip")),
            "失败条目的产物应被清理: {names:?}"
        );
        let csv = fs::read_to_string(root.join("out/mapping.csv")).unwrap();
        assert_eq!(csv, "archive,original\n", "失败条目不应进 mapping: {csv}");
        assert!(
            events
                .borrow()
                .iter()
                .all(|p| p.phase != Phase::Compressing),
            "校验失败不应上报完成进度: {:?}",
            events.borrow()
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cancel_during_verify_discards_current_entry_and_stops_batch() {
        let root = two_entries_fixture("verify-cancel");
        let be = FakeBackend::verify_cancelling_at(2);
        let report = compress_with_backend(
            &be,
            &root.join("in"),
            &root.join("out"),
            &rnd(),
            &CancelFlag::new(),
            &|_| ControlFlow::Continue(()),
        )
        .unwrap();

        assert!(report.cancelled);
        assert_eq!(report.entries.len(), 1, "A 保留, 校验期取消的 B 不应计入");
        assert_eq!(*be.attempted.borrow(), ["A", "B"], "取消后不应有更多条目");
        assert_eq!(*be.verify_calls.borrow(), 2);

        let names: Vec<String> = fs::read_dir(root.join("out"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names.iter().filter(|n| n.ends_with(".zip")).count(),
            1,
            "B 的产物应被清理, 只保留 A: {names:?}"
        );
        let csv = fs::read_to_string(root.join("out/mapping.csv")).unwrap();
        assert_eq!(csv.lines().count(), 2, "被取消的 B 不应进 mapping: {csv}");
        assert!(csv.ends_with(",A\n"), "{csv}");

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cancel_break_stops_before_next_entry() {
        let root = two_entries_fixture("cancel");
        let fb = FakeBackend::new(&[]);
        let report = compress_with_backend(
            &fb,
            &root.join("in"),
            &root.join("out"),
            &rnd(),
            &CancelFlag::new(),
            &|p| {
                if p.phase == Phase::Compressing && p.done >= 1 {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            },
        )
        .unwrap();

        assert!(report.cancelled);
        assert_eq!(*fb.attempted.borrow(), ["A"], "取消后 B 不应开始");
        assert_eq!(report.entries.len(), 1);
        fs::remove_dir_all(&root).unwrap();
    }

    /// 取消型假后端:cancel_on 条目写出半成品分卷后返回 Err(Cancelled),
    /// 模拟"run 期间进程被 kill"
    struct CancellingBackend {
        cancel_on: Vec<String>,
        attempted: RefCell<Vec<String>>,
    }

    impl CommandBackend for CancellingBackend {
        fn compress_command(
            &self,
            _work: &Path,
            name: &str,
            out_archive: &Path,
            _opts: &Options,
            _attr_hidden: &[String],
        ) -> Result<Command> {
            self.attempted.borrow_mut().push(name.to_string());
            let stem = out_archive
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let ext = out_archive
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_default();
            let dir = out_archive.parent().unwrap().to_path_buf();
            if self.cancel_on.iter().any(|f| *f == name) {
                fs::write(dir.join(format!("{stem}.{ext}.001")), b"part").unwrap();
                return Err(CoreError::Cancelled);
            }
            fs::write(out_archive, b"fake").unwrap();
            Ok(Command::new("fake-unused"))
        }

        fn run(&self, _cmd: &mut Command, _cancel: &crate::CancelFlag) -> Result<()> {
            Ok(())
        }

        fn verify(
            &self,
            _entry: &Path,
            _opts: &Options,
            _cancel: &crate::CancelFlag,
        ) -> Result<()> {
            Ok(())
        }

        fn check_split_size(&self, _size: Option<u64>) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn process_cancel_discards_entry_keeps_done_and_stops_batch() {
        let root = two_entries_fixture("kill");
        let be = CancellingBackend {
            cancel_on: vec!["B".to_string()],
            attempted: RefCell::new(Vec::new()),
        };
        let report = compress_with_backend(
            &be,
            &root.join("in"),
            &root.join("out"),
            &rnd(),
            &CancelFlag::new(),
            &|_| ControlFlow::Continue(()),
        )
        .unwrap();

        // Ok 而非 Err: 取消是正常终态, 半成品已清、已完成条目保留
        assert!(report.cancelled);
        assert_eq!(report.entries.len(), 1, "被杀的 B 不应计入");
        assert_eq!(*be.attempted.borrow(), ["A", "B"], "后续条目不应开始");
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
        assert_eq!(csv.lines().count(), 2, "被杀的 B 不应进 mapping: {csv}");
        fs::remove_dir_all(&root).unwrap();
    }

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

    #[test]
    fn compress_dir_drops_pattern_matched_top_level_entries() {
        let root = unique_temp("orch-exclude");
        fs::create_dir_all(root.join("in/keep")).unwrap();
        fs::create_dir_all(root.join("in/junk")).unwrap();
        fs::write(root.join("in/keep/a.txt"), "a").unwrap();
        fs::write(root.join("in/junk/b.tmp"), "bb").unwrap();

        let mut o = rnd();
        // 顶层目录按 basename 匹配(树内 *.tmp 剔除由 scan 用例与 e2e 覆盖)
        o.exclude.patterns = vec!["junk".into()];
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
            ExcludeReason::Pattern("junk".into())
        );
        // 进度权重只含保留条目(压缩阶段 total=1, 字节 1)
        let ev = events.borrow();
        let comp: Vec<_> = ev
            .iter()
            .filter(|p| p.phase == Phase::Compressing)
            .collect();
        assert_eq!(comp.len(), 1);
        assert_eq!(
            (comp[0].done, comp[0].total, comp[0].total_bytes),
            (1, 1, 1)
        );

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
        assert!(
            matches!(err, CoreError::NoEntries(_)),
            "全排除 = 无条目可压"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}

/// 分阶段 API 测试(_docs/ui-backend.md 5.3/6): scan → plan → compress_entries
/// 与 compress_dir 整体调用等价; plan 在任何压缩开始前完成全部预检。
#[cfg(test)]
mod staged_tests {
    use super::*;
    use crate::backend::CommandBackend;
    use crate::scan::{Entry, EntryKind, scan_dir};
    use crate::testutil::unique_temp;
    use crate::{ExcludeReason, ExcludeRules};
    use std::cell::RefCell;
    use std::process::Command;

    fn entry(name: &str, size: u64) -> Entry {
        Entry {
            name: name.into(),
            kind: EntryKind::Dir,
            size,
            entry_count: 1,
            dst: String::new(),
            format: String::new(),
            exclude: None,
            attr_hidden: Vec::new(),
        }
    }

    fn opts() -> Options {
        Options {
            split_size: None,
            password: None,
            random_name: false,
            ..Options::default()
        }
    }

    fn noop(_: &Progress) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }

    /// in/A/(1 字节) 与 in/B/(4 字节)
    fn fixture(tag: &str) -> PathBuf {
        let root = unique_temp(tag);
        fs::create_dir_all(root.join("in/A")).unwrap();
        fs::create_dir_all(root.join("in/B")).unwrap();
        fs::write(root.join("in/A/a.txt"), "a").unwrap();
        fs::write(root.join("in/B/b.txt"), "bbbb").unwrap();
        root
    }

    /// 记录型假后端:成功时写出 {dst}.zip
    struct Recorder {
        attempted: RefCell<Vec<String>>,
    }

    impl Recorder {
        fn new() -> Self {
            Self {
                attempted: RefCell::new(Vec::new()),
            }
        }
    }

    impl CommandBackend for Recorder {
        fn compress_command(
            &self,
            _work: &Path,
            name: &str,
            out_archive: &Path,
            _opts: &Options,
            _attr_hidden: &[String],
        ) -> Result<Command> {
            self.attempted.borrow_mut().push(name.to_string());
            fs::write(out_archive, b"fake").unwrap();
            Ok(Command::new("fake-unused"))
        }

        fn run(&self, _cmd: &mut Command, _cancel: &crate::CancelFlag) -> Result<()> {
            Ok(())
        }

        fn verify(
            &self,
            _entry: &Path,
            _opts: &Options,
            _cancel: &crate::CancelFlag,
        ) -> Result<()> {
            Ok(())
        }

        fn check_split_size(&self, _size: Option<u64>) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn plan_rejects_empty_entry_set_as_no_entries() {
        // UI 勾选筛选后为空 → 在 plan 处报错(整批不开始)
        let root = fixture("plan-empty");
        let mut es: Vec<Entry> = Vec::new();
        let err = plan(&root.join("in"), &root.join("out"), &mut es, &opts()).unwrap_err();
        assert!(matches!(err, CoreError::NoEntries(_)));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn plan_rejects_out_inside_input_and_fills_dst_when_ok() {
        let root = fixture("plan-basic");

        let mut es = vec![entry("A", 1)];
        let err = plan(&root.join("in"), &root.join("in/out"), &mut es, &opts()).unwrap_err();
        assert!(matches!(err, CoreError::OutInsideInput(_)));

        let mut es = vec![entry("A", 1)];
        plan(&root.join("in"), &root.join("out"), &mut es, &opts()).unwrap();
        assert_eq!(es[0].dst, "A", "plan 应生成 dst");
        assert!(root.join("out").exists(), "plan 应创建输出目录");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn compress_entries_reports_missing_entry_before_any_backend_call() {
        // 扫描到确认之间条目被删: 顶层存在性复核拦下, 整批不开始(宁停不错)
        let root = fixture("missing");
        let fb = Recorder::new();
        let es = vec![entry("A", 1), entry("gone", 1)];
        let err = compress_entries_with_backend(
            &fb,
            &root.join("in"),
            &root.join("out"),
            &es,
            &opts(),
            &CancelFlag::new(),
            &noop,
        )
        .unwrap_err();
        match err {
            CoreError::EntryMissing { name } => assert_eq!(name, "gone"),
            o => panic!("应为 EntryMissing: {o:?}"),
        }
        assert!(fb.attempted.borrow().is_empty(), "复核应先于任何压缩");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn staged_scan_plan_execute_matches_compress_dir() {
        let root = fixture("staged-equiv");

        // 基准: 整体调用
        let r1 = compress_with_backend(
            &Recorder::new(),
            &root.join("in"),
            &root.join("out1"),
            &opts(),
            &CancelFlag::new(),
            &noop,
        )
        .unwrap();

        // 分阶段: scan → plan → compress_entries(事件应与整体一致)
        let events = RefCell::new(Vec::new());
        let scan = scan_dir(&root.join("in"), &crate::ExcludeRules::default(), &|p| {
            events.borrow_mut().push(p.clone());
        })
        .unwrap();
        let mut es = scan.entries;
        plan(&root.join("in"), &root.join("out2"), &mut es, &opts()).unwrap();
        let r2 = compress_entries_with_backend(
            &Recorder::new(),
            &root.join("in"),
            &root.join("out2"),
            &es,
            &opts(),
            &CancelFlag::new(),
            &|p| {
                events.borrow_mut().push(p.clone());
                ControlFlow::Continue(())
            },
        )
        .unwrap();

        // 报告等价: 完成的条目一致
        let o1: Vec<&str> = r1.entries.iter().map(|e| e.original.as_str()).collect();
        let o2: Vec<&str> = r2.entries.iter().map(|e| e.original.as_str()).collect();
        assert_eq!(o1, o2);

        // 事件等价: 分阶段 = Scanning*2 + Compressing*2, 字节加权一致
        let ev = events.borrow();
        let phases: Vec<_> = ev.iter().map(|p| p.phase).collect();
        assert_eq!(
            phases,
            [
                Phase::Scanning,
                Phase::Scanning,
                Phase::Compressing,
                Phase::Compressing
            ]
        );
        assert_eq!((ev[3].done, ev[3].done_bytes, ev[3].total_bytes), (2, 5, 5));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn staged_path_keeps_marked_entries_as_advisory_only() {
        // 分阶段 API(UI): 不做顶层丢弃, 标记仅展示; 勾选名单决定压缩内容
        let root = unique_temp("staged-excl");
        fs::create_dir_all(root.join("in/junk")).unwrap();
        fs::write(root.join("in/junk/b.tmp"), "bb").unwrap();

        let rules = ExcludeRules {
            include_hidden: true,
            patterns: vec!["junk".into()],
        };
        let scan = scan_dir(&root.join("in"), &rules, &|_| {}).unwrap();
        assert_eq!(
            scan.entries[0].exclude,
            Some(ExcludeReason::Pattern("junk".into()))
        );

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
}
