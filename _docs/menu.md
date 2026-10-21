# 应用内菜单栏技术方案

> 状态: 已实现
> 范围: `crates/tauri-app`(`ui/index.html`、`ui/app.js`、`ui/app.css`、`tailwind/input.css`、`ui/locales/*.js`、`src/menu.rs`、`src/desktop.rs`)
> 参考: shadcn/ui Menubar(new-york-v4, MIT)——只移植视觉规格与交互约定, 不引入 React
> 相关: [filtering_rules.md](filtering_rules.md)(设置页)、[i18n.md](i18n.md)(多语言)、[ui-backend.md](ui-backend.md)(命令面)

## 1. 目标与非目标

### 目标

- 在应用窗口顶部提供**应用内自绘的菜单栏**, 视觉规格移植自 **shadcn/ui Menubar**, 用应用的 `slate` 色板映射其主题 token(白底、`text-sm`、圆角 + 浅阴影、`slate-100` accent、destructive 红色退出项)
- 菜单结构:
  - `应用` → `报告问题`、`退出`
  - `设置` → `设置…`(打开设置页)
- 「报告问题」用系统默认浏览器打开 <https://github.com/lawrenceching/ziptool>
- 「退出」与关闭窗口走同一条退出路径: 取消在跑任务、等待后端进程收尾后再退出
- 设置入口统一到菜单栏; 主界面 header 保留「压缩/解压」与「确认/取消」, 不再放齿轮按钮
- 菜单文案是页面静态文案, 跟随界面语言(简中 / 繁中港 / 繁中台 / English / Русский)即时切换

### 非目标

- 不使用系统原生菜单(Tauri `tauri::menu`): 原生菜单由 OS 绘制, 无法与应用视觉统一; 本方案改为 HTML 自绘
- 不直接使用 shadcn/ui 的 React 组件: 项目是静态 HTML + 原生 JS, 无打包器; 只移植其视觉/交互规格
- 不做托盘菜单 / 右键上下文菜单 / 全局快捷键
- 不做「最近打开」「编辑 / 帮助」等额外菜单
- 不改 CLI; 不新增前端依赖; 不修改 CSP

## 2. 结构

```text
应用 ─┬─ 报告问题
      └─ 退出
设置 ─┬─ 设置…
```

应用壳层按 **App = MenuBar / Content / StatusBar** 三段式重构:

```text
App
 ├─ MenuBar    (#app-menu-bar > #app-menu[role=menubar])
 ├─ Content    (#app-content: #view-main / #view-settings)
 └─ StatusBar  (#app-status-bar: 状态消息 + 语言菜单 + 版本号)
```

```html
<body id="app" class="h-screen flex flex-col …">
  <header id="app-menu-bar" class="app-menu-bar">            <!-- MenuBar -->
    <nav id="app-menu" class="app-menu" role="menubar">…</nav>
  </header>
  <main id="app-content" class="flex flex-col flex-1 min-h-0">  <!-- Content -->
    <div id="view-main">…</div>          <!-- 主界面 -->
    <div id="view-settings">…</div>      <!-- 设置页 -->
  </main>
  <footer id="app-status-bar">…</footer>                  <!-- StatusBar -->
</body>
```

- MenuBar 是全局第一段: 主界面与设置页都能看到、都能直接用
- Content 是唯一的内容容器, 两个视图用 `hidden` 切换, 主界面 DOM 不销毁;
  全文档只保留一个 `<main>`(`#app-content`), 主界面表格滚动区用 `<div>`, 避免嵌套 main
- StatusBar 保持原有状态消息 / 语言菜单 / 版本号
- 每个菜单都是「触发按钮 + 绝对定位下拉」: 触发按钮 `aria-haspopup="menu" aria-expanded aria-controls` + 纯文字;
  下拉 `role="menu"`, 菜单项 `role="menuitem"`(纯文字 + 右侧快捷键提示, 无图标), 分隔线 `role="separator"`
- 下拉默认 `hidden`; 打开时移除 `hidden`, 触发按钮按 `aria-expanded="true"` 高亮并播放入场动画

## 3. 视觉规范(与应用背景一致)

设计原则: **MenuBar 与下拉面板的背景和 App 背景完全一致(纯白), 不使用边框/浮层容器**;
交互反馈只用应用既有的浅色 hover 与同一套阴影, 避免出现"另一套配色"的观感。

| 元素 | 规格 | 实现 |
|---|---|---|
| MenuBar 区域 | 高 `2.5rem`, 透明背景(继承 App 白底), **无边框、无阴影** | `.app-menu-bar` |
| 顶层触发 | `text-sm font-medium`, `rounded-md`, `px-2`; hover / open 用 `slate-100` + `slate-900`(同应用按钮) | `.app-menu-trigger` |
| 下拉面板 | 纯白(同 App 背景), `rounded-md`, **无边框**, `shadow-lg`(同状态栏语言菜单), `min-width:12rem`, `p-1` | `.app-menu-panel` |
| 菜单项 | `text-sm`, `gap-2`, `px-2 py-1.5`, `rounded-md`; hover / focus 用 `slate-50`(同语言菜单项) | `.app-menu-item` |
| 快捷键提示 | 右对齐, `text-xs`, `slate-400`, `letter-spacing`(shadcn `MenubarShortcut`) | `.app-menu-shortcut` |
| 分隔线 | `1px` `slate-200`, 左右各 `-4px` | `.app-menu-separator` |
| 危险项 | `red-600` 文字, hover `red-50` | `.app-menu-item--destructive` |
| 不可用项 | `opacity:50%` + `pointer-events:none` | `:disabled` |

- 打开动画用 Web Animations API(120ms, `translateY(-2px) scale(.95) → 0/1` + 淡入), 对应 shadcn 的
  `animate-in fade-in-0 zoom-in-95 slide-in-from-top-2`, 无需额外动画插件
- 顶层入口是纯文字, 无展开箭头; **菜单项不显示图标**, 右侧显示平台快捷键:
  `退出` = `Ctrl+Q` / macOS `⌘Q`; `设置…` = `Ctrl+S` / macOS `⌘S`; 同时写入 `aria-keyshortcuts`
- 顶栏与下拉都没有边框
- 样式源码在 `tailwind/input.css` 的 `@layer components`(`.app-menu-bar` / `.app-menu*`),
  编译产物已提交到 `ui/app.css`; 改动样式后执行 `cd crates/tauri-app && npm run css`
- MenuBar 不改变 Content(主界面 header / 设置页 header)与 StatusBar 的布局

## 4. 交互与可访问性

| 操作 | 行为 |
|---|---|
| 点击触发按钮 | 展开 / 收起对应下拉; 打开另一个菜单时自动收起前一个 |
| 点击菜单项 | 先收起菜单, 再执行动作(设置页 / 报告问题 / 退出) |
| 已有菜单打开时 hover 另一个顶层项 | 直接切换菜单(Radix Menubar 行为) |
| 点击菜单外 / 窗口失焦 | 收起下拉 |
| `Escape` | 收起下拉, 焦点回到触发按钮; `preventDefault` 使同一次 Escape 不再关设置页 |
| `↓` / `↑`(触发按钮) | 打开菜单并聚焦第一个 / 最后一个项 |
| `←` / `→` | 顶层项之间移动焦点; 菜单打开时切换并打开邻居菜单 |
| `↓` / `↑` / `Home` / `End`(菜单内) | 在可用项间循环移动焦点 |
| 首字 typeahead | 600ms 内连续输入按前缀匹配菜单项(中英文均可) |
| 快捷键 `Ctrl+Q` / `⌘Q` | 退出应用(等价于「退出」菜单项) |
| 快捷键 `Ctrl+S` / `⌘S` | 打开设置页; 设置不可用时只拦截按键, 不打开(避免 WebView「保存网页」弹窗) |
| `Tab` | 收起菜单, 焦点继续按页面顺序移动 |
| 禁用项 | `disabled` 属性: 不可聚焦、不可点击; 键盘遍历自动跳过 |

- 快捷键按平台显示: Windows / Linux 用 `Ctrl+Q` / `Ctrl+S`, macOS 用 `⌘Q` / `⌘S`;
  同时写入对应菜单项的 `aria-keyshortcuts`(`Control+Q` / `Meta+Q` 等)

- roving tabindex: 菜单栏整体只占一个 Tab 位, 只有当前顶层项 `tabindex=0`, 其余 `-1`
- 只有当前打开的菜单有 `aria-expanded="true"`; 其余为 `false`
- `role="menubar"` + 顶层 `role="menuitem"` + `aria-haspopup="menu"` / `aria-controls`;
  下拉 `role="menu"`, 项 `role="menuitem"`, 分隔线 `role="separator"`
- 焦点态用 `:focus-visible` 蓝色描边(触发按钮)与 `focus:bg-accent` 背景(菜单项)
- 设置页关闭后焦点回到「设置」触发按钮(`#menu-settings-trigger`)

## 5. 设置入口与可用性

- 设置入口: 菜单栏 `设置 > 设置…`(`#menu-settings-open`)
- 可用性: `settingsReady && !state.locked && state.view !== "settings"`
  - 设置加载完成前禁用(避免用默认值展示/保存)
  - 任务运行中(压缩 / 解压)禁用; 扫描与方案计算期间可用, 不锁 UI
  - 已在设置页时禁用, 避免重复进入
- `syncMenuState()` 在设置加载前后、任务锁变化、视图切换时更新 `disabled`

## 6. 菜单动作(壳层命令)

菜单栏 UI 在前端, 壳层只负责执行动作。前端统一调用一个命令:

```js
invoke("menu_action", { action: "report-issue" | "quit" })
```

| action | 壳层行为 |
|---|---|
| `report-issue` | 用系统默认浏览器打开仓库地址(平台代码见 `src/desktop.rs`); 失败返回可读错误 |
| `quit` | `app.exit(0)` → `RunEvent::ExitRequested` → 取消 `CancelFlag`、等待任务收尾(上限 5s)后退出 |
| 其它 | 返回 `未知菜单动作: …`(白名单校验, 前端状态栏提示) |

- 平台启动器(不引入 opener 插件):

| 平台 | 命令 |
|---|---|
| Windows | `cmd /C start "" <url>`(`CREATE_NO_WINDOW`, 不闪控制台) |
| macOS | `open <url>` |
| Linux / 其他 Unix | `xdg-open <url>` |

- 只启动不等待: 后台线程 `wait()` 回收子进程, 避免僵尸进程
- 「报告问题」失败时前端状态栏显示 `status.reportIssueFailed`(`无法打开浏览器: {{error}}`)
- 浏览器预览(无 Tauri)时动作不执行, 设置入口仍可用(菜单栏是纯 HTML)

## 7. i18n

菜单文案是页面静态 DOM, 用 `data-i18n` 属性, 由 `i18n.applyDom()` 统一处理; 切换语言即时更新,
无需与 Rust 同步。

| Key | zh-CN |
|---|---|
| `menu.label` | 应用菜单(菜单栏 `aria-label`) |
| `menu.app` | 应用 |
| `menu.reportIssue` | 报告问题 |
| `menu.quit` | 退出 |
| `menu.settings` | 设置 |
| `menu.openSettings` | 设置… |
| `status.reportIssueFailed` | 无法打开浏览器: {{error}} |

- `settings.open` 已随 header 齿轮删除而移除
- 五种语言齐全; `i18n.test.mjs` 的 base key 集合 / 占位符测试会强制

## 8. 测试

### 8.1 Rust(`src/menu.rs` 单测)

- `REPORT_ISSUE_URL` 与需求地址一致
- `MenuAction::parse`: `report-issue` / `quit` 映射正确; `settings` / 空串 / 大小写不符返回 `None`

### 8.2 UI(`test/ui.test.mjs`)

- 菜单栏存在且默认收起; header 不再有 `#settings-open`
- 结构契约: `App = MenuBar / Content / StatusBar` 三段式、`role="menubar"`、顶层
  `role="menuitem"` / `aria-haspopup`、roving tabindex、无图标/无展开箭头、
  `app-menu-separator`、退出项 destructive 样式
- 快捷键提示: 非 macOS 显示 `Ctrl+Q` / `Ctrl+S`, macOS 显示 `⌘Q` / `⌘S`
- 快捷键行为: `Ctrl+Q` / `⌘Q` 调用 `menu_action{quit}`; `Ctrl+S` / `⌘S` 打开设置页;
  macOS 下 `Ctrl` 不触发
- `应用 > 报告问题` / `应用 > 退出` 分别以 `{ action }` 调用 `menu_action`, 选中后菜单收起
- `设置 > 设置…` 打开设置页; 设置页打开时该项禁用
- 切换语言后菜单文案(应用 / 报告问题 / 退出 / 设置 / 设置…)同步为英文
- 点击菜单外 / `Escape` 收起; `Escape` 后焦点回到触发按钮
- `←` / `→` 移动顶层焦点; 菜单打开时 `←` / `→` 切换菜单
- 打开一个菜单后 hover 另一个顶层项直接切换
- 首字 typeahead(如「退」→ 退出); `Tab` 收起菜单
- 扫描期间「设置…」可用(扫描只锁确认钮); 任务运行中禁用且点击无效, 任务结束后恢复
- 报告问题失败 → 状态栏提示
- 浏览器预览(无 Tauri): 菜单栏可用, 设置页可从菜单打开

### 8.3 i18n(`test/i18n.test.mjs`)

- 五语言 `menu.*` key 齐全; 静态 DOM 断言菜单栏 `data-i18n` 与启动文案

## 9. 文件清单

| 文件 | 变更 |
|---|---|
| `crates/tauri-app/ui/index.html` | App 三段式结构(`#app-menu-bar` / `#app-content` / `#app-status-bar`) + MenuBar 结构(纯文字 + 快捷键提示) + data-i18n; 删除 header 齿轮 |
| `crates/tauri-app/ui/app.js` | 菜单开关/←→/hover 切换/typeahead/动画/动作分发/快捷键(标签与全局快捷键)、`syncMenuState()`、焦点回收 |
| `crates/tauri-app/tailwind/input.css` | `@layer components`: `.app-menu-bar` / `.app-menu*`(扁平、与应用背景一致的菜单栏 + `.app-menu-shortcut`) |
| `crates/tauri-app/ui/app.css` | 重新编译的组件样式(与 input.css 对应) |
| `crates/tauri-app/ui/locales/*.js` | 五种语言 `menu.label` / `menu.*` / `status.reportIssueFailed`; 删除 `settings.open` |
| `crates/tauri-app/src/menu.rs` | 菜单动作白名单与分发(`report-issue` / `quit`) |
| `crates/tauri-app/src/desktop.rs` | 平台外部链接启动器 |
| `crates/tauri-app/src/lib.rs` | `menu_action` 命令注册; 移除原生菜单 setup/事件/`sync_menu` |
| `crates/tauri-app/test/ui.test.mjs` | 菜单栏用例(结构 / 动作 / 键盘 / hover / typeahead / 快捷键 / 禁用态) |
| `crates/tauri-app/test/i18n.test.mjs` | 菜单 key 与启动文案断言 |
| `README.md`、`_docs/filtering_rules.md`、`_docs/i18n.md`、`_docs/ui-backend.md` | 入口 / 命令 / i18n / 决策同步 |
