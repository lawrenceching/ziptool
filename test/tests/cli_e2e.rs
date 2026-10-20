//! 端到端测试:在系统临时目录构造测试夹具 → 调用 ziptool CLI 二进制压缩 →
//! 用系统 zip/7z 工具解压(不使用本工具)→ 校验压缩包生成且内容逐字节一致。
//!
//! 运行: cargo test -p zip-tool-e2e
//! 依赖: Windows 需 7z(NanaZip 兼容);Linux 需 zip / unzip 和 p7zip 的 7z
//! (zip -s 分卷用例的解压端是 7z, unzip 不支持分卷包; --format 7z 用例同样需要)。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::{env, fs};

/// workspace 根目录(本 crate 的上一级)
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// 构建并返回被测 CLI 二进制路径。
/// 用独立 target-dir(target/e2e):cargo test 期间持有主 target 的锁,
/// 嵌套 cargo 用同一 target 会死锁;独立目录也有增量缓存, 只在首次慢。
fn tool_binary() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let ws = workspace_root();
        let target = ws.join("target").join("e2e");
        let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let out = Command::new(&cargo)
            .args(["build", "-p", "zip-tool-cli"])
            .arg("--target-dir")
            .arg(&target)
            .current_dir(&ws)
            .output()
            .expect("无法启动 cargo build");
        assert!(
            out.status.success(),
            "构建 zip-tool-cli 失败:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let bin = target.join("debug").join(if cfg!(windows) {
            "ziptool.exe"
        } else {
            "ziptool"
        });
        assert!(bin.exists(), "未找到 CLI 二进制: {}", bin.display());
        bin
    })
}

/// 系统临时目录下的唯一夹具, 测试结束(含断言失败)自动清理
struct Fixture {
    root: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // ZIP_TOOL_E2E_KEEP=1 时保留现场, 便于排查失败
        if env::var_os("ZIP_TOOL_E2E_KEEP").is_none() {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

/// in/Novel/book.txt(2.5MB), in/media/clip.bin(3MB), in/photo.jpg(200KB)
fn fixture(tag: &str) -> Fixture {
    let root = env::temp_dir().join(format!("zip-tool-e2e-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("in/Novel")).unwrap();
    fs::create_dir_all(root.join("in/media")).unwrap();
    fill(&root.join("in/Novel/book.txt"), 2_500_000, 7);
    fill(&root.join("in/media/clip.bin"), 3_000_000, 11);
    fill(&root.join("in/photo.jpg"), 200_000, 13);
    Fixture { root }
}

/// 确定性且不可压内容(xorshift32, 周期远大于文件长度):解压后逐字节比对,
/// 且 --split 分卷测试不会因数据高度可压而缩成单卷。
fn fill(path: &Path, len: usize, seed: u8) {
    let mut x = (seed as u32) | 1;
    let data: Vec<u8> = (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x & 0xff) as u8
        })
        .collect();
    fs::write(path, data).unwrap();
}

struct Run {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn run_tool(args: &[&str]) -> Run {
    run_with(|cmd| {
        cmd.args(args);
    })
}

/// 自定义子进程构造(如清空环境变量做负向测试)
fn run_with(f: impl FnOnce(&mut Command)) -> Run {
    let mut cmd = Command::new(tool_binary());
    cmd.stdin(Stdio::null());
    f(&mut cmd);
    let out = cmd.output().expect("无法启动 ziptool");
    Run {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn list_dir(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn assert_file_eq(a: &Path, b: &Path) {
    let da = fs::read(a).unwrap_or_else(|e| panic!("读取 {} 失败: {e}", a.display()));
    let db = fs::read(b).unwrap_or_else(|e| panic!("读取 {} 失败: {e}", b.display()));
    assert_eq!(
        da.len(),
        db.len(),
        "{} 与 {} 大小不一致",
        a.display(),
        b.display()
    );
    assert!(da == db, "{} 与 {} 内容不一致", a.display(), b.display());
}

/// .7z 系(.7z / .7z.001)
fn is_7z_archive(archive: &Path) -> bool {
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .to_lowercase();
    name.ends_with(".7z") || name.ends_with(".7z.001")
}

/// 是否交系统 7z 解压: Windows 一律 7z(.zip 与 .zip.001 都能直接解); .7z 系
/// 两平台都交 7z(unzip 不认识 7z); unix 的 zip -s 分卷组(有 .z01 兄弟卷)也必须
/// 交 7z —— unzip 不支持分卷包, 且实测 Ubuntu 24.04 的 zip 3.0 上 `zip -s 0`
/// 合并 / `zip -FF` / cat 拼接均产出坏包(中央目录偏移按盘相对), 仅 7z 直读正确。
fn use_7z(archive: &Path) -> bool {
    if cfg!(windows) || is_7z_archive(archive) {
        return true;
    }
    let stem = archive
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    archive.with_file_name(format!("{stem}.z01")).exists()
}

/// 用系统工具解压(本测试的核心约束: 不使用本工具)。
/// 7z 系(`use_7z`)走 7z; 其余(unix 的 .zip 整包)走 unzip。
fn extract_archive(archive: &Path, dest: &Path, password: Option<&str>) {
    fs::create_dir_all(dest).unwrap();
    if use_7z(archive) {
        let mut cmd = Command::new("7z");
        cmd.arg("x").arg("-y");
        if let Some(pw) = password {
            cmd.arg(format!("-p{pw}"));
        }
        cmd.arg(format!("-o{}", dest.display()));
        cmd.arg("--").arg(archive);
        let out = cmd.stdin(Stdio::null()).output().unwrap();
        assert!(
            out.status.success(),
            "7z 解压 {} 失败:\n{}",
            archive.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    } else {
        let mut cmd = Command::new("unzip");
        cmd.arg("-o");
        if let Some(pw) = password {
            cmd.arg("-P").arg(pw);
        }
        cmd.arg(archive).arg("-d").arg(dest);
        let out = cmd.stdin(Stdio::null()).output().unwrap();
        assert!(
            out.status.success(),
            "unzip 解压 {} 失败:\n{}",
            archive.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// 断言错误密码无法解压(加密确实生效)
fn extract_with_wrong_password_must_fail(archive: &Path, dest: &Path) {
    fs::create_dir_all(dest).unwrap();
    let out = if use_7z(archive) {
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

/// zt_ + 16 位 hex + (.zip | .7z | .zNN | .zip.NNN | .7z.NNN)
fn is_random_artifact(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("zt_") else {
        return false;
    };
    let bytes = rest.as_bytes();
    if bytes.len() < 16 || !bytes[..16].iter().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }
    match &rest[16..] {
        ".zip" | ".7z" => true,
        s if s.starts_with(".zip.") => s.len() == 8 && s[5..].bytes().all(|b| b.is_ascii_digit()),
        s if s.starts_with(".7z.") => s.len() == 7 && s[4..].bytes().all(|b| b.is_ascii_digit()),
        s if s.starts_with(".z") => s.len() == 4 && s[3..].bytes().all(|b| b.is_ascii_digit()),
        _ => false,
    }
}

#[test]
fn compress_then_external_extract_roundtrip() {
    let fx = fixture("plain");
    let (inp, out) = (fx.root.join("in"), fx.root.join("out"));

    let r = run_tool(&[
        "compress",
        "-i",
        inp.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ]);
    assert!(r.ok, "压缩失败:\n{}", r.stderr);

    // 每个顶层条目一个包: 原名 + ".zip"(追加, 不替换扩展名)
    assert_eq!(list_dir(&out), ["Novel.zip", "media.zip", "photo.jpg.zip"]);

    // 系统工具逐包解压到同一 restore 目录(包内布局为 <条目名>/...,
    // 7z/unzip 的 -o/-d 是"解到该目录之下", 不能再传子目录)
    let restore = fx.root.join("restore");
    for name in ["Novel", "media", "photo.jpg"] {
        extract_archive(&out.join(format!("{name}.zip")), &restore, None);
    }
    assert_file_eq(
        &inp.join("Novel/book.txt"),
        &fx.root.join("restore/Novel/book.txt"),
    );
    assert_file_eq(
        &inp.join("media/clip.bin"),
        &fx.root.join("restore/media/clip.bin"),
    );
    assert_file_eq(&inp.join("photo.jpg"), &fx.root.join("restore/photo.jpg"));
}

#[test]
fn split_password_random_roundtrip() {
    let fx = fixture("split");
    let (inp, out) = (fx.root.join("in"), fx.root.join("out"));

    let r = run_tool(&[
        "compress",
        "-i",
        inp.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--split",
        "1m",
        "-p",
        "secret123",
        "--random-name",
    ]);
    assert!(r.ok, "压缩失败:\n{}", r.stderr);

    // mapping.csv 存在, 其余产物都是随机名, 不泄露原名
    let names = list_dir(&out);
    assert!(names.contains(&"mapping.csv".to_string()), "{names:?}");
    let archives: Vec<&String> = names.iter().filter(|n| *n != "mapping.csv").collect();
    assert!(!archives.is_empty(), "应有压缩包产物: {names:?}");
    for n in &archives {
        assert!(is_random_artifact(n), "产物应是随机名: {n}");
        for orig in ["Novel", "media", "photo"] {
            assert!(!n.contains(orig), "包名 {n} 不应包含原名");
        }
    }

    // 按 mapping.csv 还原对应关系
    let csv = fs::read_to_string(out.join("mapping.csv")).unwrap();
    let rows: Vec<(String, String)> = csv
        .lines()
        .skip(1)
        .map(|l| {
            let (a, o) = l.split_once(',').unwrap();
            (a.to_string(), o.to_string())
        })
        .collect();
    assert_eq!(rows.len(), 3, "mapping.csv 应有 3 行: {csv:?}");

    // clip.bin 3MB / 1m: 至少分成 2 卷
    let media_primary = rows.iter().find(|(_, o)| o == "media").unwrap().0.clone();
    let id: String = media_primary.chars().take("zt_".len() + 16).collect();
    let volume_count = names.iter().filter(|n| n.starts_with(&id)).count();
    assert!(
        volume_count >= 2,
        "media 应被分包, 实际 {volume_count} 卷: {names:?}"
    );

    // 错误密码必须被拒(加密生效), 正确密码解压并逐字节比对
    let (first, _) = &rows[0];
    extract_with_wrong_password_must_fail(&out.join(first), &fx.root.join("badpw"));
    let restore = fx.root.join("restore");
    for (archive, _) in &rows {
        extract_archive(&out.join(archive), &restore, Some("secret123"));
    }
    assert_file_eq(
        &inp.join("Novel/book.txt"),
        &fx.root.join("restore/Novel/book.txt"),
    );
    assert_file_eq(
        &inp.join("media/clip.bin"),
        &fx.root.join("restore/media/clip.bin"),
    );
    assert_file_eq(&inp.join("photo.jpg"), &fx.root.join("restore/photo.jpg"));
}

#[test]
fn doctor_reports_backend_tool_availability() {
    let program = if cfg!(windows) { "7z" } else { "zip" };

    // 正常环境: 定位到工具且试运行成功
    let r = run_tool(&["doctor"]);
    assert!(r.ok, "doctor 应成功:\n{}", r.stderr);
    assert!(
        r.stdout.contains(&format!("[ok] {program}")),
        "stdout: {}",
        r.stdout
    );

    // PATH 清空: 定位不到工具, 应 FAIL 且退出码非 0
    let bad = run_with(|cmd| {
        cmd.args(["doctor"]).env_remove("PATH");
    });
    assert!(!bad.ok, "PATH 清空后 doctor 应失败, stdout: {}", bad.stdout);
    assert!(
        bad.stdout.contains(&format!("[FAIL] {program}")),
        "stdout: {}",
        bad.stdout
    );
}

/// 断言 restore 树与 in 原树逐字节一致(三个顶层条目)
fn assert_tree_matches_input(fx: &Fixture) {
    let inp = fx.root.join("in");
    let restore = fx.root.join("restore");
    assert_file_eq(&inp.join("Novel/book.txt"), &restore.join("Novel/book.txt"));
    assert_file_eq(&inp.join("media/clip.bin"), &restore.join("media/clip.bin"));
    assert_file_eq(&inp.join("photo.jpg"), &restore.join("photo.jpg"));
}

fn compress_fixture(fx: &Fixture, extra: &[&str]) {
    let r = run_tool(
        &[
            "compress",
            "-i",
            fx.root.join("in").to_str().unwrap(),
            "-o",
            fx.root.join("out").to_str().unwrap(),
        ]
        .iter()
        .chain(extra)
        .copied()
        .collect::<Vec<_>>(),
    );
    assert!(r.ok, "压缩失败:\n{}", r.stderr);
}

#[test]
fn extract_single_package_roundtrip() {
    let fx = fixture("ext-single");
    compress_fixture(&fx, &[]);

    let restore = fx.root.join("restore");
    let r = run_tool(&[
        "extract",
        "-i",
        fx.root.join("out/Novel.zip").to_str().unwrap(),
        "-o",
        restore.to_str().unwrap(),
    ]);
    assert!(r.ok, "extract 失败:\n{}\n{}", r.stderr, r.stdout);
    assert!(
        r.stdout.contains("Novel.zip"),
        "报告应含入口卷名: {}",
        r.stdout
    );
    assert_file_eq(
        &fx.root.join("in/Novel/book.txt"),
        &restore.join("Novel/book.txt"),
    );
}

#[test]
fn extract_batch_directory_restores_input_tree() {
    let fx = fixture("ext-batch");
    compress_fixture(&fx, &[]);

    // out 目录本身作为输入(含 3 个包) → restore 恢复 in 的整棵树
    let r = run_tool(&[
        "extract",
        "-i",
        fx.root.join("out").to_str().unwrap(),
        "-o",
        fx.root.join("restore").to_str().unwrap(),
    ]);
    assert!(r.ok, "批量 extract 失败:\n{}\n{}", r.stderr, r.stdout);
    assert_tree_matches_input(&fx);
}

#[test]
fn extract_rejects_existing_files_then_force_overrides() {
    let fx = fixture("ext-force");
    compress_fixture(&fx, &[]);

    let restore = fx.root.join("restore");
    let ok = run_tool(&[
        "extract",
        "-i",
        fx.root.join("out").to_str().unwrap(),
        "-o",
        restore.to_str().unwrap(),
    ]);
    assert!(ok.ok);

    // 第二次解压: 目标已有文件, 默认拒绝
    let again = run_tool(&[
        "extract",
        "-i",
        fx.root.join("out").to_str().unwrap(),
        "-o",
        restore.to_str().unwrap(),
    ]);
    assert!(!again.ok, "重复解压应失败(防旧半成品冒充结果)");
    assert!(
        again.stderr.contains("use --force"),
        "错误应提示 --force: {}",
        again.stderr
    );

    // --force 显式覆盖后成功
    let forced = run_tool(&[
        "extract",
        "-i",
        fx.root.join("out").to_str().unwrap(),
        "-o",
        restore.to_str().unwrap(),
        "--force",
    ]);
    assert!(
        forced.ok,
        "--force 解压失败:\n{}\n{}",
        forced.stderr, forced.stdout
    );
    assert_tree_matches_input(&fx);
}

#[test]
fn extract_wrong_password_fails() {
    let fx = fixture("ext-pw");
    compress_fixture(&fx, &["-p", "secret123"]);

    let r = run_tool(&[
        "extract",
        "-i",
        fx.root.join("out").to_str().unwrap(),
        "-o",
        fx.root.join("restore").to_str().unwrap(),
        "-p",
        "WRONG",
    ]);
    assert!(!r.ok, "错误密码不应解压成功:\n{}", r.stdout);
}

#[test]
fn extract_member_volume_resolves_entry_volume() {
    let fx = fixture("ext-member");
    compress_fixture(&fx, &["--split", "1m"]);

    // 找 media 的一个成员卷(Windows: .zip.002;Linux: .z01), 用它解压应自动归一到入口卷
    let member = if cfg!(windows) {
        "out/media.zip.002"
    } else {
        "out/media.z01"
    };
    let member = fx.root.join(member);
    assert!(member.exists(), "media 应存在成员卷: {}", member.display());

    let restore = fx.root.join("restore");
    let r = run_tool(&[
        "extract",
        "-i",
        member.to_str().unwrap(),
        "-o",
        restore.to_str().unwrap(),
    ]);
    assert!(r.ok, "成员卷 extract 失败:\n{}\n{}", r.stderr, r.stdout);
    assert_file_eq(
        &fx.root.join("in/media/clip.bin"),
        &restore.join("media/clip.bin"),
    );
}

#[test]
fn extract_batch_random_names_restores_original_tree() {
    let fx = fixture("ext-random");
    compress_fixture(&fx, &["--split", "1m", "-p", "secret123", "--random-name"]);

    // 旗舰场景: 包名全随机且无原名信息(除 mapping.csv), 解 out 整目录应还原原树
    let r = run_tool(&[
        "extract",
        "-i",
        fx.root.join("out").to_str().unwrap(),
        "-o",
        fx.root.join("restore").to_str().unwrap(),
        "-p",
        "secret123",
    ]);
    assert!(r.ok, "随机名批量 extract 失败:\n{}\n{}", r.stderr, r.stdout);
    assert_tree_matches_input(&fx);

    // 报告里应给出 mapping.csv 记录的原条目名
    for orig in ["Novel", "media", "photo.jpg"] {
        assert!(
            r.stdout.contains(&format!("[{orig}]")),
            "报告应含原条目名 {orig}: {}",
            r.stdout
        );
    }
}

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

/// 排除规则 e2e: 预埋系统元数据 / 用户点文件, 验证
/// 1) 默认排除系统元数据(Windows 隐藏属性 + Thumbs.db/.DS_Store 名单; `.` 前缀不排)
/// 2) --include-hidden-files 关闭  3) --exclude 模式与默认元数据叠加
#[test]
fn compress_exclusion_rules_matrix() {
    let fx = fixture("excl");
    let (inp, out1) = (fx.root.join("in"), fx.root.join("out1"));
    let (out2, out3) = (fx.root.join("out2"), fx.root.join("out3"));

    // 垃圾文件(*.tmp 模式可命中) + 系统元数据; 相对名供还原侧断言
    fill(&inp.join("Novel/notes.tmp"), 1_000, 3);
    let hidden_top = ".DS_Store".to_string();
    let hidden_inner = "Novel/Thumbs.db".to_string();
    fill(inp.join(&hidden_top).as_path(), 1_000, 7);
    fill(&inp.join(&hidden_inner), 1_000, 5);
    if cfg!(windows) {
        // Windows 上同时置隐藏属性, 覆盖属性路径; 元数据名单在非 Windows 上生效
        for p in [inp.join(&hidden_top), inp.join(&hidden_inner)] {
            let st = Command::new("attrib")
                .args(["+H", p.to_str().unwrap()])
                .status()
                .unwrap();
            assert!(st.success());
        }
    }
    // 用户自建点文件 / 用户手动隐藏的普通文件都不是系统元数据, 默认必须保留
    let user_dot = ".env".to_string();
    fill(&inp.join(&user_dot), 1_000, 9);
    fill(&inp.join("Novel/.gitignore"), 1_000, 11);
    let user_hidden = "user-hidden.txt".to_string();
    fill(&inp.join(&user_hidden), 1_000, 13);
    if cfg!(windows) {
        let st = Command::new("attrib")
            .args(["+H", inp.join(&user_hidden).to_str().unwrap()])
            .status()
            .unwrap();
        assert!(st.success());
    }

    // 1) 默认: 系统元数据被排除, 用户点文件照常 roundtrip
    let r = run_tool(&[
        "compress",
        "-i",
        inp.to_str().unwrap(),
        "-o",
        out1.to_str().unwrap(),
    ]);
    assert!(r.ok, "压缩失败:\n{}", r.stderr);
    assert!(
        r.stderr.contains("excluded"),
        "应打印排除提示: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("excluded (hidden): .env"),
        ".env 不应按隐藏排除: {}",
        r.stderr
    );
    let restore1 = fx.root.join("restore1");
    for name in ["Novel", "media", "photo.jpg", ".env", "user-hidden.txt"] {
        extract_archive(&out1.join(format!("{name}.zip")), &restore1, None);
    }
    assert_file_eq(
        &inp.join("Novel/book.txt"),
        &restore1.join("Novel/book.txt"),
    );
    assert!(
        !restore1.join(&hidden_top).exists() && !restore1.join(&hidden_inner).exists(),
        "系统元数据不应进包(顶层与树内)"
    );
    assert_file_eq(&inp.join(&user_dot), &restore1.join(&user_dot));
    assert_file_eq(&inp.join(&user_hidden), &restore1.join(&user_hidden));
    assert_file_eq(
        &inp.join("Novel/.gitignore"),
        &restore1.join("Novel/.gitignore"),
    );
    assert!(
        restore1.join("Novel/notes.tmp").exists(),
        "未命中模式的垃圾文件默认保留"
    );

    // 2) --include-hidden-files: 系统元数据回来
    let r = run_tool(&[
        "compress",
        "-i",
        inp.to_str().unwrap(),
        "-o",
        out2.to_str().unwrap(),
        "--include-hidden-files",
    ]);
    assert!(r.ok, "压缩失败:\n{}", r.stderr);
    assert!(
        !r.stderr.contains("excluded"),
        "关闭隐藏排除后不应有排除提示: {}",
        r.stderr
    );
    let restore2 = fx.root.join("restore2");
    // .DS_Store 在 include 模式下是独立顶层条目(产出自己的包), 一并还原
    for name in [
        "Novel",
        "media",
        "photo.jpg",
        ".DS_Store",
        ".env",
        "user-hidden.txt",
    ] {
        extract_archive(&out2.join(format!("{name}.zip")), &restore2, None);
    }
    assert_file_eq(&inp.join(&hidden_top), &restore2.join(&hidden_top));
    assert_file_eq(&inp.join(&hidden_inner), &restore2.join(&hidden_inner));
    assert_file_eq(&inp.join(&user_dot), &restore2.join(&user_dot));
    assert_file_eq(&inp.join(&user_hidden), &restore2.join(&user_hidden));

    // 3) --exclude 与隐藏集叠加: 树内 *.tmp + 顶层 photo.jpg
    let r = run_tool(&[
        "compress",
        "-i",
        inp.to_str().unwrap(),
        "-o",
        out3.to_str().unwrap(),
        "--exclude",
        "*.tmp",
        "--exclude",
        "photo.jpg",
    ]);
    assert!(r.ok, "压缩失败:\n{}", r.stderr);
    assert!(
        r.stderr.contains("excluded (pattern"),
        "应打印模式排除提示: {}",
        r.stderr
    );
    assert!(
        !out3.join("photo.jpg.zip").exists(),
        "顶层命中模式的条目不产出包"
    );
    let restore3 = fx.root.join("restore3");
    for name in ["Novel", "media"] {
        extract_archive(&out3.join(format!("{name}.zip")), &restore3, None);
    }
    assert!(
        !restore3.join("Novel/notes.tmp").exists(),
        "*.tmp 应被模式排除"
    );
    assert_file_eq(
        &inp.join("media/clip.bin"),
        &restore3.join("media/clip.bin"),
    );
    assert_file_eq(
        &inp.join("Novel/book.txt"),
        &restore3.join("Novel/book.txt"),
    );
}

#[test]
fn compress_emits_progress_stream_frames() {
    // 两个 64MB 不可压条目: 7z 压缩各约 1s+, 进度帧约 10Hz → 几十帧;
    // 无进度流时整批只有个位数 \r(每条目 scan+compress 各一帧)。阈值 12 留足余量。
    // 固定 --format 7z: 进度流是 7z 系特性, Info-ZIP(zip 后端)无机器可读进度,
    // 非 Windows 默认格式走 zip 会只有个位数帧; 且 LZMA2 对随机数据更慢,
    // 帧数余量更大(实测 ~60 帧/11.5s, 默认格式 ~25 帧)。
    let fx = fixture("progress");
    fill(&fx.root.join("in/Novel/book.txt"), 64 * 1024 * 1024, 7);
    fill(&fx.root.join("in/media/clip.bin"), 64 * 1024 * 1024, 8);
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
    assert!(r.ok, "压缩失败:\n{}", r.stderr);
    let frames = r.stderr.matches('\r').count();
    assert!(
        frames >= 12,
        "进度帧过少({frames}), 进度流可能未生效:\n{}",
        r.stderr
    );
}
