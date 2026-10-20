//! 包级打包: 把 `strategy` 的纯计划落实为多源压缩命令。
//!
//! 与现有 `compress` 逐条目路径并行, 互不影响:
//! - `plan_packages` 产出 [`PlannedPackage`](成员 + 包名 + 类型), 并做输出存在性/重名预检
//! - `compress_packages` 逐包构造多源命令, 写 1:N mapping, 上报包级进度
//!
//! 设计约束:
//! - `Bundle` 不切卷; `Split` 才传 `-s`/`-v`
//! - 包级进度: `done/total` 按包计数, `entry` 是包标签
//! - 7z 成员多时用 `@listfile`; zip 后端(Linux)成员过多时明确报错, 由调用方降低包大小

use std::collections::{HashMap, HashSet};
use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::backend::{Backend, ensure_tool_available};
use crate::compress::{
    absolutize, cleanup_partial, mapping_append, mapping_create, path_within,
    resolve_primary_archive, target_exists,
};
use crate::naming::random_name;
use crate::progress::{Phase, Progress};
use crate::scan::Entry;
use crate::strategy::{self, PackageKind, StrategyItem, StrategyParams};
use crate::{CancelFlag, CoreError, Options, Result};

/// 7z 成员数超过该值时改用 `@listfile`, 避免 Windows 32k 命令行限制。
const SEVENZ_LIST_THRESHOLD: usize = 64;

/// zip 后端没有 listfile 通道; 超过该值明确报错(正常使用 7z 后端不会触发)。
const ZIP_SOURCE_LIMIT: usize = 10_000;

/// 一个已规划的输出包。
#[derive(Debug, Clone)]
pub struct PlannedPackage {
    /// 稳定包 id: `p0` / `p1` ...
    pub id: String,
    pub kind: PackageKind,
    /// 顶层源条目(完整 `Entry`, 保留 attr_hidden 等信息)
    pub members: Vec<Entry>,
    /// 压缩包基础名(不带扩展名)
    pub dst: String,
    pub input_bytes: u64,
    pub entry_count: usize,
}

/// 包级规划结果: 包列表 + strategy 的非致命 warning(如条目数超限改直接)。
#[derive(Debug, Default)]
pub struct PackagePlanSet {
    pub packages: Vec<PlannedPackage>,
    pub warnings: Vec<strategy::StrategyWarning>,
}

impl std::ops::Deref for PackagePlanSet {
    type Target = [PlannedPackage];

    fn deref(&self) -> &Self::Target {
        &self.packages
    }
}

/// 一个实际产出的包。
#[derive(Debug, Clone)]
pub struct PackagedArchive {
    pub id: String,
    pub kind: PackageKind,
    /// 主卷(整包 / zip -s 末卷 / 7z 首卷 .001)
    pub archive: PathBuf,
    /// 成员原名(按方案顺序)
    pub members: Vec<String>,
    pub input_bytes: u64,
    pub entry_count: usize,
}

/// 包级压缩结果。
#[derive(Debug, Default)]
pub struct PackageReport {
    pub packages: Vec<PackagedArchive>,
    pub cancelled: bool,
    pub warnings: Vec<String>,
}

/// 多源压缩命令的成员。`attr_hidden` 是该条目自己的相对隐藏路径。
#[derive(Debug, Clone, Copy)]
pub(crate) struct PackageSource<'a> {
    pub name: &'a str,
    pub attr_hidden: &'a [String],
}

/// 包级后端抽象(测试可注入)。
pub(crate) trait PackageBackend {
    fn kind(&self) -> Backend;
    fn compress_package_command(
        &self,
        work_dir: &Path,
        sources: &[PackageSource<'_>],
        out_archive: &Path,
        opts: &Options,
        list_file: Option<&Path>,
    ) -> Result<Command>;
    fn run_with_percent(
        &self,
        cmd: &mut Command,
        cancel: &CancelFlag,
        on_percent: &mut dyn FnMut(u8),
    ) -> Result<()>;
    fn verify(&self, entry: &Path, opts: &Options, cancel: &CancelFlag) -> Result<()>;
}

impl PackageBackend for Backend {
    fn kind(&self) -> Backend {
        *self
    }

    fn compress_package_command(
        &self,
        work_dir: &Path,
        sources: &[PackageSource<'_>],
        out_archive: &Path,
        opts: &Options,
        list_file: Option<&Path>,
    ) -> Result<Command> {
        Backend::compress_package_command(self, work_dir, sources, out_archive, opts, list_file)
    }

    fn run_with_percent(
        &self,
        cmd: &mut Command,
        cancel: &CancelFlag,
        on_percent: &mut dyn FnMut(u8),
    ) -> Result<()> {
        match self {
            Backend::SevenZip => self.run_streaming(cmd, cancel, &mut |tick| {
                if let Some(percent) = tick.percent {
                    on_percent(percent);
                }
            }),
            Backend::Zip => self.run(cmd, cancel),
        }
    }

    fn verify(&self, entry: &Path, opts: &Options, cancel: &CancelFlag) -> Result<()> {
        Backend::verify(self, entry, opts, cancel)
    }
}

/// 把策略计划落成可执行的包计划(命名 + 输出预检)。
///
/// `packaging` 决定分类/装箱; `opts` 只提供 format/random_name 等命名相关选项。
/// 不执行压缩, 不写 mapping。
pub fn plan_packages(
    input: &Path,
    out_dir: &Path,
    entries: &[Entry],
    packaging: &StrategyParams,
    opts: &Options,
) -> Result<PackagePlanSet> {
    if !input.is_dir() {
        return Err(CoreError::InputNotFound(input.to_path_buf()));
    }

    let input_abs = absolutize(input);
    let out_abs = absolutize(out_dir);
    if path_within(&out_abs, &input_abs) {
        return Err(CoreError::OutInsideInput(out_dir.to_path_buf()));
    }
    fs::create_dir_all(&out_abs)?;

    let backend = opts.format.backend();
    backend.check_split_size(packaging.split_above)?;

    let items: Vec<StrategyItem> = entries
        .iter()
        .map(|e| StrategyItem::new(e.name.clone(), e.size, e.entry_count))
        .collect();
    let strategy_plan = strategy::plan(&items, packaging)
        .map_err(|e| CoreError::InvalidPackaging(e.to_string()))?;
    if strategy_plan.packages.is_empty() {
        return Ok(PackagePlanSet {
            packages: Vec::new(),
            warnings: strategy_plan.warnings,
        });
    }

    let by_name: HashMap<&str, &Entry> = entries.iter().map(|e| (e.name.as_str(), e)).collect();
    let ext = opts.format.extension();
    let mut used: HashSet<String> = HashSet::new();
    let mut bundle_seq = 0usize;
    let mut packages = Vec::with_capacity(strategy_plan.packages.len());

    for (index, package) in strategy_plan.packages.iter().enumerate() {
        let members: Vec<Entry> = package
            .members
            .iter()
            .map(|name| {
                by_name
                    .get(name.as_str())
                    .map(|entry| (*entry).clone())
                    .ok_or_else(|| CoreError::EntryMissing { name: name.clone() })
            })
            .collect::<Result<Vec<_>>>()?;

        match package.kind {
            PackageKind::Bundle if members.len() < 2 => {
                return Err(CoreError::InvalidPackaging(
                    "bundle with fewer than two members".into(),
                ));
            }
            PackageKind::Direct | PackageKind::Split if members.len() != 1 => {
                return Err(CoreError::InvalidPackaging(
                    "direct/split package must contain exactly one member".into(),
                ));
            }
            _ => {}
        }

        let base = match package.kind {
            PackageKind::Bundle => {
                bundle_seq += 1;
                format!("_bundle_{bundle_seq:04}")
            }
            PackageKind::Direct | PackageKind::Split => members[0].name.clone(),
        };
        let dst = assign_dst(
            &out_abs,
            ext,
            opts.random_name,
            &base,
            package.kind,
            &mut used,
        )?;

        packages.push(PlannedPackage {
            id: format!("p{index}"),
            kind: package.kind,
            input_bytes: members.iter().map(|m| m.size).sum(),
            entry_count: members.iter().map(|m| m.entry_count).sum(),
            members,
            dst,
        });
    }

    Ok(PackagePlanSet {
        packages,
        warnings: strategy_plan.warnings,
    })
}

/// 执行包计划。调用方必须传入与 `plan_packages` 相同的 `split_above`。
pub fn compress_packages(
    input: &Path,
    out_dir: &Path,
    plans: &[PlannedPackage],
    opts: &Options,
    split_above: Option<u64>,
    cancel: &CancelFlag,
    on_progress: &dyn Fn(&Progress) -> ControlFlow<()>,
) -> Result<PackageReport> {
    let backend = opts.format.backend();
    ensure_tool_available(backend.program())?;
    backend.check_split_size(split_above)?;
    compress_packages_with_backend(
        &backend,
        input,
        out_dir,
        plans,
        opts,
        split_above,
        cancel,
        on_progress,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn compress_packages_with_backend<B: PackageBackend + ?Sized>(
    backend: &B,
    input: &Path,
    out_dir: &Path,
    plans: &[PlannedPackage],
    opts: &Options,
    split_above: Option<u64>,
    cancel: &CancelFlag,
    on_progress: &dyn Fn(&Progress) -> ControlFlow<()>,
) -> Result<PackageReport> {
    if !input.is_dir() {
        return Err(CoreError::InputNotFound(input.to_path_buf()));
    }
    let input_abs = absolutize(input);
    let out_abs = absolutize(out_dir);
    if path_within(&out_abs, &input_abs) {
        return Err(CoreError::OutInsideInput(out_dir.to_path_buf()));
    }
    fs::create_dir_all(&out_abs)?;

    // 宁停不错: 全部成员在任何压缩开始前做顶层存在性复核
    for plan in plans {
        for member in &plan.members {
            if fs::symlink_metadata(input_abs.join(&member.name)).is_err() {
                return Err(CoreError::EntryMissing {
                    name: member.name.clone(),
                });
            }
        }
    }
    for plan in plans {
        if plan.kind == PackageKind::Split && split_above.is_none() {
            return Err(CoreError::InvalidPackaging(
                "split package requires split_above".into(),
            ));
        }
    }

    let ext = opts.format.extension();
    let write_mapping = opts.random_name || plans.iter().any(|p| p.kind == PackageKind::Bundle);
    if write_mapping {
        mapping_create(&out_abs)?;
    }

    let total = plans.len();
    let total_bytes: u64 = plans.iter().map(|p| p.input_bytes).sum();
    let mut report = PackageReport::default();
    let mut done_bytes = 0u64;

    for (i, plan) in plans.iter().enumerate() {
        let label = package_label(plan);
        let base_bytes = done_bytes;
        let package_bytes = plan.input_bytes;

        if on_progress(&Progress {
            phase: Phase::Compressing,
            done: i,
            total,
            done_bytes: base_bytes,
            total_bytes,
            entry: label.clone(),
        }) == ControlFlow::Break(())
        {
            report.cancelled = true;
            break;
        }

        let out_archive = out_abs.join(format!("{}.{}", plan.dst, ext));
        let sources: Vec<PackageSource<'_>> = plan
            .members
            .iter()
            .map(|member| PackageSource {
                name: member.name.as_str(),
                attr_hidden: &member.attr_hidden,
            })
            .collect();

        let mut pkg_opts = opts.clone();
        pkg_opts.split_size = if plan.kind == PackageKind::Split {
            split_above
        } else {
            None
        };

        if backend.kind() == Backend::Zip && sources.len() > ZIP_SOURCE_LIMIT {
            return Err(CoreError::InvalidPackaging(format!(
                "zip backend supports at most {ZIP_SOURCE_LIMIT} sources per package, got {}; increase the split threshold or use 7z",
                sources.len()
            )));
        }

        let list_file =
            if backend.kind() == Backend::SevenZip && sources.len() > SEVENZ_LIST_THRESHOLD {
                Some(write_source_list(&out_abs, &sources)?)
            } else {
                None
            };

        let run_result = backend
            .compress_package_command(
                &input_abs,
                &sources,
                &out_archive,
                &pkg_opts,
                list_file.as_deref(),
            )
            .and_then(|mut cmd| {
                backend.run_with_percent(&mut cmd, cancel, &mut |percent| {
                    if package_bytes > 0 {
                        let _ = on_progress(&Progress {
                            phase: Phase::Compressing,
                            done: i,
                            total,
                            done_bytes: base_bytes + package_bytes * u64::from(percent) / 100,
                            total_bytes,
                            entry: label.clone(),
                        });
                    }
                })
            });

        if let Some(list) = &list_file {
            let _ = fs::remove_file(list);
        }

        let archive = match run_result {
            Ok(()) => resolve_primary_archive(&out_abs, &plan.dst, ext),
            Err(err) => {
                cleanup_partial(&out_abs, &plan.dst, ext);
                if matches!(err, CoreError::Cancelled) {
                    report.cancelled = true;
                    break;
                }
                return Err(err);
            }
        };

        let verify_result = if opts.verify {
            backend.verify(&archive, &pkg_opts, cancel)
        } else {
            Ok(())
        };
        if let Err(err) = verify_result {
            cleanup_partial(&out_abs, &plan.dst, ext);
            if matches!(err, CoreError::Cancelled) {
                report.cancelled = true;
                break;
            }
            return Err(err);
        }

        // 用户设定的上阈值是硬上限: 原始预算可能因头部/不可压缩数据略超,
        // 产物阶段复核主卷实际大小; 超了宁可失败也不静默交付超限包。
        if let Some(limit) = split_above {
            let actual = fs::metadata(&archive).map(|m| m.len()).unwrap_or(0);
            if actual > limit {
                cleanup_partial(&out_abs, &plan.dst, ext);
                return Err(CoreError::InvalidPackaging(format!(
                    "package {} is {actual} bytes, exceeds split_above {limit}; lower bundle_max or split_above",
                    archive.display()
                )));
            }
        }

        if write_mapping {
            let fname = archive
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            for member in &plan.members {
                mapping_append(&out_abs, &fname, &member.name)?;
            }
        }

        report.packages.push(PackagedArchive {
            id: plan.id.clone(),
            kind: plan.kind,
            archive,
            members: plan.members.iter().map(|m| m.name.clone()).collect(),
            input_bytes: plan.input_bytes,
            entry_count: plan.entry_count,
        });
        done_bytes += package_bytes;

        if on_progress(&Progress {
            phase: Phase::Compressing,
            done: i + 1,
            total,
            done_bytes,
            total_bytes,
            entry: label,
        }) == ControlFlow::Break(())
        {
            report.cancelled = true;
            break;
        }
    }

    Ok(report)
}

fn package_label(plan: &PlannedPackage) -> String {
    match plan.kind {
        PackageKind::Direct | PackageKind::Split => plan.members[0].name.clone(),
        PackageKind::Bundle => plan.dst.clone(),
    }
}

fn assign_dst(
    out_dir: &Path,
    ext: &str,
    random: bool,
    base: &str,
    kind: PackageKind,
    used: &mut HashSet<String>,
) -> Result<String> {
    if random {
        for _ in 0..16 {
            let candidate = format!("zt_{}", random_name());
            let lower = candidate.to_lowercase();
            if !used.contains(&lower) && !target_exists(out_dir, &candidate, ext) {
                used.insert(lower);
                return Ok(candidate);
            }
        }
        return Err(CoreError::OutputExists(out_dir.join(format!("zt_*.{ext}"))));
    }

    if kind == PackageKind::Bundle {
        for suffix in 0..1000u32 {
            let candidate = if suffix == 0 {
                base.to_string()
            } else {
                format!("{base}_{}", suffix + 1)
            };
            let lower = candidate.to_lowercase();
            if !used.contains(&lower) && !target_exists(out_dir, &candidate, ext) {
                used.insert(lower);
                return Ok(candidate);
            }
        }
        return Err(CoreError::OutputExists(
            out_dir.join(format!("{base}.{ext}")),
        ));
    }

    let lower = base.to_lowercase();
    if used.contains(&lower) {
        return Err(CoreError::DuplicateDst(vec![base.to_string()]));
    }
    if target_exists(out_dir, base, ext) {
        return Err(CoreError::OutputExists(
            out_dir.join(format!("{base}.{ext}")),
        ));
    }
    used.insert(lower);
    Ok(base.to_string())
}

fn write_source_list(out_dir: &Path, sources: &[PackageSource<'_>]) -> Result<PathBuf> {
    let path = out_dir.join(format!(".ziptool-list-{}.txt", random_name()));
    let mut text = String::new();
    for source in sources {
        text.push_str(source.name);
        text.push('\n');
    }
    fs::write(&path, text)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::EntryKind;
    use crate::testutil::unique_temp;
    use std::cell::RefCell;
    use std::process::Command;

    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;

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

    fn entry_with_count(name: &str, size: u64, entry_count: usize) -> Entry {
        Entry {
            entry_count,
            ..entry(name, size)
        }
    }

    fn params(merge: Option<u64>, split: Option<u64>) -> StrategyParams {
        StrategyParams {
            merge_below: merge,
            split_above: split,
            bundle_max: None,
            max_entries: None,
        }
    }

    fn opts(random: bool) -> Options {
        Options {
            random_name: random,
            ..Options::default()
        }
    }

    fn make_input(root: &Path, names: &[&str]) {
        fs::create_dir_all(root.join("in")).unwrap();
        for name in names {
            fs::create_dir_all(root.join("in").join(name)).unwrap();
        }
    }

    fn names_of(package: &PlannedPackage) -> Vec<&str> {
        package.members.iter().map(|m| m.name.as_str()).collect()
    }

    #[test]
    fn plans_direct_split_and_bundle_with_expected_names() {
        let root = unique_temp("pkg-plan");
        make_input(
            &root,
            &["FolderA", "FolderB", "fileC", "fileD", "fileE", "folderF"],
        );
        let entries = vec![
            entry("FolderA", 26 * GIB),
            entry("FolderB", 100 * MIB),
            entry("fileC", 16 * KIB),
            entry("fileD", KIB),
            entry("fileE", 100 * MIB),
            entry("folderF", 20 * MIB),
        ];
        let planned = plan_packages(
            &root.join("in"),
            &root.join("out"),
            &entries,
            &params(Some(100 * MIB), Some(2 * GIB)),
            &opts(false),
        )
        .unwrap();

        assert_eq!(planned.len(), 2);
        assert_eq!(planned[0].kind, PackageKind::Split);
        assert_eq!(planned[0].dst, "FolderA");
        assert_eq!(names_of(&planned[0]), ["FolderA"]);

        assert_eq!(planned[1].kind, PackageKind::Bundle);
        assert_eq!(planned[1].dst, "_bundle_0001");
        assert_eq!(
            names_of(&planned[1]),
            ["FolderB", "fileE", "folderF", "fileC", "fileD"]
        );
        assert_eq!(planned[1].input_bytes, 220 * MIB + 17 * KIB);

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn bundle_name_avoids_existing_output() {
        let root = unique_temp("pkg-name-collision");
        make_input(&root, &["A", "B"]);
        fs::create_dir_all(root.join("out")).unwrap();
        fs::write(root.join("out/_bundle_0001.zip"), b"old").unwrap();

        let entries = vec![entry("A", 50 * MIB), entry("B", 50 * MIB)];
        let planned = plan_packages(
            &root.join("in"),
            &root.join("out"),
            &entries,
            &params(Some(100 * MIB), Some(2 * GIB)),
            &opts(false),
        )
        .unwrap();

        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].dst, "_bundle_0001_2");

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn plan_preserves_strategy_warnings() {
        let root = unique_temp("pkg-warning");
        make_input(&root, &["bigdir", "small"]);
        let mut p = params(Some(100 * MIB), Some(2 * GIB));
        p.max_entries = Some(10);
        let entries = vec![
            entry_with_count("bigdir", 10 * MIB, 20),
            entry_with_count("small", 10 * MIB, 1),
        ];

        let set = plan_packages(
            &root.join("in"),
            &root.join("out"),
            &entries,
            &p,
            &opts(false),
        )
        .unwrap();
        assert_eq!(set.warnings.len(), 1);
        assert_eq!(set.warnings[0].item, "bigdir");
        assert!(set.packages.iter().any(|package| {
            package.kind == PackageKind::Direct && package.members[0].name == "bigdir"
        }));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn non_random_direct_existing_output_is_rejected() {
        let root = unique_temp("pkg-output-exists");
        make_input(&root, &["A"]);
        fs::create_dir_all(root.join("out")).unwrap();
        fs::write(root.join("out/A.zip"), b"old").unwrap();

        let entries = vec![entry("A", 500 * MIB)];
        let err = plan_packages(
            &root.join("in"),
            &root.join("out"),
            &entries,
            &params(Some(100 * MIB), Some(2 * GIB)),
            &opts(false),
        )
        .unwrap_err();

        assert!(matches!(err, CoreError::OutputExists(_)));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn random_names_are_unique_and_prefixed() {
        let root = unique_temp("pkg-random");
        make_input(&root, &["A", "B", "C"]);
        let entries = vec![
            entry("A", 3 * GIB),
            entry("B", 50 * MIB),
            entry("C", 50 * MIB),
        ];
        let planned = plan_packages(
            &root.join("in"),
            &root.join("out"),
            &entries,
            &params(Some(100 * MIB), Some(2 * GIB)),
            &opts(true),
        )
        .unwrap();

        assert_eq!(planned.len(), 2);
        for package in planned.iter() {
            assert!(package.dst.starts_with("zt_"), "{:?}", package.dst);
            assert_eq!(package.dst.len(), 19, "zt_ + 16 hex");
        }
        assert_ne!(planned[0].dst, planned[1].dst);

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn empty_entries_yield_empty_plan() {
        let root = unique_temp("pkg-empty");
        make_input(&root, &[]);
        let planned = plan_packages(
            &root.join("in"),
            &root.join("out"),
            &[],
            &params(Some(100 * MIB), Some(2 * GIB)),
            &opts(false),
        )
        .unwrap();
        assert!(planned.is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn output_inside_input_is_rejected() {
        let root = unique_temp("pkg-out-inside");
        make_input(&root, &["A"]);
        let err = plan_packages(
            &root.join("in"),
            &root.join("in/out"),
            &[entry("A", 1)],
            &params(Some(100 * MIB), Some(2 * GIB)),
            &opts(false),
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::OutInsideInput(_)));
        fs::remove_dir_all(&root).unwrap();
    }

    #[derive(Debug, Clone)]
    struct Recorded {
        out: PathBuf,
        names: Vec<String>,
        split: Option<u64>,
        list: Option<PathBuf>,
    }

    struct FakeBackend {
        kind: Backend,
        calls: RefCell<usize>,
        recorded: RefCell<Vec<Recorded>>,
        fail_at: Option<usize>,
        cancel_at: Option<usize>,
        verify_fail: bool,
        write_len: usize,
    }

    impl FakeBackend {
        fn new(kind: Backend) -> Self {
            Self {
                kind,
                calls: RefCell::new(0),
                recorded: RefCell::new(Vec::new()),
                fail_at: None,
                cancel_at: None,
                verify_fail: false,
                write_len: 4,
            }
        }

        fn fail_at(mut self, n: usize) -> Self {
            self.fail_at = Some(n);
            self
        }

        fn cancel_at(mut self, n: usize) -> Self {
            self.cancel_at = Some(n);
            self
        }

        fn verify_failing(mut self) -> Self {
            self.verify_fail = true;
            self
        }

        fn writing(mut self, len: usize) -> Self {
            self.write_len = len;
            self
        }
    }

    impl PackageBackend for FakeBackend {
        fn kind(&self) -> Backend {
            self.kind
        }

        fn compress_package_command(
            &self,
            _work_dir: &Path,
            sources: &[PackageSource<'_>],
            out_archive: &Path,
            opts: &Options,
            list_file: Option<&Path>,
        ) -> Result<Command> {
            self.recorded.borrow_mut().push(Recorded {
                out: out_archive.to_path_buf(),
                names: sources.iter().map(|s| s.name.to_string()).collect(),
                split: opts.split_size,
                list: list_file.map(Path::to_path_buf),
            });
            Ok(Command::new("fake-package-backend"))
        }

        fn run_with_percent(
            &self,
            _cmd: &mut Command,
            _cancel: &CancelFlag,
            on_percent: &mut dyn FnMut(u8),
        ) -> Result<()> {
            let call = {
                let mut calls = self.calls.borrow_mut();
                *calls += 1;
                *calls
            };
            if self.fail_at == Some(call) {
                return Err(CoreError::BackendFailed {
                    program: "fake".into(),
                    status: "exit status: 1".into(),
                    output: "boom".into(),
                });
            }
            if self.cancel_at == Some(call) {
                return Err(CoreError::Cancelled);
            }
            let out = self
                .recorded
                .borrow()
                .last()
                .map(|r| r.out.clone())
                .expect("command recorded before run");
            fs::write(out, vec![0u8; self.write_len]).unwrap();
            on_percent(50);
            on_percent(100);
            Ok(())
        }

        fn verify(&self, entry: &Path, _opts: &Options, _cancel: &CancelFlag) -> Result<()> {
            if self.verify_fail {
                return Err(CoreError::VerifyFailed {
                    archive: entry.to_path_buf(),
                    status: "exit code: 2".into(),
                    output: "bad".into(),
                });
            }
            Ok(())
        }
    }

    fn compress_plans(
        root: &Path,
        backend: &FakeBackend,
        planned: &[PlannedPackage],
    ) -> Result<PackageReport> {
        compress_packages_with_backend(
            backend,
            &root.join("in"),
            &root.join("out"),
            planned,
            &opts(false),
            Some(2 * GIB),
            &CancelFlag::new(),
            &|_| ControlFlow::Continue(()),
        )
    }

    #[test]
    fn bundle_and_split_are_compressed_with_expected_sources() {
        let root = unique_temp("pkg-compress");
        make_input(&root, &["A", "B", "C"]);
        let entries = vec![
            entry("A", 50 * MIB),
            entry("B", 50 * MIB),
            entry("C", 3 * GIB),
        ];
        let planned = plan_packages(
            &root.join("in"),
            &root.join("out"),
            &entries,
            &params(Some(100 * MIB), Some(2 * GIB)),
            &opts(false),
        )
        .unwrap();
        let backend = FakeBackend::new(Backend::SevenZip);
        let report = compress_plans(&root, &backend, &planned).unwrap();

        assert_eq!(report.packages.len(), 2);
        let recorded = backend.recorded.borrow();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0].names, ["C"]);
        assert_eq!(recorded[0].split, Some(2 * GIB));
        assert_eq!(recorded[1].names, ["A", "B"]);
        assert_eq!(recorded[1].split, None);
        assert_eq!(report.packages[0].kind, PackageKind::Split);
        assert_eq!(report.packages[1].kind, PackageKind::Bundle);

        let mapping = fs::read_to_string(root.join("out/mapping.csv")).unwrap();
        let lines: Vec<&str> = mapping.lines().collect();
        assert_eq!(lines[0], "archive,original");
        assert_eq!(lines.len(), 4, "C + bundle A/B + header: {mapping}");
        assert!(mapping.contains(",C\n"));
        assert!(mapping.contains(",A\n"));
        assert!(mapping.contains(",B\n"));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn progress_is_package_level_and_byte_weighted() {
        let root = unique_temp("pkg-progress");
        make_input(&root, &["A", "B"]);
        let entries = vec![entry("A", 100 * MIB), entry("B", 100 * MIB)];
        let planned = plan_packages(
            &root.join("in"),
            &root.join("out"),
            &entries,
            &params(Some(100 * MIB), Some(2 * GIB)),
            &opts(false),
        )
        .unwrap();
        let backend = FakeBackend::new(Backend::SevenZip);
        let events = RefCell::new(Vec::new());
        compress_packages_with_backend(
            &backend,
            &root.join("in"),
            &root.join("out"),
            &planned,
            &opts(false),
            Some(2 * GIB),
            &CancelFlag::new(),
            &|p| {
                events
                    .borrow_mut()
                    .push((p.done, p.total, p.done_bytes, p.entry.clone()));
                ControlFlow::Continue(())
            },
        )
        .unwrap();

        let events = events.borrow();
        assert_eq!(events[0], (0, 1, 0, "_bundle_0001".into()));
        assert_eq!(
            events.last().unwrap(),
            &(1, 1, 200 * MIB, "_bundle_0001".into())
        );
        assert!(events.iter().any(|(_, _, bytes, _)| *bytes == 100 * MIB));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn many_sources_use_list_file_for_sevenz() {
        let root = unique_temp("pkg-list");
        let names: Vec<String> = (0..65).map(|i| format!("d{i:02}")).collect();
        let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
        make_input(&root, &name_refs);
        let entries: Vec<Entry> = names.iter().map(|n| entry(n, MIB)).collect();
        let planned = plan_packages(
            &root.join("in"),
            &root.join("out"),
            &entries,
            &params(Some(100 * MIB), Some(2 * GIB)),
            &opts(false),
        )
        .unwrap();
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].members.len(), 65);

        let backend = FakeBackend::new(Backend::SevenZip);
        compress_plans(&root, &backend, &planned).unwrap();
        let recorded = backend.recorded.borrow();
        let list = recorded[0]
            .list
            .as_ref()
            .expect("65 sources need list file");
        assert!(!list.exists(), "list file should be cleaned after run");
        assert_eq!(recorded[0].names.len(), 65);

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cancel_keeps_completed_packages_and_cleans_partial() {
        let root = unique_temp("pkg-cancel");
        make_input(&root, &["A", "B", "C"]);
        let entries = vec![
            entry("A", 50 * MIB),
            entry("B", 50 * MIB),
            entry("C", 3 * GIB),
        ];
        let planned = plan_packages(
            &root.join("in"),
            &root.join("out"),
            &entries,
            &params(Some(100 * MIB), Some(2 * GIB)),
            &opts(false),
        )
        .unwrap();
        let backend = FakeBackend::new(Backend::SevenZip).cancel_at(2);
        let report = compress_plans(&root, &backend, &planned).unwrap();

        assert!(report.cancelled);
        assert_eq!(report.packages.len(), 1);
        assert!(root.join("out/C.zip").exists(), "first package kept");
        assert!(
            !root.join("out/_bundle_0001.zip").exists(),
            "cancelled bundle partial must be removed"
        );
        let mapping = fs::read_to_string(root.join("out/mapping.csv")).unwrap();
        assert_eq!(
            mapping.lines().count(),
            2,
            "only completed package mapping: {mapping}"
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn output_exceeding_split_above_is_rejected_and_cleaned() {
        let root = unique_temp("pkg-over-limit");
        make_input(&root, &["A"]);
        let planned = vec![PlannedPackage {
            id: "p0".into(),
            kind: PackageKind::Direct,
            members: vec![entry("A", 1)],
            dst: "A".into(),
            input_bytes: 1,
            entry_count: 1,
        }];
        let backend = FakeBackend::new(Backend::SevenZip).writing(100);
        let err = compress_packages_with_backend(
            &backend,
            &root.join("in"),
            &root.join("out"),
            &planned,
            &opts(false),
            Some(5),
            &CancelFlag::new(),
            &|_| ControlFlow::Continue(()),
        )
        .unwrap_err();

        assert!(matches!(err, CoreError::InvalidPackaging(_)));
        assert!(!root.join("out/A.zip").exists(), "超限产物必须按半成品清理");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn backend_failure_cleans_partial_and_stops() {
        let root = unique_temp("pkg-fail");
        make_input(&root, &["A", "B", "C"]);
        let entries = vec![
            entry("A", 50 * MIB),
            entry("B", 50 * MIB),
            entry("C", 3 * GIB),
        ];
        let planned = plan_packages(
            &root.join("in"),
            &root.join("out"),
            &entries,
            &params(Some(100 * MIB), Some(2 * GIB)),
            &opts(false),
        )
        .unwrap();
        let backend = FakeBackend::new(Backend::SevenZip).fail_at(2);
        let err = compress_plans(&root, &backend, &planned).unwrap_err();

        assert!(matches!(err, CoreError::BackendFailed { .. }));
        assert!(root.join("out/C.zip").exists(), "first package kept");
        assert!(
            !root.join("out/_bundle_0001.zip").exists(),
            "failed bundle partial must be removed"
        );
        assert_eq!(
            fs::read_to_string(root.join("out/mapping.csv"))
                .unwrap()
                .lines()
                .count(),
            2,
            "failed package must not enter mapping"
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn verify_failure_cleans_partial_and_stops() {
        let root = unique_temp("pkg-verify");
        make_input(&root, &["A", "B"]);
        let entries = vec![entry("A", 50 * MIB), entry("B", 50 * MIB)];
        let planned = plan_packages(
            &root.join("in"),
            &root.join("out"),
            &entries,
            &params(Some(100 * MIB), Some(2 * GIB)),
            &opts(false),
        )
        .unwrap();
        let backend = FakeBackend::new(Backend::SevenZip).verify_failing();
        let err = compress_plans(&root, &backend, &planned).unwrap_err();
        assert!(matches!(err, CoreError::VerifyFailed { .. }));
        assert!(!root.join("out/_bundle_0001.zip").exists());
        assert_eq!(
            fs::read_to_string(root.join("out/mapping.csv"))
                .unwrap()
                .lines()
                .count(),
            1,
            "failed package must not enter mapping"
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_member_is_rejected_before_any_backend_call() {
        let root = unique_temp("pkg-missing");
        make_input(&root, &["A", "B"]);
        let entries = vec![entry("A", 50 * MIB), entry("B", 50 * MIB)];
        let planned = plan_packages(
            &root.join("in"),
            &root.join("out"),
            &entries,
            &params(Some(100 * MIB), Some(2 * GIB)),
            &opts(false),
        )
        .unwrap();
        fs::remove_dir_all(root.join("in/B")).unwrap();
        let backend = FakeBackend::new(Backend::SevenZip);
        let err = compress_plans(&root, &backend, &planned).unwrap_err();
        assert!(matches!(err, CoreError::EntryMissing { .. }));
        assert!(backend.recorded.borrow().is_empty());

        fs::remove_dir_all(&root).unwrap();
    }
}
