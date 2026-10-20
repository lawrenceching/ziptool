//! 仓库维护命令。
//!
//! 当前提供 `cargo bump-version <x.y.z>`:
//! - 同步根 `Cargo.toml` 的 `[workspace.package] version`
//! - 同步 `crates/tauri-app/tauri.conf.json` 的 `version`
//! - 同步 `Cargo.lock` 中 workspace 成员的版本
//! - 默认提交并创建 `v<x.y.z>` annotated tag; **不执行任何 git push**

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use semver::Version;

const USAGE: &str = "\
用法: cargo bump-version <x.y.z> [选项]

选项:
  --dry-run     只打印将做的修改, 不写文件/不提交/不打 tag
  --no-commit   只改文件, 不提交 (不能与 --no-tag 组合)
  --no-tag      提交但不创建 tag
  -h, --help    显示帮助

命令不会执行 git push; 推送由你手动完成。
";

#[derive(Debug, Default)]
struct BumpArgs {
    version: Option<String>,
    dry_run: bool,
    no_commit: bool,
    no_tag: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("bump-version") => {}
        Some("-h" | "--help") | None => {
            print!("{USAGE}");
            return Ok(());
        }
        Some(other) => return Err(format!("未知子命令 {other:?}\n\n{USAGE}")),
    }

    let args = parse_args(args.collect())?;
    let root = repo_root()?;
    let version = args
        .version
        .as_deref()
        .ok_or_else(|| format!("缺少版本号\n\n{USAGE}"))?;
    let version = normalize_version(version)?;
    let current = read_cargo_version(&root)?;
    if version <= current {
        return Err(format!("新版本 {version} 必须大于当前版本 {current}"));
    }

    let tag = format!("v{version}");
    if !args.dry_run && !git_clean(&root)? {
        return Err("工作区不干净; 请先提交或 stash 后再 bump".into());
    }
    if !args.dry_run && tag_exists(&root, &tag)? {
        return Err(format!("tag {tag} 已存在"));
    }
    if args.no_commit && !args.no_tag {
        return Err("--no-commit 不能与 --no-tag 组合".into());
    }

    let cargo_path = root.join("Cargo.toml");
    let tauri_path = root.join("crates/tauri-app/tauri.conf.json");
    let lock_path = root.join("Cargo.lock");

    let new_cargo = set_workspace_version(&read(&cargo_path)?, &version)?;
    let new_tauri = set_tauri_version(&read(&tauri_path)?, &version)?;
    let new_lock = set_lock_versions(&read(&lock_path)?, &root, &version)?;

    if args.dry_run {
        println!("will update {} -> {version}", cargo_path.display());
        println!("will update {} -> {version}", tauri_path.display());
        println!(
            "will update workspace package versions in {}",
            lock_path.display()
        );
        if !args.no_commit {
            println!("will commit: chore(release): {tag}");
        }
        if !args.no_tag {
            println!("will create tag: {tag}");
        }
        return Ok(());
    }

    write(&cargo_path, &new_cargo)?;
    write(&tauri_path, &new_tauri)?;
    write(&lock_path, &new_lock)?;

    run_git(
        &root,
        &[
            "add".into(),
            "Cargo.toml".into(),
            "Cargo.lock".into(),
            "crates/tauri-app/tauri.conf.json".into(),
        ],
        "git add",
    )?;

    if args.no_commit {
        println!("已更新版本到 {version}; 未提交 (--no-commit)");
        return Ok(());
    }

    run_git(
        &root,
        &[
            "commit".into(),
            "-m".into(),
            format!("chore(release): {tag}"),
        ],
        "git commit",
    )?;

    if !args.no_tag {
        run_git(
            &root,
            &[
                "tag".into(),
                "-a".into(),
                tag.clone(),
                "-m".into(),
                tag.clone(),
            ],
            "git tag",
        )?;
    }

    if args.no_tag {
        println!("已提交; 未创建 tag (--no-tag)");
    } else {
        println!("已提交并创建 tag {tag}");
        println!("下一步 (手动): git push origin HEAD && git push origin {tag}");
    }
    Ok(())
}

fn parse_args(args: Vec<String>) -> Result<BumpArgs, String> {
    let mut parsed = BumpArgs::default();
    for arg in args {
        match arg.as_str() {
            "--dry-run" => parsed.dry_run = true,
            "--no-commit" => parsed.no_commit = true,
            "--no-tag" => parsed.no_tag = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            s if s.starts_with('-') => return Err(format!("未知参数 {s:?}\n\n{USAGE}")),
            s => {
                if parsed.version.replace(s.to_string()).is_some() {
                    return Err(format!("只能指定一个版本号\n\n{USAGE}"));
                }
            }
        }
    }
    Ok(parsed)
}

fn normalize_version(raw: &str) -> Result<Version, String> {
    let raw = raw.trim().trim_start_matches('v');
    Version::parse(raw).map_err(|e| format!("非法版本号 {raw:?}: {e}"))
}

fn repo_root() -> Result<PathBuf, String> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| format!("执行 git rev-parse 失败: {e}"))?;
    if !output.status.success() {
        return Err("当前目录不在 git 仓库内".into());
    }
    let path = String::from_utf8(output.stdout)
        .map_err(|e| format!("git 仓库路径不是 UTF-8: {e}"))?
        .trim()
        .to_string();
    Ok(PathBuf::from(path))
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))
}

fn write(path: &Path, content: &str) -> Result<(), String> {
    fs::write(path, content).map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

fn read_cargo_version(root: &Path) -> Result<Version, String> {
    let text = read(&root.join("Cargo.toml"))?;
    let value = find_section_value(&text, "workspace.package", "version")
        .ok_or_else(|| "Cargo.toml 缺少 [workspace.package] version".to_string())?;
    Version::parse(&value).map_err(|e| format!("Cargo.toml 版本非法: {e}"))
}

fn set_workspace_version(text: &str, version: &Version) -> Result<String, String> {
    set_section_value(text, "workspace.package", "version", &version.to_string())
}

fn set_tauri_version(text: &str, version: &Version) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut replaced = false;
    for line in text.split_inclusive('\n') {
        if !replaced && let Some(key) = line.find("\"version\"") {
            let after_key = key + "\"version\"".len();
            let Some(colon_rel) = line[after_key..].find(':') else {
                return Err("tauri.conf.json 的 version 行格式异常".into());
            };
            let colon = after_key + colon_rel;
            let Some(start_rel) = line[colon + 1..].find('"') else {
                return Err("tauri.conf.json 的 version 行缺少引号".into());
            };
            let start = colon + 1 + start_rel + 1;
            let Some(end_rel) = line[start..].find('"') else {
                return Err("tauri.conf.json 的 version 行缺少结束引号".into());
            };
            let end = start + end_rel;
            out.push_str(&line[..start]);
            out.push_str(&version.to_string());
            out.push_str(&line[end..]);
            replaced = true;
        } else {
            out.push_str(line);
        }
    }
    if !replaced {
        return Err("tauri.conf.json 找不到 version".into());
    }
    Ok(out)
}

fn find_section_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section =
                trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) == Some(section);
            continue;
        }
        if in_section && trimmed.starts_with(key) {
            return quoted_value(trimmed);
        }
    }
    None
}

fn set_section_value(
    text: &str,
    section: &str,
    key: &str,
    version: &str,
) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut in_section = false;
    let mut replaced = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let section_trimmed = trimmed.trim_end_matches(['\r', '\n']);
        if section_trimmed.starts_with('[') {
            in_section = section_trimmed
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                == Some(section);
        } else if in_section && !replaced && trimmed.starts_with(key) {
            let indent = &line[..line.len() - line.trim_start().len()];
            let newline = if line.ends_with("\r\n") {
                "\r\n"
            } else if line.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            out.push_str(&format!("{indent}{key} = \"{version}\"{newline}"));
            replaced = true;
            continue;
        }
        out.push_str(line);
    }
    if !replaced {
        return Err(format!("找不到 [{section}] {key}"));
    }
    Ok(out)
}

fn quoted_value(line: &str) -> Option<String> {
    let start = line.find('"')? + 1;
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_string())
}

fn set_lock_versions(text: &str, root: &Path, version: &Version) -> Result<String, String> {
    let names = workspace_package_names(root)?;
    let mut out = String::with_capacity(text.len());
    let mut current_name: Option<String> = None;
    let mut in_package = false;

    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("[[package]]") {
            in_package = true;
            current_name = None;
        } else if in_package && trimmed.starts_with("name = ") {
            current_name = quoted_value(trimmed);
        } else if in_package
            && current_name.as_ref().is_some_and(|n| names.contains(n))
            && trimmed.starts_with("version = ")
        {
            let indent = &line[..line.len() - line.trim_start().len()];
            let newline = if line.ends_with("\r\n") {
                "\r\n"
            } else if line.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            out.push_str(&format!("{indent}version = \"{version}\"{newline}"));
            continue;
        }
        out.push_str(line);
    }
    Ok(out)
}

fn workspace_package_names(root: &Path) -> Result<Vec<String>, String> {
    let manifest = read(&root.join("Cargo.toml"))?;
    let members = workspace_members(&manifest)?;
    let mut names = Vec::new();
    for member in members {
        let path = root.join(member).join("Cargo.toml");
        let text = read(&path)?;
        let mut in_package = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                in_package = trimmed == "[package]";
            } else if in_package && trimmed.starts_with("name = ") {
                if let Some(name) = quoted_value(trimmed) {
                    names.push(name);
                }
                break;
            }
        }
    }
    if names.is_empty() {
        return Err("未找到任何 workspace 成员包".into());
    }
    Ok(names)
}

fn workspace_members(manifest: &str) -> Result<Vec<String>, String> {
    let mut members = Vec::new();
    let mut in_members = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if !in_members && trimmed.starts_with("members =") {
            in_members = true;
        }
        if in_members {
            members.extend(extract_quoted(trimmed));
            if trimmed.contains(']') {
                break;
            }
        }
    }
    if members.is_empty() {
        return Err("Cargo.toml 未找到 workspace members".into());
    }
    Ok(members)
}

fn extract_quoted(line: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('"') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('"') else {
            break;
        };
        result.push(after[..end].to_string());
        rest = &after[end + 1..];
    }
    result
}

fn git_clean(root: &Path) -> Result<bool, String> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("执行 git status 失败: {e}"))?;
    if !output.status.success() {
        return Err("git status 失败".into());
    }
    Ok(output.stdout.is_empty())
}

fn tag_exists(root: &Path, tag: &str) -> Result<bool, String> {
    let output = Command::new("git")
        .args(["rev-parse", "-q", "--verify", &format!("refs/tags/{tag}")])
        .current_dir(root)
        .output()
        .map_err(|e| format!("执行 git rev-parse 失败: {e}"))?;
    Ok(output.status.success())
}

fn run_git(root: &Path, args: &[String], what: &str) -> Result<(), String> {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .map_err(|e| format!("执行 {what} 失败: {e}"))?;
    if !status.success() {
        return Err(format!("{what} 失败 (exit {status})"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"[workspace]
members = ["crates/core", "xtask"]

[workspace.package]
version = "0.1.3"
edition = "2024"
"#;

    const TAURI: &str = r#"{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "ziptool",
  "version": "0.1.2",
  "identifier": "com.ziptool.desktop"
}
"#;

    const LOCK: &str = r#"[[package]]
name = "zip-tool-core"
version = "0.1.3"

[[package]]
name = "semver"
version = "1.0.0"
"#;

    #[test]
    fn updates_workspace_version_only() {
        let out = set_workspace_version(MANIFEST, &Version::new(0, 1, 4)).unwrap();
        assert!(out.contains("version = \"0.1.4\""));
        assert!(out.contains("edition = \"2024\""));
        assert!(!out.contains("\"0.1.3\""));
    }

    #[test]
    fn updates_tauri_version_only() {
        let out = set_tauri_version(TAURI, &Version::new(0, 1, 4)).unwrap();
        assert!(out.contains("\"version\": \"0.1.4\""));
        assert!(out.contains("\"productName\": \"ziptool\""));
    }

    #[test]
    fn updates_lock_only_workspace_packages() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let out = set_lock_versions(LOCK, root, &Version::new(0, 1, 4)).unwrap();
        assert!(out.contains("name = \"zip-tool-core\"\nversion = \"0.1.4\""));
        assert!(out.contains("name = \"semver\"\nversion = \"1.0.0\""));
    }

    #[test]
    fn rejects_non_increasing_version() {
        let current = Version::parse("0.1.3").unwrap();
        assert!(Version::parse("0.1.3").unwrap() <= current);
        assert!(Version::parse("0.1.2").unwrap() <= current);
    }
}
