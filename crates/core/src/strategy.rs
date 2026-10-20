//! 打包策略: 纯内存规划, 不执行压缩。
//!
//! 输入是顶层条目列表([`StrategyItem`])与策略参数([`StrategyParams`]),
//! 输出是每个输出包由哪些条目组成([`StrategyPlan`])。
//!
//! 分类规则(阈值均为原始大小):
//! - `size > split_above` → [`PackageKind::Split`] (单条目, 由执行层分包)
//! - `size <= merge_below` → 合并候选
//! - 其余 → [`PackageKind::Direct`] (单条目, 不分包)
//!
//! 合并候选按大小降序做首次适应递减(FFD)装箱, 每包满足
//! `input_bytes <= bundle_max` 且 `entry_count <= max_entries`。
//! 只有一个成员的合并包退化为 `Direct`, 以便执行层沿用原条目名。
//!
//! 本模块不做压缩、命名、写盘, 也不估计压缩后大小; 因此可以用纯单测
//! 覆盖全部决策。

use std::collections::HashSet;
use std::fmt;

/// 默认下阈值: 100 MiB。`size <= merge_below` 的条目进入合并池。
pub const DEFAULT_MERGE_BELOW: u64 = 100 * 1024 * 1024;

/// 默认上阈值: 2 GiB。`size > split_above` 的条目由执行层分包;
/// 同时也是 [`StrategyParams::bundle_max`] 的默认来源。
pub const DEFAULT_SPLIT_ABOVE: u64 = 2 * 1024 * 1024 * 1024;

/// 默认合并包条目数上限。目录自身也计 1 个条目。
pub const DEFAULT_MAX_ENTRIES: usize = 50_000;

/// 打包策略参数。所有阈值都可选; `None` 表示关闭对应维度。
///
/// 默认值 = 下阈值 100 MiB、上阈值 2 GiB、`bundle_max` 取上阈值、
/// 合并包条目数上限 50k。需要"不合并也不分包"时用 [`Self::disabled`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategyParams {
    /// 下阈值: `size <= merge_below` 的顶层条目进入合并池; `None` = 不合并。
    pub merge_below: Option<u64>,
    /// 上阈值: `size > split_above` 的顶层条目单独分包; `None` = 不分包。
    pub split_above: Option<u64>,
    /// 单个合并包的原始大小上限; `None` = 取 `split_above`;
    /// 若 `split_above` 也为 `None`, 取 [`DEFAULT_SPLIT_ABOVE`]。
    pub bundle_max: Option<u64>,
    /// 单个合并包的条目数上限(普通文件 + 目录); `None` = 不限制。
    /// 单条目 `Direct` / `Split` 包不受此约束(其条目数由源目录决定)。
    pub max_entries: Option<usize>,
}

impl Default for StrategyParams {
    fn default() -> Self {
        Self {
            merge_below: Some(DEFAULT_MERGE_BELOW),
            split_above: Some(DEFAULT_SPLIT_ABOVE),
            bundle_max: None,
            max_entries: Some(DEFAULT_MAX_ENTRIES),
        }
    }
}

impl StrategyParams {
    /// 关闭合并与分包: 每个顶层条目各自成为一个 `Direct` 包。
    pub fn disabled() -> Self {
        Self {
            merge_below: None,
            split_above: None,
            bundle_max: None,
            max_entries: None,
        }
    }

    /// 实际生效的合并包大小上限。
    ///
    /// 顺序: 显式 `bundle_max` → `split_above` → [`DEFAULT_SPLIT_ABOVE`]。
    /// 不启用合并时返回 `None`(此时该参数没有意义)。
    pub fn effective_bundle_max(&self) -> Option<u64> {
        // 不启用合并时, 该参数没有意义。
        self.merge_below?;
        self.bundle_max
            .or(self.split_above)
            .or(Some(DEFAULT_SPLIT_ABOVE))
    }

    /// 参数自洽性校验。`plan` 会先调用本函数。
    ///
    /// 规则:
    /// - `split_above` / `bundle_max` / `max_entries` 若显式给出, 必须 > 0;
    /// - `merge_below <= split_above`(两者都给时);
    /// - `merge_below <= effective bundle_max`;
    /// - `effective bundle_max <= split_above`(两者都有时)。
    pub fn validate(&self) -> Result<(), StrategyError> {
        if self.split_above == Some(0) {
            return Err(StrategyError::InvalidParams(
                "split_above must be > 0".into(),
            ));
        }
        if self.bundle_max == Some(0) {
            return Err(StrategyError::InvalidParams(
                "bundle_max must be > 0".into(),
            ));
        }
        if self.max_entries == Some(0) {
            return Err(StrategyError::InvalidParams(
                "max_entries must be > 0".into(),
            ));
        }

        match (self.merge_below, self.split_above) {
            (Some(merge), Some(split)) if merge > split => {
                return Err(StrategyError::InvalidParams(
                    "merge_below must be <= split_above".into(),
                ));
            }
            _ => {}
        }

        if let Some(merge) = self.merge_below {
            let Some(bundle) = self.effective_bundle_max() else {
                return Err(StrategyError::InvalidParams(
                    "merge_below requires bundle_max".into(),
                ));
            };
            if merge > bundle {
                return Err(StrategyError::InvalidParams(
                    "merge_below must be <= effective bundle_max".into(),
                ));
            }
            if let Some(split) = self.split_above.filter(|split| bundle > *split) {
                return Err(StrategyError::InvalidParams(format!(
                    "bundle_max must be <= split_above ({bundle} > {split})"
                )));
            }
        }

        Ok(())
    }
}

/// 策略输入: 一个顶层条目。
///
/// 顶层条目可以是普通文件或目录; `size` / `entry_count` 都是递归聚合值,
/// 由扫描层提供。`entry_count` 用于限制合并包内的条目数: 普通文件记 1,
/// 目录自身也记 1(空目录 = 1)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategyItem {
    /// 条目名(不含路径), 解压后也会作为包内根路径。
    pub name: String,
    /// 原始字节大小。
    pub size: u64,
    /// 包内条目数(普通文件 + 目录)。
    pub entry_count: usize,
}

impl StrategyItem {
    pub fn new(name: impl Into<String>, size: u64, entry_count: usize) -> Self {
        Self {
            name: name.into(),
            size,
            entry_count,
        }
    }
}

/// 输出包类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageKind {
    /// 单条目直接压缩, 不分包。
    Direct,
    /// 单条目压缩, 且原始大小超过上阈值; 由执行层按上阈值分包。
    Split,
    /// 多个顶层条目合并为一个包。
    Bundle,
}

/// 一个输出包的打包计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagePlan {
    pub kind: PackageKind,
    /// 成员原名。`Direct` / `Split` 恰好一个; `Bundle` 至少两个。
    pub members: Vec<String>,
    /// 成员原始大小之和。
    pub input_bytes: u64,
    /// 成员条目数之和。
    pub entry_count: usize,
}

impl PackagePlan {
    fn single(kind: PackageKind, item: &StrategyItem) -> Self {
        Self {
            kind,
            members: vec![item.name.clone()],
            input_bytes: item.size,
            entry_count: item.entry_count,
        }
    }

    /// 是否为合并包。单成员合并包已在规划阶段退化为 `Direct`, 因此
    /// `true` 等价于成员数 >= 2。
    pub fn is_bundle(&self) -> bool {
        self.kind == PackageKind::Bundle
    }

    pub fn member_count(&self) -> usize {
        self.members.len()
    }
}

/// 策略无法满足的输入或参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrategyError {
    /// 参数组合不自洽, 或显式阈值非法。
    InvalidParams(String),
    /// 顶层条目名在大小写不敏感比较下重复; 输出盘(NTFS/FAT)无法共存。
    DuplicateName(String),
}

impl fmt::Display for StrategyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidParams(msg) => write!(f, "invalid strategy params: {msg}"),
            Self::DuplicateName(name) => {
                write!(f, "duplicate item name (case-insensitive): {name}")
            }
        }
    }
}

impl std::error::Error for StrategyError {}

/// 规划过程中发现的非致命问题。对应条目仍会进入计划(通常退化为 `Direct`)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategyWarning {
    /// 关联条目名。
    pub item: String,
    pub reason: WarningReason,
}

/// 非致命问题的具体原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarningReason {
    /// 单条目自身条目数就超过 `max_entries`, 无法放进任何合并包。
    TooManyEntries { entry_count: usize, limit: usize },
}

/// 打包策略结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StrategyPlan {
    /// 输出包。顺序: 输入顺序中的 `Direct` / `Split`, 然后是合并包
    /// (按 FFD 创建顺序; 单成员合并包以 `Direct` 追加在末尾)。
    pub packages: Vec<PackagePlan>,
    /// 非致命问题。
    pub warnings: Vec<StrategyWarning>,
}

impl StrategyPlan {
    pub fn package_count(&self) -> usize {
        self.packages.len()
    }

    pub fn direct_count(&self) -> usize {
        self.packages
            .iter()
            .filter(|p| p.kind == PackageKind::Direct)
            .count()
    }

    pub fn split_count(&self) -> usize {
        self.packages
            .iter()
            .filter(|p| p.kind == PackageKind::Split)
            .count()
    }

    pub fn bundle_count(&self) -> usize {
        self.packages
            .iter()
            .filter(|p| p.kind == PackageKind::Bundle)
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }
}

/// 生成打包策略。纯内存操作, 不访问文件系统、不执行压缩。
///
/// 步骤:
/// 1. 校验参数与条目名唯一性;
/// 2. 按 `Split` → 合并候选 → `Direct` 分类(顺序不能反, 上阈值优先);
/// 3. 合并候选按大小降序做 FFD 装箱;
/// 4. 单成员合并包退化为 `Direct`。
///
/// `Split` 只表示"该条目原始大小超过上阈值, 执行层应分包", 本函数不决定
/// 实际卷数; 压缩后大小与卷数由执行层负责。
pub fn plan(
    items: &[StrategyItem],
    params: &StrategyParams,
) -> Result<StrategyPlan, StrategyError> {
    params.validate()?;
    ensure_unique_names(items)?;

    let mut packages = Vec::new();
    let mut candidates: Vec<&StrategyItem> = Vec::new();
    let mut warnings = Vec::new();

    for item in items {
        // 上阈值优先: 超过上阈值的条目即使也满足下阈值, 也按分包处理。
        // (参数校验保证 merge_below <= split_above, 正常不会重叠。)
        if params.split_above.is_some_and(|limit| item.size > limit) {
            packages.push(PackagePlan::single(PackageKind::Split, item));
            continue;
        }

        if params.merge_below.is_some_and(|limit| item.size <= limit) {
            if let Some(limit) = params.max_entries.filter(|limit| item.entry_count > *limit) {
                warnings.push(StrategyWarning {
                    item: item.name.clone(),
                    reason: WarningReason::TooManyEntries {
                        entry_count: item.entry_count,
                        limit,
                    },
                });
                packages.push(PackagePlan::single(PackageKind::Direct, item));
                continue;
            }
            candidates.push(item);
            continue;
        }

        packages.push(PackagePlan::single(PackageKind::Direct, item));
    }

    if let Some(bundle_max) = params.effective_bundle_max() {
        pack_candidates(&mut packages, candidates, bundle_max, params.max_entries);
    }

    Ok(StrategyPlan { packages, warnings })
}

/// FFD 装箱。调用方保证 `bundle_max` 与每个候选的大小兼容。
fn pack_candidates<'a>(
    packages: &mut Vec<PackagePlan>,
    mut candidates: Vec<&'a StrategyItem>,
    bundle_max: u64,
    max_entries: Option<usize>,
) {
    if candidates.is_empty() {
        return;
    }

    // 大小降序、名字升序: 结果与输入顺序无关, 便于复现与测试。
    candidates.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));

    let mut builders: Vec<BundleBuilder<'a>> = Vec::new();
    for item in candidates {
        if let Some(builder) = builders
            .iter_mut()
            .find(|builder| builder.can_add(item, bundle_max, max_entries))
        {
            builder.add(item);
        } else {
            builders.push(BundleBuilder::new(item));
        }
    }

    for builder in builders {
        if builder.items.len() == 1 {
            // 单成员合并包没有合并收益, 退化为直接包以保留原名。
            packages.push(PackagePlan::single(PackageKind::Direct, builder.items[0]));
        } else {
            packages.push(builder.into_package());
        }
    }
}

struct BundleBuilder<'a> {
    items: Vec<&'a StrategyItem>,
    bytes: u64,
    entries: usize,
}

impl<'a> BundleBuilder<'a> {
    fn new(item: &'a StrategyItem) -> Self {
        Self {
            items: vec![item],
            bytes: item.size,
            entries: item.entry_count,
        }
    }

    fn can_add(&self, item: &StrategyItem, bundle_max: u64, max_entries: Option<usize>) -> bool {
        let Some(bytes) = self.bytes.checked_add(item.size) else {
            return false;
        };
        if bytes > bundle_max {
            return false;
        }
        if let Some(limit) = max_entries {
            let Some(entries) = self.entries.checked_add(item.entry_count) else {
                return false;
            };
            if entries > limit {
                return false;
            }
        }
        true
    }

    fn add(&mut self, item: &'a StrategyItem) {
        self.bytes += item.size;
        self.entries += item.entry_count;
        self.items.push(item);
    }

    fn into_package(self) -> PackagePlan {
        PackagePlan {
            kind: PackageKind::Bundle,
            members: self.items.iter().map(|item| item.name.clone()).collect(),
            input_bytes: self.bytes,
            entry_count: self.entries,
        }
    }
}

/// 输出盘通常是 NTFS/FAT, 大小写不敏感; 在规划阶段就拒绝冲突,
/// 避免压到一半才失败。
fn ensure_unique_names(items: &[StrategyItem]) -> Result<(), StrategyError> {
    let mut seen = HashSet::with_capacity(items.len());
    for item in items {
        if !seen.insert(item.name.to_lowercase()) {
            return Err(StrategyError::DuplicateName(item.name.clone()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;

    fn item(name: &str, size: u64) -> StrategyItem {
        StrategyItem::new(name, size, 1)
    }

    fn item_entries(name: &str, size: u64, entry_count: usize) -> StrategyItem {
        StrategyItem::new(name, size, entry_count)
    }

    fn params(
        merge_below: Option<u64>,
        split_above: Option<u64>,
        bundle_max: Option<u64>,
        max_entries: Option<usize>,
    ) -> StrategyParams {
        StrategyParams {
            merge_below,
            split_above,
            bundle_max,
            max_entries,
        }
    }

    fn assert_members(package: &PackagePlan, expected: &[&str]) {
        assert_eq!(
            package.members,
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn default_params_use_documented_defaults() {
        let p = StrategyParams::default();
        assert_eq!(p.merge_below, Some(100 * MIB));
        assert_eq!(p.split_above, Some(2 * GIB));
        assert_eq!(p.bundle_max, None);
        assert_eq!(p.effective_bundle_max(), Some(2 * GIB));
        assert_eq!(p.max_entries, Some(DEFAULT_MAX_ENTRIES));
        p.validate().unwrap();
    }

    #[test]
    fn disabled_params_emit_every_item_as_direct() {
        let items = [item("A", 3 * GIB), item("b", 1)];
        let plan = plan(&items, &StrategyParams::disabled()).unwrap();

        assert_eq!(plan.package_count(), 2);
        assert_eq!(plan.direct_count(), 2);
        assert_eq!(plan.split_count(), 0);
        assert_eq!(plan.bundle_count(), 0);
        assert!(plan.warnings.is_empty());
        assert_members(&plan.packages[0], &["A"]);
        assert_members(&plan.packages[1], &["b"]);
    }

    #[test]
    fn partitions_example_and_bundles_all_fragments() {
        let items = [
            item("FolderA", 26 * GIB),
            item("FolderB", 100 * MIB),
            item("fileC", 16 * KIB),
            item("fileD", KIB),
            item("fileE", 100 * MIB),
            item("folderF", 20 * MIB),
        ];
        let p = params(
            Some(100 * MIB),
            Some(2 * GIB),
            None,
            Some(DEFAULT_MAX_ENTRIES),
        );
        let plan = plan(&items, &p).unwrap();

        assert_eq!(plan.split_count(), 1);
        assert_eq!(plan.packages[0].kind, PackageKind::Split);
        assert_members(&plan.packages[0], &["FolderA"]);

        assert_eq!(plan.direct_count(), 0);
        assert_eq!(plan.bundle_count(), 1);
        let bundle = plan.packages.last().unwrap();
        assert_eq!(bundle.kind, PackageKind::Bundle);
        assert_eq!(bundle.members.len(), 5);
        assert_eq!(bundle.input_bytes, 220 * MIB + 17 * KIB);
        assert_eq!(bundle.entry_count, 5);
    }

    #[test]
    fn split_uses_strictly_greater_than() {
        let items = [item("at", 2 * GIB), item("over", 2 * GIB + 1)];
        let p = params(Some(100 * MIB), Some(2 * GIB), None, None);
        let plan = plan(&items, &p).unwrap();

        assert_eq!(plan.packages[0].kind, PackageKind::Direct);
        assert_eq!(plan.packages[1].kind, PackageKind::Split);
    }

    #[test]
    fn merge_uses_less_than_or_equal_and_over_threshold_is_direct() {
        let items = [
            item("at1", 100 * MIB),
            item("at2", 100 * MIB),
            item("over", 100 * MIB + 1),
        ];
        let p = params(Some(100 * MIB), Some(2 * GIB), None, None);
        let plan = plan(&items, &p).unwrap();

        assert_eq!(plan.bundle_count(), 1);
        assert_eq!(plan.direct_count(), 1);
        let bundle = plan
            .packages
            .iter()
            .find(|package| package.kind == PackageKind::Bundle)
            .unwrap();
        assert_members(bundle, &["at1", "at2"]);
        let direct = plan
            .packages
            .iter()
            .find(|package| package.kind == PackageKind::Direct)
            .unwrap();
        assert_members(direct, &["over"]);
    }

    #[test]
    fn single_member_bundle_degrades_to_direct() {
        let items = [item("only", 50 * MIB)];
        let p = params(Some(100 * MIB), Some(2 * GIB), Some(100 * MIB), None);
        let plan = plan(&items, &p).unwrap();

        assert_eq!(plan.bundle_count(), 0);
        assert_eq!(plan.direct_count(), 1);
        assert_members(&plan.packages[0], &["only"]);
    }

    #[test]
    fn ffd_packs_descending_items_into_bundles() {
        let items = [
            item("a60", 60 * MIB),
            item("b40", 40 * MIB),
            item("c40", 40 * MIB),
            item("d20", 20 * MIB),
        ];
        let p = params(Some(100 * MIB), Some(2 * GIB), Some(100 * MIB), None);
        let plan = plan(&items, &p).unwrap();

        assert_eq!(plan.bundle_count(), 2);
        let bundles: Vec<&PackagePlan> = plan
            .packages
            .iter()
            .filter(|package| package.kind == PackageKind::Bundle)
            .collect();
        assert_members(bundles[0], &["a60", "b40"]);
        assert_members(bundles[1], &["c40", "d20"]);
        assert_eq!(bundles[0].input_bytes, 100 * MIB);
        assert_eq!(bundles[1].input_bytes, 60 * MIB);
    }

    #[test]
    fn max_entries_splits_bundles_and_singleton_degrades_to_direct() {
        let items = [
            item_entries("a", 10 * MIB, 3),
            item_entries("b", 10 * MIB, 3),
            item_entries("c", 10 * MIB, 3),
        ];
        let p = params(Some(100 * MIB), Some(2 * GIB), Some(100 * MIB), Some(6));
        let plan = plan(&items, &p).unwrap();

        assert_eq!(plan.bundle_count(), 1);
        assert_eq!(plan.direct_count(), 1);
        let bundle = plan
            .packages
            .iter()
            .find(|package| package.kind == PackageKind::Bundle)
            .unwrap();
        assert_members(bundle, &["a", "b"]);
        let direct = plan
            .packages
            .iter()
            .find(|package| package.kind == PackageKind::Direct)
            .unwrap();
        assert_members(direct, &["c"]);
    }

    #[test]
    fn item_over_entry_limit_is_direct_with_warning() {
        let items = [
            item_entries("bigdir", 10 * MIB, 1000),
            item_entries("small", 10 * MIB, 1),
        ];
        let p = params(Some(100 * MIB), Some(2 * GIB), Some(100 * MIB), Some(100));
        let plan = plan(&items, &p).unwrap();

        assert_eq!(plan.direct_count(), 2);
        assert_eq!(plan.bundle_count(), 0);
        assert_eq!(plan.warnings.len(), 1);
        assert_eq!(plan.warnings[0].item, "bigdir");
        assert_eq!(
            plan.warnings[0].reason,
            WarningReason::TooManyEntries {
                entry_count: 1000,
                limit: 100
            }
        );
        assert_members(&plan.packages[0], &["bigdir"]);
    }

    #[test]
    fn bundle_max_defaults_to_split_above() {
        let p = params(Some(500 * MIB), Some(500 * MIB), None, None);
        assert_eq!(p.effective_bundle_max(), Some(500 * MIB));

        let items = [item("a", 200 * MIB), item("b", 200 * MIB)];
        let plan = plan(&items, &p).unwrap();
        assert_eq!(plan.bundle_count(), 1);
        assert_eq!(plan.packages[0].input_bytes, 400 * MIB);
    }

    #[test]
    fn bundle_max_defaults_to_2g_when_split_disabled() {
        let p = params(Some(100 * MIB), None, None, None);
        assert_eq!(p.effective_bundle_max(), Some(DEFAULT_SPLIT_ABOVE));

        let items = [
            item("a", 100 * MIB),
            item("b", 100 * MIB),
            item("c", 100 * MIB),
        ];
        let plan = plan(&items, &p).unwrap();
        assert_eq!(plan.bundle_count(), 1);
        assert_eq!(plan.packages[0].input_bytes, 300 * MIB);
    }

    #[test]
    fn explicit_bundle_max_can_be_smaller_than_split() {
        let p = params(Some(100 * MIB), Some(2 * GIB), Some(300 * MIB), None);
        let items: Vec<StrategyItem> = (0..6).map(|i| item(&format!("i{i}"), 90 * MIB)).collect();
        let plan = plan(&items, &p).unwrap();
        assert_eq!(plan.bundle_count(), 2);
        assert!(
            plan.packages
                .iter()
                .all(|package| package.input_bytes <= 300 * MIB)
        );
    }

    #[test]
    fn split_disabled_keeps_large_items_direct() {
        let p = params(None, None, None, None);
        let items = [item("huge", 10 * GIB)];
        let plan = plan(&items, &p).unwrap();

        assert_eq!(plan.packages[0].kind, PackageKind::Direct);
        assert_eq!(plan.split_count(), 0);
    }

    #[test]
    fn merge_disabled_only_splits_large_items() {
        let p = params(None, Some(2 * GIB), None, None);
        let items = [item("small", MIB), item("large", 3 * GIB)];
        let plan = plan(&items, &p).unwrap();

        assert_eq!(plan.packages[0].kind, PackageKind::Direct);
        assert_eq!(plan.packages[1].kind, PackageKind::Split);
        assert_eq!(plan.bundle_count(), 0);
    }

    #[test]
    fn invalid_params_are_rejected() {
        let cases = [
            params(Some(200 * MIB), Some(100 * MIB), None, None),
            params(Some(100 * MIB), Some(100 * MIB), Some(200 * MIB), None),
            params(Some(100 * MIB), Some(2 * GIB), Some(50 * MIB), None),
            params(None, Some(0), None, None),
            params(None, None, Some(0), None),
            params(None, None, None, Some(0)),
        ];

        for (index, p) in cases.iter().enumerate() {
            let err = p.validate().unwrap_err();
            assert!(
                matches!(err, StrategyError::InvalidParams(_)),
                "case {index}: {err:?}"
            );
        }
    }

    #[test]
    fn duplicate_names_are_rejected_case_insensitively() {
        let items = [item("Novel", 1), item("novel", 2)];
        let err = plan(&items, &StrategyParams::disabled()).unwrap_err();
        assert_eq!(err, StrategyError::DuplicateName("novel".into()));
    }

    #[test]
    fn empty_input_yields_empty_plan() {
        let plan = plan(&[], &StrategyParams::default()).unwrap();
        assert!(plan.is_empty());
        assert_eq!(plan.package_count(), 0);
        assert!(plan.warnings.is_empty());
    }

    #[test]
    fn every_item_is_planned_exactly_once_and_bundles_respect_limits() {
        let items: Vec<StrategyItem> = (0..100)
            .map(|i| {
                let size = ((i * 37) % 90 + 1) as u64 * MIB;
                let entries = (i % 7) + 1;
                StrategyItem::new(format!("item-{i:03}"), size, entries)
            })
            .collect();
        let p = params(Some(100 * MIB), Some(2 * GIB), Some(300 * MIB), Some(20));
        let plan = plan(&items, &p).unwrap();

        let mut planned: Vec<&str> = plan
            .packages
            .iter()
            .flat_map(|package| package.members.iter().map(String::as_str))
            .collect();
        planned.sort_unstable();
        let mut expected: Vec<&str> = items.iter().map(|item| item.name.as_str()).collect();
        expected.sort_unstable();
        assert_eq!(planned, expected, "每个条目恰好出现一次");

        for package in &plan.packages {
            match package.kind {
                PackageKind::Direct | PackageKind::Split => {
                    assert_eq!(package.member_count(), 1);
                }
                PackageKind::Bundle => {
                    assert!(package.member_count() >= 2);
                    assert!(package.input_bytes <= 300 * MIB);
                    assert!(package.entry_count <= 20);
                }
            }
        }
    }

    #[test]
    fn plan_is_deterministic() {
        let items = [
            item("a", 60 * MIB),
            item("b", 40 * MIB),
            item("c", 30 * MIB),
        ];
        let p = StrategyParams::default();
        let first = plan(&items, &p).unwrap();
        let second = plan(&items, &p).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn zero_byte_items_can_share_a_bundle() {
        let items = [item("a", 0), item("b", 0), item("c", 0)];
        let p = params(Some(100 * MIB), Some(2 * GIB), Some(100 * MIB), None);
        let plan = plan(&items, &p).unwrap();

        assert_eq!(plan.bundle_count(), 1);
        assert_eq!(plan.packages[0].input_bytes, 0);
        assert_eq!(plan.packages[0].member_count(), 3);
    }

    #[test]
    fn bundle_can_be_exactly_bundle_max() {
        let items = [item("a", 60 * MIB), item("b", 40 * MIB)];
        let p = params(Some(100 * MIB), Some(2 * GIB), Some(100 * MIB), None);
        let plan = plan(&items, &p).unwrap();

        assert_eq!(plan.bundle_count(), 1);
        assert_eq!(plan.packages[0].input_bytes, 100 * MIB);
    }

    #[test]
    fn direct_and_split_packages_keep_input_order_bundles_follow() {
        let items = [
            item("A", 3 * GIB),
            item("b", 50 * MIB),
            item("C", 200 * MIB),
            item("d", 50 * MIB),
            item("e", 50 * MIB),
        ];
        let p = params(Some(100 * MIB), Some(2 * GIB), Some(200 * MIB), None);
        let plan = plan(&items, &p).unwrap();

        let kinds: Vec<PackageKind> = plan.packages.iter().map(|package| package.kind).collect();
        assert_eq!(
            kinds,
            [PackageKind::Split, PackageKind::Direct, PackageKind::Bundle]
        );
        assert_members(&plan.packages[0], &["A"]);
        assert_members(&plan.packages[1], &["C"]);
        assert_eq!(plan.packages[2].members.len(), 3);
    }

    #[test]
    fn entry_limit_disabled_allows_large_entry_counts() {
        let items = [
            item_entries("a", 50 * MIB, 1_000_000),
            item_entries("b", 50 * MIB, 1_000_000),
        ];
        let p = params(Some(100 * MIB), Some(2 * GIB), Some(200 * MIB), None);
        let plan = plan(&items, &p).unwrap();

        assert_eq!(plan.bundle_count(), 1);
        assert_eq!(plan.packages[0].entry_count, 2_000_000);
        assert!(plan.warnings.is_empty());
    }
}
