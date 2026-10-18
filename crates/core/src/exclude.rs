//! 排除规则:内建隐藏默认集 + 用户自定义模式,叠加(并集)生效。
//! 统一语法见 _docs/exclude-rules.md §3:
//! - 不含 `/`:任意深度的文件/目录名(basename, 如 `Thumbs.db`、`*.tmp`)
//! - 含 `/`:从条目根锚定匹配相对路径(`*`/`?` 不跨 `/`,`**` 只允许在结尾)
//! - 隐藏判定仅用 OS 原生口径:Unix = `.` 前缀约定,Windows = 文件系统隐藏属性
//! - 不变式:统计口径 ≡ 后端实压内容(任何护栏回退都以"加回统计"保持一致)

/// 排除规则:UI 恒定 `include_hidden = false`(被标行显示不勾选);
/// CLI 默认排除隐藏,`--include-hidden-files` 置 true 关闭
#[derive(Debug, Default, Clone)]
pub struct ExcludeRules {
    /// true = 不排除任何隐藏项(内建默认集关闭;此时不会产生 Hidden 标记)
    pub include_hidden: bool,
    /// 用户自定义模式,多条叠加;无否定语义
    pub patterns: Vec<String>,
}

/// 顶层条目的排除标记(仅顶层;树内文件不逐个上报)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExcludeReason {
    /// 内建隐藏判定命中
    Hidden,
    /// 用户模式命中(保留命中模式, 供 UI 展示)
    Pattern(String),
}

impl std::fmt::Display for ExcludeReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExcludeReason::Hidden => write!(f, "hidden"),
            ExcludeReason::Pattern(p) => write!(f, "pattern {p:?}"),
        }
    }
}

/// 被排除的顶层条目(CLI 提示用)
#[derive(Debug, Clone)]
pub struct ExcludedEntry {
    pub name: String,
    pub reason: ExcludeReason,
}

/// Windows 属性隐藏路径收集上限(条数):超限停止收集, 未收集项不计入排除
/// (统计与后端一致地"不排除"), 并发 warning
pub const EXCLUDE_PATH_LIMIT: usize = 1000;
/// 排除开关字符预算(与 EXCLUDE_PATH_LIMIT 双护栏, 收集端同步执行)
pub const EXCLUDE_ARG_BUDGET: usize = 24_000;

/// Unix 内建隐藏默认集的统一模式(任意深度 basename 以 `.` 开头)。
/// Windows 不用名字口径(`.` 开头不算隐藏), 不注入此模式。
pub const HIDDEN_UNIX_PATTERN: &str = ".*";

/// 校验用户模式(非法即报错, 宁停不错):
/// 空串 / 空路径段(首尾或连续 `/`)/ 单独 `**` / `**` 不在结尾, 都非法
pub fn validate_patterns(patterns: &[String]) -> Result<(), String> {
    for p in patterns {
        let p = p.replace('\\', "/");
        if p.is_empty() {
            return Err("排除模式不能为空".into());
        }
        if p.starts_with('/') || p.ends_with('/') || p.contains("//") {
            return Err(format!("排除模式 {p:?} 含空路径段"));
        }
        if p == "**" {
            return Err("排除模式不能只含 **".into());
        }
        if let Some(i) = p.find("**") {
            if i + 2 != p.len() {
                return Err(format!("排除模式 {p:?} 的 ** 只允许出现在结尾"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod validate_tests {
    use super::*;

    fn ok(pats: &[&str]) {
        let v: Vec<String> = pats.iter().map(|s| s.to_string()).collect();
        assert!(validate_patterns(&v).is_ok(), "{pats:?} 应合法");
    }
    fn bad(pats: &[&str]) {
        let v: Vec<String> = pats.iter().map(|s| s.to_string()).collect();
        assert!(validate_patterns(&v).is_err(), "{pats:?} 应非法");
    }

    #[test]
    fn accepts_legal_patterns() {
        ok(&["Thumbs.db", "*.tmp", "sub/*.log", "node_modules/**", "a?c", r"win\path"]);
    }

    #[test]
    fn rejects_illegal_patterns() {
        bad(&[""]);            // 空串
        bad(&["/abs"]);        // 首段空
        bad(&["dir/"]);        // 尾段空
        bad(&["a//b"]);        // 中段空
        bad(&["**"]);          // 单独 **
        bad(&["a/**/b"]);      // ** 不在结尾
        bad(&["a**b"]);        // ** 不在结尾
        // 反斜杠只做归一, 不是非法来源
        ok(&[r"a\b", r"a\b\**"]);
    }
}
