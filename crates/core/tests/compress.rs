//! compress_dir 集成测试:真实调用当前平台后端(本机为 7z/NanaZip)。

use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use zip_tool_core::{Options, Progress, compress_dir};

static TMP_N: AtomicU64 = AtomicU64::new(0);

fn tmp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "zip-tool-test-{tag}-{}-{}",
        std::process::id(),
        TMP_N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// in/A/ in/B/ C.jpg 三个顶层条目
fn make_fixture(root: &Path) {
    fs::create_dir_all(root.join("in/A")).unwrap();
    fs::create_dir_all(root.join("in/B")).unwrap();
    fs::write(root.join("in/A/a.txt"), "alpha").unwrap();
    fs::write(root.join("in/B/b.txt"), "beta").unwrap();
    fs::write(root.join("in/C.jpg"), "gamma").unwrap();
}

fn archive_names(out: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(out)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn compresses_each_entry_into_its_own_archive() {
    let root = tmp_dir("basic");
    make_fixture(&root);
    let out = root.join("out");

    let report =
        compress_dir(&root.join("in"), &out, &Options::default(), &|_| {
            ControlFlow::Continue(())
        })
        .unwrap();

    assert!(!report.cancelled);
    assert_eq!(report.entries.len(), 3);
    // dst = 原名 + ".zip"(追加而非替换扩展名, 保住 "C.jpg" 的完整原名且避免多点名冲突)
    assert_eq!(archive_names(&out), ["A.zip", "B.zip", "C.jpg.zip"]);
}

#[test]
fn reports_one_progress_event_per_completed_entry() {
    let root = tmp_dir("progress");
    make_fixture(&root);
    let out = root.join("out");

    let events = std::cell::RefCell::new(Vec::new());
    compress_dir(&root.join("in"), &out, &Options::default(), &|p: &Progress| {
        if p.phase == zip_tool_core::Phase::Compressing {
            events
                .borrow_mut()
                .push((p.done, p.total, p.entry.clone()));
        }
        ControlFlow::Continue(())
    })
    .unwrap();

    let events = events.into_inner();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events.iter().map(|e| e.0).collect::<Vec<_>>(),
        [1, 2, 3],
        "done 应从 1 递增到 3"
    );
    assert!(events.iter().all(|e| e.1 == 3), "total 恒为 3");
    let mut seen: Vec<&str> = events.iter().map(|e| e.2.as_str()).collect();
    seen.sort();
    assert_eq!(seen, ["A", "B", "C.jpg"]);
}

#[test]
fn stops_when_callback_breaks() {
    let root = tmp_dir("cancel");
    make_fixture(&root);
    let out = root.join("out");

    let report = compress_dir(&root.join("in"), &out, &Options::default(), &|p| {
        if p.done >= 1 {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .unwrap();

    assert!(report.cancelled);
    assert_eq!(report.entries.len(), 1, "只应有第一个条目完成");
    assert_eq!(archive_names(&out).len(), 1);
}

#[test]
fn random_names_hide_originals_and_mapping_csv_records_them() {
    let root = tmp_dir("random");
    make_fixture(&root);
    let out = root.join("out");
    let opts = Options {
        random_name: true,
        ..Default::default()
    };

    let report = compress_dir(&root.join("in"), &out, &opts, &|_| {
        ControlFlow::Continue(())
    })
    .unwrap();

    assert_eq!(report.entries.len(), 3);
    let originals: Vec<&str> = report.entries.iter().map(|e| e.original.as_str()).collect();
    for e in &report.entries {
        let fname = e.archive.file_name().unwrap().to_string_lossy();
        assert!(
            fname.starts_with("zt_") && fname.ends_with(".zip"),
            "包名应是 zt_*.zip,得到 {fname}"
        );
        for orig in &originals {
            assert!(!fname.contains(orig), "包名 {fname} 不应包含原名 {orig}");
        }
    }

    // 原名不出现在输出目录,映射记录在 mapping.csv
    let mut names = archive_names(&out);
    names.retain(|n| n != "mapping.csv");
    assert!(names.iter().all(|n| n.starts_with("zt_")), "{names:?}");
    assert_eq!(names.len(), 3);

    let csv = fs::read_to_string(out.join("mapping.csv")).unwrap();
    for orig in ["A", "B", "C.jpg"] {
        assert!(csv.contains(&format!(",{orig}\n")), "mapping.csv 应含 {orig}:\n{csv}");
    }
}

#[test]
fn missing_or_empty_input_is_an_error() {
    let root = tmp_dir("err");

    assert!(compress_dir(&root.join("nope"), &root.join("o1"), &Options::default(), &|_| {
        ControlFlow::Continue(())
    })
    .is_err());

    fs::create_dir_all(root.join("empty")).unwrap();
    assert!(compress_dir(&root.join("empty"), &root.join("o2"), &Options::default(), &|_| {
        ControlFlow::Continue(())
    })
    .is_err());
}
