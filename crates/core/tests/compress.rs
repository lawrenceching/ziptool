//! compress_dir 集成测试:真实调用当前平台后端(本机为 7z/NanaZip)。

use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use zip_tool_core::{CancelFlag, Options, Progress, compress_dir};

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

    let report = compress_dir(
        &root.join("in"),
        &out,
        &Options::default(),
        &CancelFlag::new(),
        &|_| ControlFlow::Continue(()),
    )
    .unwrap();

    assert!(!report.cancelled);
    assert_eq!(report.entries.len(), 3);
    // dst = 原名 + ".zip"(追加而非替换扩展名, 保住 "C.jpg" 的完整原名且避免多点名冲突)
    assert_eq!(archive_names(&out), ["A.zip", "B.zip", "C.jpg.zip"]);
}

#[test]
fn completion_frames_stay_one_per_entry_amid_in_entry_percent_ticks() {
    let root = tmp_dir("progress");
    make_fixture(&root);
    let out = root.join("out");

    let events = std::cell::RefCell::new(Vec::new());
    compress_dir(
        &root.join("in"),
        &out,
        &Options::default(),
        &CancelFlag::new(),
        &|p: &Progress| {
            if p.phase == zip_tool_core::Phase::Compressing {
                events.borrow_mut().push((p.done, p.total, p.entry.clone()));
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();

    let events = events.into_inner();
    // 完成帧每条目恰好一帧(done = 序号+1);7z 后端夹带的条目内 tick 帧
    // (done = 当前条目的 0 基序号, 见 compress.md §7)额外出现, zip 后端没有
    let is_completion = |done: usize, entry: &str| {
        (done == 1 && entry == "A")
            || (done == 2 && entry == "B")
            || (done == 3 && entry == "C.jpg")
    };
    let completions: Vec<(usize, usize, String)> = events
        .iter()
        .filter(|(d, _, e)| is_completion(*d, e))
        .cloned()
        .collect();
    assert_eq!(
        completions.iter().map(|e| e.0).collect::<Vec<_>>(),
        [1, 2, 3],
        "done 应从 1 递增到 3: {events:?}"
    );
    assert!(completions.iter().all(|e| e.1 == 3), "total 恒为 3");
    let mut seen: Vec<&str> = completions.iter().map(|e| e.2.as_str()).collect();
    seen.sort();
    assert_eq!(seen, ["A", "B", "C.jpg"]);
    // 其余事件只能是条目内 tick 帧: done 为该条目的 0 基序号或完成值
    for (done, total, entry) in &events {
        let idx = match entry.as_str() {
            "A" => 0,
            "B" => 1,
            "C.jpg" => 2,
            other => panic!("未知条目 {other}: {events:?}"),
        };
        assert_eq!(*total, 3, "total 恒为 3: {events:?}");
        assert!(
            *done == idx || *done == idx + 1,
            "done 只能是当前条目 tick({idx})或完成帧({}): {done} {entry}",
            idx + 1
        );
    }
}

#[test]
fn stops_when_callback_breaks() {
    let root = tmp_dir("cancel");
    make_fixture(&root);
    let out = root.join("out");

    let report = compress_dir(
        &root.join("in"),
        &out,
        &Options::default(),
        &CancelFlag::new(),
        &|p| {
            if p.done >= 1 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        },
    )
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

    let report = compress_dir(&root.join("in"), &out, &opts, &CancelFlag::new(), &|_| {
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
        assert!(
            csv.contains(&format!(",{orig}\n")),
            "mapping.csv 应含 {orig}:\n{csv}"
        );
    }
}

#[test]
fn missing_or_empty_input_is_an_error() {
    let root = tmp_dir("err");

    assert!(
        compress_dir(
            &root.join("nope"),
            &root.join("o1"),
            &Options::default(),
            &CancelFlag::new(),
            &|_| { ControlFlow::Continue(()) }
        )
        .is_err()
    );

    fs::create_dir_all(root.join("empty")).unwrap();
    assert!(
        compress_dir(
            &root.join("empty"),
            &root.join("o2"),
            &Options::default(),
            &CancelFlag::new(),
            &|_| { ControlFlow::Continue(()) }
        )
        .is_err()
    );
}
