// zip-tool UI 逻辑(纯静态, 无打包器; 由 Node 单测 test/ui.test.mjs 契约覆盖)。
// 后端交互规格见 _docs/ui-backend.md:
// - scan → 表格渲染并缓存在外壳(壳层 ScanContext 保鲜)
// - compress / extract_archive 按勾选名单执行, 进度经 Channel 推送
// - 行状态推导: pos < done 完成 / == done 进行中 / 失败行红色徽章
// 无 window.__TAURI__ 时走浏览器预览 mock(假数据 + 定时器推进度)。

const $ = (id) => document.getElementById(id);
const state = {
  mode: "compress",
  rows: [], // 行序 = scan 返回序(行状态推导协议依赖此顺序)
  run: null, // { sel: [行下标], done, total, failed } 运行中才非空
};

const MODE_TEXT = { compress: "压缩", extract: "解压" };
// 行状态: pending 待处理 / meta 元数据 / warning 告警 / done 完成 / running 进行中 / failed 失败
const BADGE = {
  pending: "bg-blue-50 text-blue-600",
  done: "bg-emerald-50 text-emerald-600",
  running: "bg-blue-100 text-blue-700",
  failed: "bg-red-50 text-red-600",
  meta: "bg-slate-100 text-slate-400",
  warning: "bg-amber-50 text-amber-600",
};

// 压缩模式预设 = 底层参数组合(格式 + 强度);core/CLI 无"模式"概念(_docs/compress-modes.md)
const PRESETS = {
  compat: { format: "zip", level: 9 },
  balanced: { format: "7z", level: 5 },
};
// 预设按钮两种形态(选中蓝 / 未选白; 平衡钮恒有左边框)
const PRESET_ON = "px-2 py-1 bg-blue-600 text-white";
const PRESET_OFF = "px-2 py-1 bg-white text-slate-600 hover:bg-slate-50";

function highlightPreset(name) {
  $("opt-preset-custom").classList.toggle("hidden", name !== "custom");
  $("preset-compat").className = name === "compat" ? PRESET_ON : PRESET_OFF;
  $("preset-balanced").className =
    name === "balanced" ? PRESET_ON : PRESET_OFF + " border-l border-slate-300";
}

// 选预设 = 写入底层参数; 手动改参数 = 离开预设(自定义)
function applyPreset(name) {
  const p = PRESETS[name];
  $("opt-format").value = p.format;
  $("opt-level").value = p.level;
  highlightPreset(name);
  updatePwHint();
}

function status(msg) {
  const el = $("msg");
  el.textContent = msg;
  el.title = msg; // 长消息截断时悬停看全文
}

// —— 大小显示 ——
function fmtBytes(n) {
  if (!Number.isFinite(n) || n < 0) return "-";
  if (n >= 1024 ** 3) return (n / 1024 ** 3).toFixed(2) + " GB";
  if (n >= 1024 ** 2) return (n / 1024 ** 2).toFixed(1) + " MB";
  if (n >= 1024) return (n / 1024).toFixed(1) + " KB";
  return n + " B";
}

// —— 后端调用封装(浏览器预览无 Tauri 时走 mock) ——
const hasTauri = () => !!window.__TAURI__;
const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
// 进度经 Channel 推送(onProgress 参数); 回调见 onProgress()
// 注意: Tauri 2 的 Channel 在 core 命名空间(@tauri-apps/api/core), v1 的 ipc 命名空间已不存在
function makeChannel() {
  return new window.__TAURI__.core.Channel(onProgress);
}

// —— 清空扫描结果(模式切换/重选源时旧表格必须失效) ——
function clearTable() {
  state.rows = [];
  state.run = null;
  $("rows").textContent = "";
  $("empty").classList.remove("hidden");
  syncConfirm();
}

// —— 任务互斥的 UI 侧: 任务期间锁控件(确认钮另行切换成取消) ——
const LOCK_IDS = [
  "mode-compress", "mode-extract", "src", "dst",
  "opt-split", "opt-password", "opt-random", "opt-force",
  "preset-compat", "preset-balanced", "opt-advanced", "opt-format", "opt-level",
  "opt-exclude",
];
// 确认钮两种形态: 常规蓝 / 运行中红色「取消」
const BTN_CONFIRM =
  "px-6 py-1.5 rounded-lg bg-blue-600 text-white font-medium enabled:hover:bg-blue-700 disabled:opacity-40 disabled:cursor-not-allowed transition-colors";
const BTN_CANCEL =
  "px-6 py-1.5 rounded-lg bg-red-600 text-white font-medium enabled:hover:bg-red-700 disabled:opacity-40 disabled:cursor-not-allowed transition-colors";
function setLocked(locked) {
  for (const id of LOCK_IDS) $(id).disabled = locked;
}

// —— 模式切换: 切换高亮 + 选项显隐; 已有扫描结果则作废待重扫 ——
function setMode(mode) {
  const switched = state.mode !== mode;
  state.mode = mode;
  const on = "px-5 py-1.5 font-medium bg-blue-600 text-white transition-colors";
  const off =
    "px-5 py-1.5 font-medium bg-white text-slate-600 hover:bg-slate-50 transition-colors";
  $("mode-compress").className = mode === "compress" ? on : off;
  $("mode-extract").className = mode === "extract" ? on : off + " border-l border-slate-300";
  // 选项显隐: 压缩=分包+随机名, 解压=覆盖; 密码两模式共用
  $("opt-split-wrap").classList.toggle("hidden", mode !== "compress");
  $("opt-random-wrap").classList.toggle("hidden", mode !== "compress");
  $("opt-force-wrap").classList.toggle("hidden", mode !== "extract");
  $("opt-force-wrap").classList.toggle("flex", mode === "extract");
  $("opt-preset-wrap").classList.toggle("hidden", mode !== "compress");
  $("opt-preset-wrap").classList.toggle("flex", mode === "compress");
  $("opt-advanced-wrap").classList.toggle("hidden", mode !== "compress");
  $("opt-advanced-wrap").classList.toggle("flex", mode === "compress");
  if (switched && state.rows.length > 0) {
    $("src").value = "";
    clearTable();
    status("模式已切换, 请重新选择源文件夹");
  }
}

// —— 进度事件(扫描/压缩/解压共用一条 Channel 协议) ——
function onProgress(ev) {
  if (ev.phase === "scanning") {
    status(`扫描中 ${ev.done}/${ev.total} ${ev.entry}`);
    return;
  }
  if (!state.run) return;
  state.run.done = ev.done;
  state.run.total = ev.total;
  status(
    `${MODE_TEXT[state.mode]}中 ${ev.done}/${ev.total} ${ev.entry} ` +
      `(${fmtBytes(ev.doneBytes)} / ${fmtBytes(ev.totalBytes)})`
  );
  renderTable(state.rows);
}

// 排除模式输入: 换行/逗号分隔, 去空白, 空段剔除(高级区)
function parseExcludes() {
  return $("opt-exclude").value
    .split(/[\n,]/)
    .map((s) => s.trim())
    .filter(Boolean);
}

// —— 扫描: invoke("scan"), 结果渲染表格并缓存在外壳 ——
async function doScan(path) {
  clearTable();
  setLocked(true);
  status("扫描中…");
  try {
    if (!hasTauri()) {
      renderTable(mockScan(path));
      status(`已扫描 ${path}: ${state.rows.length} 个条目(浏览器预览假数据)`);
      return;
    }
    const args = { mode: state.mode, path, onProgress: makeChannel() };
    if (state.mode === "compress") args.excludes = parseExcludes();
    const res = await invoke("scan", args);
    renderTable(res.items);
    let msg = `已扫描 ${path}: ${res.items.length} 个条目, 共 ${fmtBytes(res.totalBytes)}`;
    if (res.warnings.length > 0) {
      // compress 子树级告警走状态栏汇总(表格告警行是 extract 专用通道)
      msg += `; ${res.warnings.length} 条告警: ${res.warnings[0]}` +
        (res.warnings.length > 1 ? ` 等` : "");
    }
    status(msg);
  } catch (e) {
    status("扫描失败: " + e);
  } finally {
    setLocked(false);
    syncConfirm();
  }
}

// —— 点击源/目标输入框: 弹出系统文件夹选择对话框 ——
async function pickFolder(target) {
  let path = null;
  try {
    if (!hasTauri()) {
      // 浏览器预览(无 Tauri 环境): 用假路径走通界面流程
      path = "C:\\demo\\data";
    } else if (window.__TAURI__.dialog) {
      path = await window.__TAURI__.dialog.open({
        directory: true,
        multiple: false,
        title:
          target === "src"
            ? `选择要${MODE_TEXT[state.mode]}的文件夹`
            : "选择目标文件夹",
      });
    } else {
      status("文件夹选择插件不可用, 请检查 tauri-plugin-dialog");
      return;
    }
  } catch (e) {
    status("无法打开文件夹选择框: " + e);
    return;
  }
  if (!path) return; // 用户取消, 保留原值
  $(target).value = path;
  if (target === "src") doScan(path);
}

// —— 行状态推导: pos < done → 完成, pos == done → 进行中, 其余待处理 ——
// pos 是该行在"勾选名单"里的位次(表格行序 = scan 返回序);
// 终态(完成/失败)在任务收尾时烘焙进行对象的 status, 不依赖 run 存活
function displayStatus(i) {
  const r = state.rows[i];
  if (r.status !== "pending") return r.status; // meta / warning / done / failed
  if (!state.run) return "pending";
  const pos = state.run.sel.indexOf(i);
  if (pos === -1 || pos > state.run.done) return "pending";
  if (pos < state.run.done) return "done";
  return state.run.finished ? "done" : "running";
}

// —— 表格渲染 ——
function renderTable(items) {
  // checked 已有值则保留(全选切换/进度重绘时), 否则默认勾选 pending 行
  // (被标排除行 excluded 显示但默认不勾选; 手动勾选了就会被压缩)
  state.rows = items.map((it, i) => ({
    ...it,
    checked: state.rows[i]?.checked ?? (it.status === "pending" && !it.excluded),
  }));
  const tbody = $("rows");
  tbody.textContent = "";
  $("empty").classList.toggle("hidden", items.length > 0);

  for (const [i, r] of state.rows.entries()) {
    const st = displayStatus(i);
    const tr = document.createElement("tr");
    tr.className = "border-b border-slate-100 hover:bg-slate-50";

    const td0 = document.createElement("td");
    td0.className = "py-2 text-center";
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.checked = r.checked && st === "pending";
    cb.disabled = r.status !== "pending" || !!state.run;
    cb.setAttribute("aria-label", "选择 " + r.name);
    cb.className = "accent-blue-600 align-middle";
    cb.addEventListener("change", () => {
      state.rows[i].checked = cb.checked;
      syncConfirm();
    });
    td0.appendChild(cb);

    const td1 = document.createElement("td");
    td1.className = "py-2 pr-2 truncate max-w-0";
    // 悬停看全名 + 附注(排除原因 / 原名 / 告警原因 / 元数据行数)
    const tip = [r.excluded, r.original ? `原名 ${r.original}` : null, r.detail]
      .filter(Boolean)
      .join(", ");
    td1.title = tip ? `${r.name}(${tip})` : r.name;
    td1.textContent = r.name;

    const td2 = document.createElement("td");
    td2.className = "py-2 pr-4 text-right tabular-nums text-slate-600";
    td2.textContent = fmtBytes(r.size);

    const td3 = document.createElement("td");
    td3.className = "py-2 pr-4";
    const badge = document.createElement("span");
    badge.className =
      "inline-block px-2 py-0.5 rounded-full text-xs " + BADGE[st];
    badge.textContent =
      st === "pending" ? (state.mode === "compress" ? "待压缩" : "待解压") :
      st === "done" ? "完成" :
      st === "running" ? "进行中" :
      st === "failed" ? "失败" :
      st === "warning" ? "告警" : "元数据";
    if (st === "failed") badge.title = r.failMsg || "";
    if (st === "warning") badge.title = r.detail || "";
    td3.appendChild(badge);

    tr.append(td0, td1, td2, td3);
    tbody.appendChild(tr);
  }
  syncConfirm();
}

function syncConfirm() {
  // 全选框只统计可勾选行(meta/warning 禁用, 不参与)
  const selectable = state.rows.filter((r) => r.status === "pending");
  const n = selectable.filter((r) => r.checked).length;
  const all = selectable.length > 0 && n === selectable.length;
  $("confirm").disabled = !!state.run ? false : n === 0;
  const allBox = $("check-all");
  allBox.checked = all;
  allBox.indeterminate = n > 0 && !all;
  allBox.disabled = selectable.length === 0 || !!state.run;
}

$("check-all").addEventListener("change", (e) => {
  for (const [i, r] of state.rows.entries()) {
    if (r.status === "pending") state.rows[i].checked = e.target.checked;
  }
  renderTable(state.rows);
});

// —— 确认 → 执行 / 运行中 → 取消 ——
$("confirm").addEventListener("click", async () => {
  if (state.run) {
    // 取消: 通知外壳, 当前条目/组完成后停止
    if (hasTauri()) invoke("cancel_current");
    else mockCancel?.();
    return;
  }
  const sel = state.rows
    .map((r, i) => (r.status === "pending" && r.checked ? i : -1))
    .filter((i) => i >= 0);
  if (sel.length === 0) {
    status("请先勾选要处理的条目");
    return;
  }
  if (!$("dst").value.trim()) {
    status("请先选择目标文件夹");
    $("dst").focus();
    return;
  }
  const isCompress = state.mode === "compress";
  const names = sel.map((i) => state.rows[i].name);
  const options = isCompress
    ? {
        splitSize: $("opt-split").value.trim() || null,
        password: $("opt-password").value || null,
        randomName: $("opt-random").checked,
        format: $("opt-format").value,
        level: Number($("opt-level").value),
        excludes: parseExcludes(),
      }
    : {
        password: $("opt-password").value || null,
        force: $("opt-force").checked,
      };
  state.run = { sel, done: 0, total: sel.length, failed: false, finished: false };
  setLocked(true);
  $("confirm").textContent = "取消";
  $("confirm").className = BTN_CANCEL;
  syncConfirm();
  renderTable(state.rows);

  try {
    let res;
    if (!hasTauri()) {
      res = await mockRun(names);
    } else if (isCompress) {
      res = await invoke("compress", {
        src: $("src").value,
        dst: $("dst").value,
        entries: names,
        options,
        onProgress: makeChannel(),
      });
    } else {
      res = await invoke("extract_archive", {
        src: $("src").value,
        dst: $("dst").value,
        entries: names,
        options,
        onProgress: makeChannel(),
      });
    }
    state.run.finished = true;
    if (!res.cancelled) state.run.done = state.run.total;
    // 终态烘焙: 完成(含取消前的部分)标"完成", 不再参与后续勾选
    const doneCount = state.run.done;
    state.run.sel.forEach((rowIdx, pos) => {
      if (pos < doneCount) state.rows[rowIdx].status = "done";
    });
    // 结果汇报: 压缩=包数; 解压=组数; 后续告警追加在消息尾
    let msg = res.cancelled
      ? `已取消(完成 ${state.run.done}/${state.run.total})`
      : isCompress
        ? `压缩完成: ${res.entries.length} 个压缩包 → ${$("dst").value}`
        : `解压完成: ${res.groups.length} 组 → ${$("dst").value}`;
    if (res.warnings.length > 0) {
      msg += `; ${res.warnings.length} 条告警: ${res.warnings[0]}` +
        (res.warnings.length > 1 ? ` 等` : "");
    }
    status(msg);
  } catch (e) {
    // 失败: 当前行标"失败"(悬停徽章看原因), 已完成的行保留完成态(宁停不错)
    const done = state.run.done;
    state.run.sel.forEach((rowIdx, pos) => {
      if (pos < done) state.rows[rowIdx].status = "done";
      else if (pos === done) {
        state.rows[rowIdx].status = "failed";
        state.rows[rowIdx].failMsg = String(e);
      }
    });
    status(`${MODE_TEXT[state.mode]}失败: ` + e);
  } finally {
    state.run = null;
    setLocked(false);
    $("confirm").textContent = "确认";
    $("confirm").className = BTN_CONFIRM;
    syncConfirm();
    renderTable(state.rows);
  }
});

$("mode-compress").addEventListener("click", () => setMode("compress"));
$("mode-extract").addEventListener("click", () => setMode("extract"));
$("src").addEventListener("click", () => pickFolder("src"));
$("dst").addEventListener("click", () => pickFolder("dst"));

// —— 压缩模式预设与高级微调 ——
$("preset-compat").addEventListener("click", () => applyPreset("compat"));
$("preset-balanced").addEventListener("click", () => applyPreset("balanced"));
$("opt-advanced").addEventListener("change", (e) => {
  const show = e.target.checked;
  for (const id of ["opt-format-wrap", "opt-level-wrap", "opt-exclude-wrap"]) {
    $(id).classList.toggle("hidden", !show);
    $(id).classList.toggle("flex", show);
  }
});
// 手动改底层参数 = 离开预设
for (const id of ["opt-format", "opt-level"]) {
  $(id).addEventListener("input", () => highlightPreset("custom"));
}
// 密码语义随格式: zip=ZipCrypto(兼容优先, 强度弱), 7z=AES-256
function updatePwHint() {
  const hint = $("pw-hint");
  const pw = $("opt-password").value;
  if (!pw || state.mode !== "compress") {
    hint.classList.add("hidden");
    return;
  }
  hint.textContent =
    $("opt-format").value === "7z"
      ? "7z 格式: AES-256 加密"
      : "zip 格式: ZipCrypto 加密(兼容性最好, 强度弱)";
  hint.classList.remove("hidden");
}
$("opt-password").addEventListener("input", updatePwHint);
$("opt-format").addEventListener("change", updatePwHint);
// 初始状态 = 最大兼容性预设
applyPreset("compat");

// ———— 浏览器预览 mock(无 Tauri 环境): 假扫描 + 定时器推进度 ————
let mockCancel = null;
function mockScan(path) {
  return state.mode === "compress"
    ? [
        { name: "Novel", kind: "dir", size: 2_600_000_000, status: "pending", original: null, detail: null },
        { name: "media", kind: "dir", size: 890_000_000, status: "pending", original: null, detail: null },
        { name: "photo.jpg", kind: "file", size: 204_800, status: "pending", original: null, detail: null },
        { name: ".DS_Store", kind: "file", size: 4096, status: "pending", original: null, detail: null, excluded: "隐藏文件" },
        { name: "cache", kind: "dir", size: 12_000, status: "pending", original: null, detail: null, excluded: "匹配排除模式: cache" },
      ]
    : [
        { name: "Novel.zip", kind: "file", size: 2_600_000_000, status: "pending", original: "Novel", detail: null },
        { name: "media.zip.001", kind: "file", size: 890_000_000, status: "pending", original: "media", detail: null },
        { name: "photo.jpg.zip", kind: "file", size: 204_800, status: "pending", original: null, detail: null },
        { name: "mapping.csv", kind: "file", size: 1024, status: "meta", original: null, detail: "3 行" },
        { name: "notes.txt", kind: "file", size: 0, status: "warning", original: null, detail: "非压缩包文件, 已跳过" },
      ];
}
function mockRun(names) {
  const phase = state.mode === "compress" ? "compressing" : "extracting";
  const totalBytes = state.run.sel.reduce((s, i) => s + state.rows[i].size, 0);
  return new Promise((resolve) => {
    let done = 0;
    const t = setInterval(() => {
      done += 1;
      const frac = names.slice(0, done).reduce((s, n) => {
        const r = state.rows.find((x) => x.name === n);
        return s + (r ? r.size : 0);
      }, 0);
      onProgress({
        phase, done, total: names.length,
        doneBytes: frac, totalBytes,
        entry: names[done - 1],
      });
      if (done >= names.length) {
        clearInterval(t);
        resolve(
          state.mode === "compress"
            ? { cancelled: false, entries: names.map((n) => ({ archive: "C:\\out\\" + n + ".zip", original: n })), warnings: [] }
            : { cancelled: false, groups: names.map((n) => ({ entry: n, original: n, files: 3 })), warnings: [] }
        );
      }
    }, 400);
    mockCancel = () => {
      clearInterval(t);
      resolve({ cancelled: true, entries: [], groups: [], warnings: [] });
    };
  });
}

// —— 状态栏版本号 ——
Promise.resolve(window.__TAURI__?.app?.getVersion())
  .then((v) => ($("ver").textContent = v ? "v" + v : "dev"))
  .catch(() => ($("ver").textContent = "dev"));
