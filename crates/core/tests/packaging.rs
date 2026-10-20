//! 包级压缩真实后端 roundtrip: bundle(多源) / split(分卷) / 7z listfile。

use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use zip_tool_core::strategy::StrategyParams;
use zip_tool_core::{
    CancelFlag, ExcludeRules, Options, compress_packages, extract, plan_packages, scan_dir,
};

static TMP_N: AtomicU64 = AtomicU64::new(0);

fn tmp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "zip-tool-pkg-{tag}-{}-{}",
        std::process::id(),
        TMP_N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn noop(_: &zip_tool_core::Progress) -> ControlFlow<()> {
    ControlFlow::Continue(())
}

fn scan(root: &Path) -> Vec<zip_tool_core::Entry> {
    scan_dir(&root.join("in"), &ExcludeRules::default(), &|_| {})
        .unwrap()
        .entries
}

fn params(merge: u64, split: u64) -> StrategyParams {
    StrategyParams {
        merge_below: Some(merge),
        split_above: Some(split),
        bundle_max: None,
        max_entries: None,
    }
}

#[test]
fn bundle_roundtrip_via_real_backend() {
    let root = tmp_dir("bundle");
    fs::create_dir_all(root.join("in/A")).unwrap();
    fs::create_dir_all(root.join("in/B")).unwrap();
    fs::write(root.join("in/A/a.txt"), "alpha").unwrap();
    fs::write(root.join("in/B/b.txt"), "beta").unwrap();
    fs::write(root.join("in/C.jpg"), "gamma").unwrap();

    let entries = scan(&root);
    let planned = plan_packages(
        &root.join("in"),
        &root.join("out"),
        &entries,
        &params(100 * 1024 * 1024, 2 * 1024 * 1024 * 1024),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(planned.len(), 1, "三个小条目应合并为一个 bundle");
    assert_eq!(planned[0].members.len(), 3);

    let report = compress_packages(
        &root.join("in"),
        &root.join("out"),
        &planned,
        &Options::default(),
        Some(2 * 1024 * 1024 * 1024),
        &CancelFlag::new(),
        &noop,
    )
    .unwrap();
    assert_eq!(report.packages.len(), 1);
    assert!(root.join("out/_bundle_0001.zip").exists());

    let restore = root.join("restore");
    extract(
        &root.join("out"),
        &restore,
        None,
        false,
        &CancelFlag::new(),
        &noop,
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(restore.join("A/a.txt")).unwrap(),
        "alpha"
    );
    assert_eq!(fs::read_to_string(restore.join("B/b.txt")).unwrap(), "beta");
    assert_eq!(fs::read_to_string(restore.join("C.jpg")).unwrap(), "gamma");

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn split_roundtrip_via_real_backend() {
    let root = tmp_dir("split");
    fs::create_dir_all(root.join("in")).unwrap();
    // 不可压缩数据, 保证 64k 分卷阈值真的切出多个卷
    let mut data = vec![0u8; 200 * 1024];
    let mut state = 0x1234_5678u32;
    for byte in &mut data {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *byte = (state >> 24) as u8;
    }
    fs::write(root.join("in/random.bin"), &data).unwrap();

    let entries = scan(&root);
    let planned = plan_packages(
        &root.join("in"),
        &root.join("out"),
        &entries,
        &params(64 * 1024, 64 * 1024),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].kind, zip_tool_core::strategy::PackageKind::Split);

    let report = compress_packages(
        &root.join("in"),
        &root.join("out"),
        &planned,
        &Options::default(),
        Some(64 * 1024),
        &CancelFlag::new(),
        &noop,
    )
    .unwrap();
    assert_eq!(report.packages.len(), 1);
    assert!(report.packages[0].archive.exists());

    let restore = root.join("restore");
    extract(
        &root.join("out"),
        &restore,
        None,
        false,
        &CancelFlag::new(),
        &noop,
    )
    .unwrap();
    assert_eq!(fs::read(restore.join("random.bin")).unwrap(), data);

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn sevenz_listfile_roundtrip_with_many_members() {
    let root = tmp_dir("listfile");
    for i in 0..65 {
        let dir = root.join(format!("in/d{i:02}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("f.txt"), format!("data-{i}")).unwrap();
    }

    let entries = scan(&root);
    let planned = plan_packages(
        &root.join("in"),
        &root.join("out"),
        &entries,
        &params(100 * 1024 * 1024, 2 * 1024 * 1024 * 1024),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].members.len(), 65);

    let report = compress_packages(
        &root.join("in"),
        &root.join("out"),
        &planned,
        &Options::default(),
        Some(2 * 1024 * 1024 * 1024),
        &CancelFlag::new(),
        &noop,
    )
    .unwrap();
    assert_eq!(report.packages.len(), 1);

    let restore = root.join("restore");
    extract(
        &root.join("out"),
        &restore,
        None,
        false,
        &CancelFlag::new(),
        &noop,
    )
    .unwrap();
    for i in 0..65 {
        assert_eq!(
            fs::read_to_string(restore.join(format!("d{i:02}/f.txt"))).unwrap(),
            format!("data-{i}")
        );
    }

    fs::remove_dir_all(&root).unwrap();
}
