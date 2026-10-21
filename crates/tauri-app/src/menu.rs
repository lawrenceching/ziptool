//! 应用菜单栏动作(菜单栏 UI 由前端自绘, 见 `_docs/menu.md`)。
//!
//! 前端通过 `menu_action(action)` 命令上报点击的菜单项, 壳层负责动作语义与白名单校验:
//! - `report-issue` → 用系统默认浏览器打开仓库地址(平台代码在 [`crate::desktop`])
//! - `quit` → `app.exit(0)`, 触发 `ExitRequested`, 复用壳层已有的取消任务 + 等待收尾逻辑
//!
//! 菜单文案/结构/可用性都在前端(`ui/index.html` + `ui/app.js`), 这里不感知 i18n。

use tauri::AppHandle;

use crate::desktop;

/// 「报告问题」打开的地址(仓库主页)。
pub const REPORT_ISSUE_URL: &str = "https://github.com/lawrenceching/ziptool";

/// 菜单动作白名单; 前端传字符串, 壳层解析后再执行, 未知动作一律报错。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuAction {
    ReportIssue,
    Quit,
}

impl MenuAction {
    fn parse(action: &str) -> Option<Self> {
        match action {
            "report-issue" => Some(Self::ReportIssue),
            "quit" => Some(Self::Quit),
            _ => None,
        }
    }
}

/// 执行菜单动作; 返回可读错误给前端状态栏展示(退出无返回值)。
pub fn dispatch(app: &AppHandle, action: &str) -> Result<(), String> {
    let action = MenuAction::parse(action).ok_or_else(|| format!("未知菜单动作: {action}"))?;
    match action {
        MenuAction::ReportIssue => {
            desktop::open_external_url(REPORT_ISSUE_URL).map_err(|e| format!("打开浏览器失败: {e}"))
        }
        MenuAction::Quit => {
            app.exit(0);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_issue_url_points_to_repository() {
        assert_eq!(REPORT_ISSUE_URL, "https://github.com/lawrenceching/ziptool");
    }

    #[test]
    fn menu_action_accepts_only_known_ids() {
        assert_eq!(
            MenuAction::parse("report-issue"),
            Some(MenuAction::ReportIssue)
        );
        assert_eq!(MenuAction::parse("quit"), Some(MenuAction::Quit));
        assert_eq!(MenuAction::parse("settings"), None);
        assert_eq!(MenuAction::parse(""), None);
        assert_eq!(MenuAction::parse("REPORT-ISSUE"), None);
    }
}
