//! 应用设置: `app_config_dir()/settings.json` 的读写与归一化。
//!
//! 磁盘结构见 _docs/filtering_rules.md §5:
//! - `general.shutdownAfterCompress`: 本期 UI 置灰, 字段保留给后续 ticket
//! - `filters.enabledPolicies`: 策略 id 数组; 缺失用默认, 显式 `[]` = 全关
//!
//! 读取失败一律回退默认值并给可读警告, 不阻止启动; 保存先写临时文件再改名。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use zip_tool_core::filter;

pub const SETTINGS_FILE: &str = "settings.json";
pub const SETTINGS_VERSION: u32 = 1;

/// 策略开关 DTO; 与前端 `filters` 字段一一对应。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct FilterSettingsDto {
    #[serde(default = "default_enabled_policies")]
    pub enabled_policies: Vec<String>,
}

/// 唯一默认列表来源(壳层包装 core 函数, 不在两处各写一份)
fn default_enabled_policies() -> Vec<String> {
    filter::default_enabled_policies()
}

impl Default for FilterSettingsDto {
    fn default() -> Self {
        Self {
            enabled_policies: default_enabled_policies(),
        }
    }
}

impl FilterSettingsDto {
    /// DTO → core 策略集合; 归一化与展开交给 core。
    pub fn to_core(&self) -> filter::FilterSettings {
        filter::FilterSettings {
            enabled_policies: self.enabled_policies.clone(),
        }
    }
}

/// 通用设置; 目前只有关机占位字段。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct GeneralSettingsDto {
    pub shutdown_after_compress: bool,
}

/// 命令 DTO(不暴露 version)
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct SettingsDto {
    pub general: GeneralSettingsDto,
    pub filters: FilterSettingsDto,
}

/// 磁盘结构 = SettingsDto + version
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SettingsFile {
    #[serde(default = "current_version")]
    pub version: u32,
    #[serde(default)]
    pub general: GeneralSettingsDto,
    #[serde(default)]
    pub filters: FilterSettingsDto,
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            general: GeneralSettingsDto::default(),
            filters: FilterSettingsDto::default(),
        }
    }
}

/// `load_settings` 的返回: canonical 设置 + 非致命警告
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SettingsLoadResult {
    pub settings: SettingsDto,
    pub warnings: Vec<String>,
}

fn current_version() -> u32 {
    SETTINGS_VERSION
}

pub fn settings_path(dir: &Path) -> PathBuf {
    dir.join(SETTINGS_FILE)
}

/// 归一化: 版本回到当前; 策略 id 去未知 / 去重 / 目录顺序。
fn normalize(file: &mut SettingsFile) {
    file.version = SETTINGS_VERSION;
    file.filters.enabled_policies =
        filter::normalize_enabled_policies(&file.filters.enabled_policies);
}

impl SettingsFile {
    pub fn to_dto(&self) -> SettingsDto {
        SettingsDto {
            general: self.general.clone(),
            filters: self.filters.clone(),
        }
    }
}

impl From<SettingsDto> for SettingsFile {
    fn from(dto: SettingsDto) -> Self {
        Self {
            version: SETTINGS_VERSION,
            general: dto.general,
            filters: dto.filters,
        }
    }
}

/// 读取设置; 纯函数, 便于单测。
///
/// - 文件不存在 → 默认值, 无警告
/// - 损坏 / IO 错误 → 默认值 + 警告
/// - 版本过新 → 默认值 + 警告(不尝试部分解析)
/// - 缺失字段 / 未知策略 id → 归一化
pub fn load_from(dir: &Path) -> (SettingsFile, Vec<String>) {
    let path = settings_path(dir);
    match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<SettingsFile>(&text) {
            Ok(mut file) => {
                if file.version > SETTINGS_VERSION {
                    return (
                        SettingsFile::default(),
                        vec![format!(
                            "设置版本过新(文件 {} > 当前 {}), 已使用默认值",
                            file.version, SETTINGS_VERSION
                        )],
                    );
                }
                normalize(&mut file);
                (file, Vec::new())
            }
            Err(e) => (
                SettingsFile::default(),
                vec![format!("设置文件损坏, 已使用默认值: {e}")],
            ),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => (SettingsFile::default(), Vec::new()),
        Err(e) => (SettingsFile::default(), vec![format!("读取设置失败: {e}")]),
    }
}

/// 保存设置; 先写 `settings.json.tmp` 再改名。
///
/// Windows 的 `rename` 不能覆盖已存在文件, 因此先移除目标(极小窗口内文件
/// 不存在, 崩溃最坏退回默认值)。
pub fn save_to(dir: &Path, file: &SettingsFile) -> io::Result<()> {
    let mut file = file.clone();
    normalize(&mut file);
    fs::create_dir_all(dir)?;

    let path = settings_path(dir);
    let mut bytes = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
    bytes.push(b'\n');
    let tmp = dir.join("settings.json.tmp");
    fs::write(&tmp, bytes)?;

    #[cfg(windows)]
    if path.exists()
        && let Err(e) = fs::remove_file(&path)
    {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// 保存前归一化 DTO 并转成磁盘结构; 供 `save_settings` 返回 canonical。
pub fn canonicalize(dto: SettingsDto) -> SettingsFile {
    let mut file = SettingsFile::from(dto);
    normalize(&mut file);
    file
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ziptool-settings-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn dto(enabled: &[&str], shutdown: bool) -> SettingsDto {
        SettingsDto {
            general: GeneralSettingsDto {
                shutdown_after_compress: shutdown,
            },
            filters: FilterSettingsDto {
                enabled_policies: enabled.iter().map(|s| (*s).to_string()).collect(),
            },
        }
    }

    #[test]
    fn defaults_enable_both_policies_and_shutdown_off() {
        let file = SettingsFile::default();
        assert_eq!(file.version, SETTINGS_VERSION);
        assert!(!file.general.shutdown_after_compress);
        assert_eq!(
            file.filters.enabled_policies,
            filter::default_enabled_policies()
        );
    }

    #[test]
    fn missing_file_returns_defaults_without_warning() {
        let dir = temp_dir("missing");
        let (file, warnings) = load_from(&dir);
        assert_eq!(file, SettingsFile::default());
        assert!(warnings.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupt_json_returns_defaults_with_warning() {
        let dir = temp_dir("corrupt");
        fs::write(settings_path(&dir), "{not json").unwrap();
        let (file, warnings) = load_from(&dir);
        assert_eq!(file, SettingsFile::default());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("损坏"), "{warnings:?}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn newer_version_returns_defaults_with_warning() {
        let dir = temp_dir("newer");
        fs::write(settings_path(&dir), r#"{"version":2,"filters":{}}"#).unwrap();
        let (file, warnings) = load_from(&dir);
        assert_eq!(file, SettingsFile::default());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("版本过新"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_fields_use_defaults_but_explicit_empty_is_preserved() {
        let dir = temp_dir("fields");
        fs::write(settings_path(&dir), "{}").unwrap();
        let (file, warnings) = load_from(&dir);
        assert!(warnings.is_empty());
        assert_eq!(
            file.filters.enabled_policies,
            filter::default_enabled_policies()
        );

        fs::write(settings_path(&dir), r#"{"filters":{"enabledPolicies":[]}}"#).unwrap();
        let (file, _) = load_from(&dir);
        assert!(file.filters.enabled_policies.is_empty(), "显式空 = 全关");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unknown_and_duplicate_ids_are_normalized() {
        let dir = temp_dir("normalize");
        fs::write(
            settings_path(&dir),
            r#"{"filters":{"enabledPolicies":["lang-temp","unknown","os-hidden","lang-temp"]}}"#,
        )
        .unwrap();
        let (file, _) = load_from(&dir);
        assert_eq!(
            file.filters.enabled_policies,
            vec!["os-hidden".to_string(), "lang-temp".to_string()]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_then_load_roundtrip_and_overwrite() {
        let dir = temp_dir("roundtrip");
        let first = SettingsFile::from(dto(&["os-hidden"], true));
        save_to(&dir, &first).unwrap();
        let (loaded, warnings) = load_from(&dir);
        assert!(warnings.is_empty());
        assert_eq!(loaded, first);

        let second = SettingsFile::from(dto(&[], false));
        save_to(&dir, &second).unwrap();
        let (loaded, _) = load_from(&dir);
        assert_eq!(loaded, second);
        assert!(!dir.join("settings.json.tmp").exists(), "临时文件应清理");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn canonicalize_drops_unknown_and_keeps_shutdown_value() {
        let file = canonicalize(dto(&["unknown", "lang-temp"], true));
        assert_eq!(file.version, SETTINGS_VERSION);
        assert!(file.general.shutdown_after_compress);
        assert_eq!(file.filters.enabled_policies, vec!["lang-temp".to_string()]);
    }

    #[test]
    fn dto_serde_uses_camel_case_and_defaults() {
        let parsed: SettingsDto = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, SettingsDto::default());
        let json = serde_json::to_string(&dto(&["os-hidden"], true)).unwrap();
        assert!(json.contains("enabledPolicies"), "{json}");
        assert!(json.contains("shutdownAfterCompress"), "{json}");
    }
}
