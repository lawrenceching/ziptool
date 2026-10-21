# 报告草稿：dsh `workspace-write` 与 Windows Tauri / WebView2 开发冲突

> **用途**：向 DeepSeek Harness（dsh）反馈问题。可据此开 Discussion / Issue。  
> **日期**：2026-10-21  
> **项目**：ziptool（Rust + Tauri 2 + wry + WebView2）  
> **相关本地笔记**：[`HRESULT_0x800700AA.md`](./HRESULT_0x800700AA.md)  
> **相关上游讨论**：[Discussion #9314](https://github.com/deepseek-ai/deepseek-harness/discussions/9314)、[#9819](https://github.com/deepseek-ai/deepseek-harness/discussions/9819)、[`sandbox-windows-acl` README](https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/sandbox/sandbox-windows-acl/README.md)

---

## English summary (for GitHub)

**Title suggestion:** `workspace-write` Low integrity label breaks Tauri/WebView2 apps built and run from the workspace

On Windows, dsh `workspace-write` (ACL sandbox) permanently labels the workspace root with `Mandatory Label\Low Mandatory Level (OI)(CI)`. Executables built under that tree (e.g. `cargo run` → `target\debug\*.exe`) run at Low IL. For Tauri 2 / wry apps this fails at `ICoreWebView2Environment::CreateCoreWebView2Controller` with `HRESULT 0x800700AA` (“The requested resource is in use”): the browser process (`msedgewebview2.exe`) is never started. Environment creation succeeds; Controller fails.

This matches the known “label persists after session” issue (#9314 / #9819), but the user-visible failure for GUI desktop apps is a cryptic WebView2 error rather than a clear write/zone warning.

**Ask:** (1) revert Low IL (and related ACEs) when a workspace/session is closed or removed; (2) document that Tauri/WebView2 / other Medium-IL GUI hosts cannot be developed via `cargo run` inside or after `workspace-write` without repair; (3) ideally a one-click “release workspace” that restores Medium.

**Workaround we used:**

```bat
icacls "<workspace>" /setintegritylevel (OI)(CI)Medium /T
```

---

## 1. 背景

我们在 Windows 上开发 **Tauri 2** 桌面应用（嵌入 **WebView2**，经 wry）。该仓库曾作为 **dsh workspace** 使用（本机存在会话目录 `--C-Users-lawrence-workspace-ziptool--`）。

之后在仓库内执行：

```bat
cargo run -p zip-tool-tauri
```

应用在创建 WebView 时崩溃，错误信息具有误导性：

```text
failed to create webview: WebView2 error
HRESULT(0x800700AA) 请求的资源在使用中。
```

表面像「用户数据目录被占用」，实际根因是工作区被标成 **Low Integrity**。

---

## 2. 现象与复现

### 2.1 环境

- OS：Windows 10/11  
- 应用：Tauri 2.12 + wry 0.57 + WebView2 Runtime（Evergreen）  
- dsh：曾以 `workspace-write` / Windows ACL 沙箱打开该仓库  

### 2.2 复现步骤

1. 用 dsh 将某目录添加为 workspace，并启用 Windows ACL / `workspace-write` 沙箱（至少跑过会触发 provisioning 的会话）。  
2. 结束或离开 dsh 会话（**不**手动恢复 ACL）。  
3. 在该目录内 `cargo build` / `cargo run` 一个带 WebView2 的 Tauri 应用。  
4. 观察启动失败。

### 2.3 关键观察

| 检查 | 结果 |
|------|------|
| `icacls <workspace>` | `Mandatory Label\Low Mandatory Level:(OI)(CI)(NW)` |
| 同目录 ACE | `Everyone:(CI)(DENY)(DC)`；以及 `S-1-4-…` capability SID |
| 父目录 / 未进过 dsh 的同级目录 | **无** Low IL |
| 将同一 exe 拷到 `%TEMP%` 再运行 | **成功** |
| 仅对该 exe `icacls … /setintegritylevel Medium` | **成功** |
| 管理员提权运行（路径仍在 workspace） | **成功** |

分层探针（无 Tauri，只调 WebView2 API）进一步证明：

| API | Temp（Medium） | workspace 内（Low） |
|-----|----------------|---------------------|
| `CreateCoreWebView2EnvironmentWithOptions` | S_OK | S_OK |
| `CreateCoreWebView2Controller` 完成回调 | S_OK，拉起 `msedgewebview2` | **0x800700AA**，**browser 进程数为 0** |

因此失败点是 **Controller**，不是 Environment，也不是「UDF 被别的进程占用」。

---

## 3. 与 dsh 设计的对应关系

`sandbox-windows-acl` 在 `workspace-write` provisioning 时会：

1. 给工作区根打上 **Low Mandatory Level**（`(OI)(CI)` 继承）；  
2. 拒绝 Everyone 的 `FILE_DELETE_CHILD`；  
3. 写入按路径派生的 capability SID。  

这与我们在工作区上看到的 ACL **完全一致**。  
沙箱会话内使用 Low IL **属于设计**；问题在于：

> **标签与相关 ACE 在会话结束后仍持久留在磁盘上**，使「在仓库外正常开发」的进程（`cargo run` 出的 GUI）也以 Low IL 运行。

对 Tauri/WebView2 的具体后果：Low IL 宿主无法正常创建 WebView2 Controller / 拉起 browser，却返回 **0x800700AA**，排查成本极高（易误判为文件锁、二次实例、Runtime 损坏等）。

这与 #9314（Low IL 下构建产物写失败）、#9819（Low → Internet zone / 快捷方式异常）是同一根因的不同表象；本报告补充 **GUI 宿主 + WebView2** 这一类。

---

## 4. 影响面

- **受影响**：Windows 上在 dsh `workspace-write` 工作区内构建并运行的 **Tauri / wry / 原生 WebView2** 应用（以及任何需要 Medium IL 宿主才能拉起 WebView2 的程序）。  
- **不受影响或影响较轻**：纯 CLI、不创建 WebView 的工具（仍可能遇到 #9314 所述的「Low 进程写 Medium 路径失败」）。  
- **在 dsh 沙箱内 `cargo run` Tauri**：因令牌本身为 Low，**通常同样无法**正常起 WebView2；不能指望「只在 dsh 里跑就不修 ACL」。  
- **开发体验**：错误信息与根因无关，导致长时间误排查。

---

## 5. 我们对 dsh 的请求

1. **会话结束 / 移除 workspace 时自动恢复**  
   - 将工作区（及已继承的子树）完整性标签恢复为 Medium（或移除显式 Low，回到系统默认）；  
   - 同步清理 provisioning 写入的 capability ACE 与 Everyone `DELETE_CHILD` deny（若不再需要）。  

2. **提供显式「释放工作区 / Release workspace」操作**  
   - 一键修复 ACL，避免用户必须自行研究 `icacls`。  

3. **文档与产品提示**  
   - 明确说明：`workspace-write` 会修改工作区 MIC/DACL，且当前版本可能**持久残留**；  
   - 警告：在该工作区内开发 Tauri/WebView2 / 其他 GUI 宿主时，会话外 `cargo run` 可能失败，并给出修复命令；  
   - 错误/帮助文案中避免让用户只去查「文件占用」。  

4. **（可选）开发者模式**  
   - 对「需要 Medium IL 宿主」的本地 GUI 调试，提供不打 Low 标签、或仅会话内临时标签且保证退出即还原的模式。

---

## 6. 临时缓解（用户侧）

关闭占用该目录的 dsh 会话后：

```bat
icacls "<workspace>" /setintegritylevel (OI)(CI)Medium /T
```

复查：

```bat
icacls "<workspace>" | findstr /i Mandatory
```

应看到 `Medium Mandatory Level`，而不再是 `Low`。  
注意：`icacls /reset` **不能**清掉完整性标签（#9819 已说明）。

若再次用 dsh 打开同一工作区，标签可能被重新写入，需再次修复或等待上游提供自动恢复。

---

## 7. 建议的 Issue / Discussion 标题与标签

- **Title:** `workspace-write` Low IL persists and breaks Tauri/WebView2 (`CreateCoreWebView2Controller` → `0x800700AA`)  
- **Labels（若适用）：** `bug`, `windows`, `sandbox`, `documentation`  
- **关联：** #9314, #9819, `sandbox-windows-acl`

---

## 8. 联系与复现意愿

本报告基于 ziptool 仓库上的完整分层复现（纯 WebView2 探针 + Tauri + `icacls` 对照）。  
如需，可提供：探针二进制构建步骤、失败/成功两侧日志、以及修复前后的 `icacls` 输出。

—— 草稿结束 ——
