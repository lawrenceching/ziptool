//! extract 集成测试:压缩 → 解压 round-trip(明文 / 分卷 / 密码)。

use std::fs;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use zip_tool_core::{Options, Progress, compress_dir, extract};

static TMP_N: AtomicU64 = AtomicU64::new(0);

fn tmp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "zip-tool-ext-{tag}-{}-{}",
        std::process::id(),
        TMP_N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn noop(_: &Progress) -> ControlFlow<()> {
    ControlFlow::Continue(())
}

/// LCG 生成不可压缩的伪随机字节(压缩比≈1,分卷测试稳定)
fn pseudo_random_bytes(n: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(n);
    let mut state: u32 = 0x1234_5678;
    for _ in 0..n {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        v.push((state >> 24) as u8);
    }
    v
}

#[test]
fn round_trip_plain() {
    let root = tmp_dir("plain");
    fs::create_dir_all(root.join("in/Novel")).unwrap();
    fs::write(root.join("in/Novel/a.txt"), "alpha").unwrap();

    let report = compress_dir(
        &root.join("in"),
        &root.join("out"),
        &Options::default(),
        &|_| ControlFlow::Continue(()),
    )
    .unwrap();

    let dest = root.join("dest");
    let report = extract(&report.entries[0].archive, &dest, None, false, &noop).unwrap();
    assert_eq!(report.groups.len(), 1, "单包输入恰好一组");
    assert_eq!(report.groups[0].files, 1);

    assert_eq!(
        fs::read_to_string(dest.join("Novel/a.txt")).unwrap(),
        "alpha"
    );
}

#[test]
fn round_trip_split_volumes() {
    let root = tmp_dir("split");
    fs::create_dir_all(root.join("in/Big")).unwrap();
    fs::write(root.join("in/Big/b.bin"), pseudo_random_bytes(200 * 1024)).unwrap();

    let opts = Options {
        split_size: Some(64 * 1024),
        ..Default::default()
    };
    let report = compress_dir(&root.join("in"), &root.join("out"), &opts, &|_| {
        ControlFlow::Continue(())
    })
    .unwrap();

    let archive = &report.entries[0].archive;
    assert!(
        archive.extension().is_some_and(|e| e == "001"),
        "分卷时应以首卷 .zip.001 为主文件,得到 {}",
        archive.display()
    );
    let volumes = fs::read_dir(root.join("out")).unwrap().count();
    assert!(volumes >= 2, "200KB/64KB 应产生多个分卷,实际 {volumes}");

    let dest = root.join("dest");
    extract(archive, &dest, None, false, &noop).unwrap();
    assert_eq!(
        fs::read(dest.join("Big/b.bin")).unwrap(),
        pseudo_random_bytes(200 * 1024)
    );
}

#[test]
fn round_trip_with_password() {
    let root = tmp_dir("pw");
    fs::create_dir_all(root.join("in/Secret")).unwrap();
    fs::write(root.join("in/Secret/s.txt"), "top secret").unwrap();

    let opts = Options {
        password: Some("pw123".into()),
        ..Default::default()
    };
    let report = compress_dir(&root.join("in"), &root.join("out"), &opts, &|_| {
        ControlFlow::Continue(())
    })
    .unwrap();

    let dest = root.join("dest");
    extract(
        &report.entries[0].archive,
        &dest,
        Some("pw123"),
        false,
        &noop,
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(dest.join("Secret/s.txt")).unwrap(),
        "top secret"
    );

    // 错误密码必须失败
    let bad = root.join("dest-bad");
    assert!(
        extract(
            &report.entries[0].archive,
            &bad,
            Some("wrong"),
            false,
            &noop
        )
        .is_err(),
        "错误密码不应解压成功"
    );
    // 不给密码也必须失败
    assert!(
        extract(
            &report.entries[0].archive,
            &root.join("dest-none"),
            None,
            false,
            &noop
        )
        .is_err()
    );
}
