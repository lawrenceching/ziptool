//! 文件排除策略: 把 UI 的策略开关展开为既有的 [`ExcludeRules`]。
//!
//! 单一管线原则(_docs/filtering_rules.md §4):
//! - 策略只产出模式 / `include_hidden`, 之后仍走 `scan_dir` / `exclusion_args` 的既有路径;
//! - 策略 id 是稳定契约, UI 只传 id, 不复制模式名单;
//! - 未知 id 忽略(前向兼容), 空集合 = 两种策略都关。

use std::collections::HashSet;

use crate::exclude::{ExcludeRules, validate_patterns};

/// 操作系统隐藏文件策略 id
pub const POLICY_OS_HIDDEN: &str = "os-hidden";
/// 编程语言临时文件策略 id
pub const POLICY_LANG_TEMP: &str = "lang-temp";

/// 系统元数据名单(定义在 `exclude`; UI 策略与 CLI 默认共用)。
pub use crate::exclude::OS_METADATA_PATTERNS;

/// 编程语言 / 构建工具临时物(`lang-temp`)。
///
/// 只收明确由工具生成、可再生且名字足够专有的项; 通用名(`target`、`build`、
/// `dist`、`vendor`、`cache`)刻意不收, 见 _docs/filtering_rules.md §4.5。
pub const LANG_TEMP_PATTERNS: &[&str] = &[
    // JavaScript / TypeScript
    "node_modules",
    ".pnpm-store",
    ".yarn/cache",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".parcel-cache",
    ".eslintcache",
    ".stylelintcache",
    // Python
    "__pycache__",
    "*.pyc",
    "*.pyo",
    "*.pyd",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".tox",
    ".nox",
    ".hypothesis",
    "*.egg-info",
    ".eggs",
    ".venv",
    "venv",
    // Java / Kotlin
    ".gradle",
    ".kotlin",
    "*.class",
    // C / C++
    "*.o",
    "cmake-build-*",
    // Elixir
    "_build",
    // Dart / Flutter
    ".dart_tool",
    // Swift
    ".build",
    // 通用临时文件
    "*.tmp",
    "*.temp",
    "*.swp",
    "*.swo",
    "*~",
];

/// 策略开关集合; 只存 id, 不存展开后的模式。
///
/// 目录更新对已有设置自动生效: 设置文件里只有 `enabledPolicies`, 模式在扫描 /
/// 压缩时由本模块展开。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterSettings {
    pub enabled_policies: Vec<String>,
}

impl Default for FilterSettings {
    fn default() -> Self {
        Self {
            enabled_policies: default_enabled_policies(),
        }
    }
}

/// 默认启用的策略(D-1 / D-2 已确认: 两种都开)
pub fn default_enabled_policies() -> Vec<String> {
    vec![POLICY_OS_HIDDEN.to_string(), POLICY_LANG_TEMP.to_string()]
}

/// 已知策略 id, 顺序即展开顺序
pub fn is_known_policy(id: &str) -> bool {
    id == POLICY_OS_HIDDEN || id == POLICY_LANG_TEMP
}

/// 去未知 id、去重、按目录顺序归一化; 空输入保持空(代表全部关闭)。
pub fn normalize_enabled_policies(ids: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for known in [POLICY_OS_HIDDEN, POLICY_LANG_TEMP] {
        if ids.iter().any(|id| id == known) {
            out.push(known.to_string());
        }
    }
    out
}

/// 策略 + 自定义模式 → 既有 `ExcludeRules`。
///
/// 系统元数据默认集由 `include_hidden` 在 core 的 `effective_patterns` 中统一
/// 加入(UI/CLI 不重复拼接); 这里只产出 `lang-temp` 模式与自定义模式。
/// 顺序保证 `ScanContext.rules_eq` 可比较; 去重键是斜杠归一后的精确值。
pub fn effective_rules(
    settings: &FilterSettings,
    custom: &[String],
) -> Result<ExcludeRules, String> {
    let enabled = normalize_enabled_policies(&settings.enabled_policies);
    let os_hidden = enabled.iter().any(|id| id == POLICY_OS_HIDDEN);
    let lang_temp = enabled.iter().any(|id| id == POLICY_LANG_TEMP);

    let mut patterns: Vec<String> = Vec::new();
    if lang_temp {
        patterns.extend(LANG_TEMP_PATTERNS.iter().map(|p| (*p).to_string()));
    }
    patterns.extend(custom.iter().cloned());

    // 去重: 斜杠归一后精确比较, 保留首次出现的原始写法。
    let mut seen: HashSet<String> = HashSet::new();
    patterns.retain(|p| seen.insert(p.replace('\\', "/")));

    validate_patterns(&patterns)?;
    Ok(ExcludeRules {
        // false = core 在 effective_patterns 中加入系统元数据默认集
        include_hidden: !os_hidden,
        patterns,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exclude::matches_patterns;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_string()).collect()
    }

    /// 内建模式是 `&[&str]`, 现有匹配函数收 `&[String]`; 测试里转换一次。
    fn lang_matches(name: &str) -> Option<String> {
        let patterns: Vec<String> = LANG_TEMP_PATTERNS
            .iter()
            .map(|p| (*p).to_string())
            .collect();
        matches_patterns(&patterns, name)
    }

    fn os_matches(name: &str) -> Option<String> {
        let patterns: Vec<String> = OS_METADATA_PATTERNS
            .iter()
            .map(|p| (*p).to_string())
            .collect();
        matches_patterns(&patterns, name)
    }

    #[test]
    fn os_metadata_keeps_user_dotfiles() {
        for name in [
            ".env",
            ".git",
            ".git/config",
            ".gitignore",
            ".ssh",
            ".config",
            ".bashrc",
            "important.txt",
        ] {
            assert!(os_matches(name).is_none(), "不应排除用户文件 {name}");
        }
        for name in [
            "Thumbs.db",
            "desktop.ini",
            ".DS_Store",
            ".Trashes",
            ".Spotlight-V100",
            "lost+found",
            ".Trash-1000",
            "System Volume Information",
        ] {
            assert!(os_matches(name).is_some(), "应排除系统元数据 {name}");
        }
    }

    #[test]
    fn known_policy_ids_are_stable() {
        assert_eq!(POLICY_OS_HIDDEN, "os-hidden");
        assert_eq!(POLICY_LANG_TEMP, "lang-temp");
        assert!(is_known_policy(POLICY_OS_HIDDEN));
        assert!(is_known_policy(POLICY_LANG_TEMP));
        assert!(!is_known_policy("unknown"));
    }

    #[test]
    fn default_enables_both_policies() {
        assert_eq!(
            FilterSettings::default().enabled_policies,
            s(&[POLICY_OS_HIDDEN, POLICY_LANG_TEMP])
        );
        assert_eq!(
            default_enabled_policies(),
            s(&[POLICY_OS_HIDDEN, POLICY_LANG_TEMP])
        );
    }

    #[test]
    fn normalize_drops_unknown_dedupes_and_orders() {
        assert_eq!(
            normalize_enabled_policies(&s(&["lang-temp", "unknown", "os-hidden", "lang-temp"])),
            s(&[POLICY_OS_HIDDEN, POLICY_LANG_TEMP])
        );
        assert!(normalize_enabled_policies(&[]).is_empty(), "空 = 全关");
        assert!(normalize_enabled_policies(&s(&["unknown"])).is_empty());
    }

    #[test]
    fn every_builtin_pattern_is_valid() {
        let all: Vec<String> = OS_METADATA_PATTERNS
            .iter()
            .chain(LANG_TEMP_PATTERNS.iter())
            .map(|p| (*p).to_string())
            .collect();
        validate_patterns(&all).expect("内建模式必须全部合法");
    }

    #[test]
    fn builds_rules_for_each_policy_combination() {
        let both = effective_rules(&FilterSettings::default(), &[]).unwrap();
        assert!(!both.include_hidden, "os-hidden 开 = core 加系统元数据");
        assert!(!both.patterns.contains(&"Thumbs.db".to_string()));
        assert!(both.patterns.contains(&"node_modules".to_string()));

        let only_os = effective_rules(
            &FilterSettings {
                enabled_policies: s(&[POLICY_OS_HIDDEN]),
            },
            &[],
        )
        .unwrap();
        assert!(!only_os.include_hidden);
        assert!(only_os.patterns.is_empty(), "os-hidden 只由 core 加元数据");

        let only_lang = effective_rules(
            &FilterSettings {
                enabled_policies: s(&[POLICY_LANG_TEMP]),
            },
            &[],
        )
        .unwrap();
        assert!(only_lang.include_hidden, "os-hidden 关 = 不加系统元数据");
        assert!(!only_lang.patterns.contains(&"Thumbs.db".to_string()));
        assert!(only_lang.patterns.contains(&"node_modules".to_string()));

        let none = effective_rules(
            &FilterSettings {
                enabled_policies: vec![],
            },
            &[],
        )
        .unwrap();
        assert!(none.include_hidden);
        assert!(none.patterns.is_empty());
    }

    #[test]
    fn custom_patterns_are_appended_and_deduped() {
        let rules = effective_rules(
            &FilterSettings::default(),
            &s(&["node_modules", "*.bak", "*.bak", "sub\\a.log"]),
        )
        .unwrap();
        assert_eq!(
            rules
                .patterns
                .iter()
                .filter(|p| p.as_str() == "node_modules")
                .count(),
            1,
            "与内建重复只保留一份"
        );
        assert_eq!(
            rules
                .patterns
                .iter()
                .filter(|p| p.as_str() == "*.bak")
                .count(),
            1
        );
        assert!(
            rules.patterns.contains(&"sub\\a.log".to_string()),
            "保留原始写法"
        );
        assert_eq!(rules.patterns.last().unwrap(), "sub\\a.log", "自定义在最后");
    }

    #[test]
    fn invalid_custom_pattern_fails() {
        assert!(effective_rules(&FilterSettings::default(), &s(&["a/**b"])).is_err());
        assert!(effective_rules(&FilterSettings::default(), &s(&[""])).is_err());
    }

    #[test]
    fn lang_patterns_never_hit_vcs_or_generic_names() {
        let generic = [
            ".git",
            ".git/config",
            "data",
            "src",
            "target",
            "build",
            "dist",
            "vendor",
            "cache",
            "main.rs",
            "Cargo.toml",
        ];
        for name in generic {
            assert!(lang_matches(name).is_none(), "语言临时模式不应命中 {name}");
        }
    }

    #[test]
    fn lang_patterns_hit_representative_cache_paths() {
        let hits = [
            "A/node_modules",
            "A/B/node_modules",
            "node_modules",
            "sub/__pycache__",
            "a.pyc",
            "sub/x.class",
            "venv",
            "project/.venv",
            ".yarn/cache",
            "cmake-build-debug",
            "foo.tmp",
            "foo.swp",
            "foo~",
        ];
        for name in hits {
            assert!(lang_matches(name).is_some(), "应命中 {name}");
        }
    }
}
