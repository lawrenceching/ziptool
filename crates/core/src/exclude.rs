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

/// 单段通配匹配:`*` 任意串、`?` 单字符(都不跨 `/`),其余字面。
/// 两指针 + `*` 回溯(经典 glob 单段算法)。
fn match_seg(pat: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let mut star = usize::MAX; // 最近一个 '*' 的位置
    let mut mark = 0usize; // 回溯时 text 的推进点
    while t < text.len() {
        if p < pat.len() && (pat[p] == b'?' || pat[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pat.len() && pat[p] == b'*' {
            star = p;
            mark = t;
            p += 1;
        } else if star != usize::MAX {
            p = star + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == b'*' {
        p += 1;
    }
    p == pat.len()
}

/// 统一模式匹配相对路径(分隔符 `/`,输入侧 `\` 容忍归一):
/// - 不含 `/`:与末段(basename)比 → 任意深度
/// - 含 `/`:整条锚定,逐段比较(`*`/`?` 不跨段);`prefix/**` = prefix 自身或其下
pub fn matches_pattern(pattern: &str, rel: &str) -> bool {
    let pat = pattern.replace('\\', "/");
    let rel = rel.replace('\\', "/");
    if !pat.contains('/') {
        return rel
            .rsplit('/')
            .next()
            .is_some_and(|base| match_seg(pat.as_bytes(), base.as_bytes()));
    }
    if let Some(prefix) = pat.strip_suffix("/**") {
        let pfx = prefix.trim_end_matches('/');
        return rel == pfx
            || (rel.len() > pfx.len()
                && rel.starts_with(pfx)
                && rel.as_bytes()[pfx.len()] == b'/');
    }
    let pv: Vec<&str> = pat.split('/').collect();
    let rv: Vec<&str> = rel.split('/').collect();
    pv.len() == rv.len() && pv.iter().zip(&rv).all(|(p, r)| match_seg(p.as_bytes(), r.as_bytes()))
}

/// 命中任一模式即排除, 返回首个命中的模式(供 `ExcludeReason::Pattern`)
pub fn matches_patterns(patterns: &[String], rel: &str) -> Option<String> {
    patterns.iter().find(|p| matches_pattern(p, rel)).cloned()
}

/// 内建隐藏判定(名字口径):仅 Unix 原生约定(`.` 前缀);
/// Windows 无名字口径(`.` 开头可能是用户自命名), 恒 false, 走属性判定
pub fn is_hidden_name(name: &str) -> bool {
    !cfg!(windows) && name.starts_with('.')
}

/// Windows 文件系统隐藏属性(stable std API: `MetadataExt::file_attributes`);
/// 非 Windows 恒 false
pub fn is_hidden_attr(md: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        md.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
    }
    #[cfg(not(windows))]
    {
        let _ = md;
        false
    }
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

#[cfg(test)]
mod match_tests {
    use super::*;

    #[test]
    fn basename_pattern_matches_any_depth_without_crossing() {
        // 不含 / 的模式 = 任意深度的 basename(只与末段比)
        assert!(matches_pattern("Thumbs.db", "Thumbs.db"));
        assert!(matches_pattern("Thumbs.db", "a/b/Thumbs.db"));
        assert!(!matches_pattern("Thumbs.db", "a/b/Thumbs"));
        assert!(matches_pattern("*.log", "a.log"));
        assert!(matches_pattern("*.log", "sub/b.log"));
        assert!(matches_pattern("*.log", "sub/deep/c.log"));
        assert!(!matches_pattern("*.log", "a.log2"));
        // ? 单字符
        assert!(matches_pattern("a?c", "abc"));
        assert!(!matches_pattern("a?c", "ac"));
        assert!(!matches_pattern("a?c", "abbc"));
        // 大小写敏感
        assert!(!matches_pattern("Thumbs.db", "thumbs.db"));
    }

    #[test]
    fn anchored_pattern_matches_path_from_entry_root() {
        // 含 / 的模式 = 从条目根锚定的整条相对路径
        assert!(matches_pattern("sub/*.log", "sub/a.log"));
        assert!(!matches_pattern("sub/*.log", "sub/x/y.log"), "* 不跨 /");
        assert!(!matches_pattern("sub/a.log", "other/sub/a.log"), "锚定不任意深度");
        assert!(!matches_pattern("sub/a.log", "sub/a.log2"));
        // prefix/** = prefix 自身或其下
        assert!(matches_pattern("node_modules/**", "node_modules"));
        assert!(matches_pattern("node_modules/**", "node_modules/a/b.js"));
        assert!(!matches_pattern("node_modules/**", "node_modulesx"));
        assert!(!matches_pattern("node_modules/**", "a/node_modules/b"));
        // 输入侧容忍反斜杠(归一为 /)
        assert!(matches_pattern("sub\\a.log", "sub/a.log"));
        assert!(matches_pattern("sub/*.log", "sub\\a.log"));
    }

    #[test]
    fn matches_patterns_returns_first_hit() {
        let pats: Vec<String> = vec!["*.tmp".into(), "cache".into()];
        assert_eq!(matches_patterns(&pats, "x/y/tmp.tmp"), Some("*.tmp".into()));
        assert_eq!(matches_patterns(&pats, "cache"), Some("cache".into()));
        assert_eq!(matches_patterns(&pats, "keep.txt"), None);
        assert_eq!(matches_patterns(&[], "anything"), None);
    }

    #[test]
    fn hidden_name_uses_unix_convention_only() {
        if cfg!(windows) {
            // Windows 无名字口径: . 开头不算隐藏(可能是用户自命名)
            assert!(!is_hidden_name(".DS_Store"));
            assert!(!is_hidden_name(".git"));
        } else {
            assert!(is_hidden_name(".DS_Store"));
            assert!(is_hidden_name(".."));
            assert!(is_hidden_name(".git"));
            assert!(!is_hidden_name("git"));
            assert!(!is_hidden_name("a.txt"));
        }
    }
}
