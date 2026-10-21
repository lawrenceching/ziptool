# WebView2 启动失败：HRESULT 0x800700AA（2026-10-21）

> 向 dsh 反馈的报告草稿见 [`dsh-workspace-write-tauri-report.md`](./dsh-workspace-write-tauri-report.md)。

## 现象

```text
cargo run -p zip-tool-tauri
→ Failed to setup app: failed to create webview: WebView2 error
→ HRESULT(0x800700AA)「请求的资源在使用中。」(ERROR_BUSY)
```

中间曾因删光 `%LOCALAPPDATA%\com.ziptool.desktop` 出现 `拒绝访问。(os error 5)`（目录重建失败）；手动建回目录后仍回到 `0x800700AA`。

## 根因（已确认）

`C:\Users\lawrence\workspace\ziptool` **整棵目录树被标成 Low Integrity**：

```text
icacls C:\Users\lawrence\workspace\ziptool
→ Mandatory Label\Low Mandatory Level:(OI)(CI)(NW)
```

从该树内启动的宿主 exe（如 `target\debug\zip-tool-tauri.exe`）以低完整性运行。  
WebView2 在 **`CreateCoreWebView2Controller`** 完成回调返回 `0x800700AA`，且 **全程不拉起** `msedgewebview2`、不写 `EBWebView`。

| 条件 | Controller 结果 |
|------|-----------------|
| exe 在 Temp（无 Low IL） | 成功，拉起 browser，写出 EBWebView |
| exe 在 ziptool 树内 + Low IL | **0x800700AA**，browser 数 = 0 |
| 仅将该 exe `/setintegritylevel Medium` | **成功** |
| 管理员提权运行（High IL，路径仍在 ziptool） | **成功** |

父目录 `...\workspace`、同级 `ziptool2` **没有** Low IL → 解释「只有本仓库路径失败」。

同目录上另有异常 ACE（与 dsh 沙箱机制一致，见下节）：

- `Everyone:(CI)(DENY)(DC)` — 拒绝父目录 `FILE_DELETE_CHILD`
- `S-1-4-957346370-904739272:(OI)(CI)(W,D,DC)` — 按工作区路径派生的 capability SID（非 Windows 内置 SID）

## 谁写入了 Low IL：DeepSeek Harness（dsh）

**高度确信是 dsh 的 Windows ACL 沙箱留下的，不是本仓库代码或普通用法错误。**

### 机制（dsh 设计如此）

`packages/sandbox/sandbox-windows-acl`（`workspace-write`）会在工作区根上一次写入：

1. `Mandatory Label\Low Mandatory Level` + `(OI)(CI)` 继承  
2. 对 Everyone / world 拒绝 `FILE_DELETE_CHILD`  
3. 按规范路径派生的 capability SID（`S-1-4-…`）

本机旁证：已安装 DeepSeek Harness；`%USERPROFILE%\.dsh` 下存在会话目录  
`--C-Users-lawrence-workspace-ziptool--`（dsh 曾把本仓库当作 workspace）。

文档：  
[sandbox-windows-acl README](https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/sandbox/sandbox-windows-acl/README.md)

### 这是 bug 还是使用不当？

| 层面 | 判定 |
|------|------|
| 沙箱会话内给工作区标 Low | **设计行为**（隔离写入所需） |
| 会话结束 / 移除工作区后标签仍留在磁盘，编出的 exe 以 Low IL 跑 | **已知产品缺陷/副作用**（清理不完整） |
| 用 dsh 打开本仓库做开发 | **正常用法**，不是误操作 |

官方讨论亦将「标签持久残留 → 工作区内构建的 exe 异常」当作问题跟踪：

- [Discussion #9314](https://github.com/deepseek-ai/deepseek-harness/discussions/9314)  
- [Discussion #9819](https://github.com/deepseek-ai/deepseek-harness/discussions/9819)

合理预期：退出 dsh 后应恢复 Medium，或提供一键「释放工作区」。当前需手动 `icacls … Medium`。

若继续用 dsh：先关闭占用本目录的会话，再改回 Medium；否则沙箱可能再次标成 Low。

## 修复

已对仓库递归执行：

```bat
icacls "C:\Users\lawrence\workspace\ziptool" /setintegritylevel (OI)(CI)Medium /T
```

复查（应见 `Medium`，不应再是 `Low`）：

```bat
icacls "C:\Users\lawrence\workspace\ziptool"
icacls "C:\Users\lawrence\workspace\ziptool\target\debug\zip-tool-tauri.exe"
```

## 启动链路（失败点）

```text
ziptool / Tauri / wry
  → CoInitializeEx / HWND                         ✅
  → CreateCoreWebView2EnvironmentWithOptions      ✅（仓库内外、High/Low IL 均 S_OK）
  → ICoreWebView2Environment::CreateCoreWebView2Controller
        sync 受理 S_OK
        完成回调                                  ❌ Low IL 时 0x800700AA
```

说明：`CreateCoreWebView2EnvironmentWithOptions` **不是**失败点（曾误判）。  
官方 Runtime 实现无公开源码；结论靠分层探针 + ACL 对照得出。

## 分层结论

| 层 | 结论 |
|----|------|
| ziptool 业务 / dialog / 菜单 | 否 |
| Tauri 框架 | 否（仅包装错误） |
| wry | 否（只转发 WebView2 调用） |
| `CreateCoreWebView2EnvironmentWithOptions` | 否 |
| **`CreateCoreWebView2Controller` + 宿主 Low IL** | **是** |

## 已排除的误判

| 假设 | 结论 |
|------|------|
| `EBWebView` / UDF 被其他进程占用 | 否；失败时往往无写出 EBWebView、无 ziptool 相关 `msedgewebview2` |
| `WEBVIEW2_USER_DATA_FOLDER` | Tauri 用 API 指定 UDF，env 几乎无效 |
| WebView2 运行时损坏 | 否；Temp / Medium IL / 提权均可 |
| 仅 debug / about:blank / 插件 | 否 |
| 「路径字符串含 ziptool」本身 | 否；实质是该路径上的 **Low Mandatory Level** |
| 工作目录 cwd | 否；exe 在 Temp、cwd 在仓库仍成功 |

## 关键验证手段（备查）

- 探针：`%LOCALAPPDATA%\Temp\wv2env`（只调 Environment + Controller）
- 对照：同一二进制分别放在 Temp vs `ziptool\_rc`
- 识别 ziptool WebView2：

```powershell
Get-CimInstance Win32_Process -Filter "name='msedgewebview2.exe'" |
  Where-Object { $_.CommandLine -match 'com\.ziptool\.desktop' }
```

- 查完整性标签：`icacls <path> | findstr /i Mandatory`

## 若复发

```bat
icacls "C:\Users\lawrence\workspace\ziptool" | findstr /i Mandatory
```

若再次出现 `Low Mandatory Level`：

1. 优先检查是否又用 **DeepSeek Harness** 打开过本仓库（关会话后再修）
2. 重新执行：`icacls "...\ziptool" /setintegritylevel (OI)(CI)Medium /T`
