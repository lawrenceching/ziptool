// zip-tool UI 逻辑(纯静态, 无打包器; 由 Node 单测 test/ui.test.mjs 契约覆盖)。
// 后端交互规格见 _docs/ui-backend.md:
// - scan → 表格渲染并缓存在外壳(壳层 ScanContext 保鲜)
// - compress / extract_archive 按勾选名单执行, 进度经 Channel 推送
// - 行状态推导: pos < done 完成 / == done 进行中 / 失败行红色徽章
// 无 window.__TAURI__ 时走浏览器预览 mock(假数据 + 定时器推进度)。

const $ = (id) => document.getElementById(id);
const i18n = window.zipToolI18n;
const t = (key, params) => i18n.t(key, params);
const state = {
  mode: "compress",
  rows: [], // 行序 = scan 返回序(行状态推导协议依赖此顺序)
  run: null, // { sel: [行下标], done, total, failed, finished }
  plan: null, // { planId, packages, summary, warnings }
  planSeq: 0, // 丢弃过期异步方案
  planning: false,
  planError: null,
  planOpen: false,
  planFilter: "all",
};

const DEFAULT_PACKAGING = {
  mergeBelow: "100m",
  splitAbove: "2g",
  bundleMax: "",
  maxEntries: "50000",
};

// 原生拖拽: 两个投放区 + 最近命中行 + 校验竞态 token
const DROP_ZONE_IDS = ["src", "dst"];
const DROP_PLACEHOLDER_KEYS = {
  src: "path.srcPlaceholder",
  dst: "path.dstPlaceholder",
};
let dropHover = null;
let dropToken = 0;

// 行状态: pending 待处理 / meta 元数据 / warning 告警 / done 完成 / running 进行中 / failed 失败
const BADGE = {
  pending: "bg-blue-50 text-blue-600",
  done: "bg-emerald-50 text-emerald-600",
  running: "bg-blue-100 text-blue-700",
  failed: "bg-red-50 text-red-600",
  meta: "bg-slate-100 text-slate-400",
  warning: "bg-amber-50 text-amber-600",
};

const PKG_BADGE = {
  direct: "bg-slate-100 text-slate-600",
  split: "bg-amber-50 text-amber-700",
  bundle: "bg-blue-50 text-blue-700",
  warning: "bg-amber-50 text-amber-700",
  none: "text-slate-400",
};

// 压缩模式预设 = 底层参数组合(格式 + 强度);core/CLI 无"模式"概念(_docs/compress-modes.md)
// level: null = 不传级别, 用后端默认(zip -6 / 7z -mx=5; -9 压缩时间过长, 默认不拉满)
const PRESETS = {
  compat: { format: "zip", level: null },
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
  $("opt-level").value = p.level ?? ""; // null = 后端默认, 输入框留空显示占位符
  highlightPreset(name);
  updatePwHint();
}

let lastStatus = { key: "status.ready", params: {} };

function setStatus(key, params = {}) {
  lastStatus = { key, params };
  renderStatus();
}

function renderStatus() {
  const p = { ...lastStatus.params };
  const warnings = Array.isArray(p.warnings) ? p.warnings : null;
  delete p.warnings;
  if (typeof p.bytes === "number") p.bytes = fmtBytes(p.bytes);
  if (typeof p.doneBytes === "number") p.doneBytes = fmtBytes(p.doneBytes);
  if (typeof p.totalBytes === "number") p.totalBytes = fmtBytes(p.totalBytes);
  let text = t(lastStatus.key, p);
  if (warnings && warnings.length > 0) {
    text += t("status.warnings", { count: warnings.length, first: warnings[0] });
    if (warnings.length > 1) text += "…";
  }
  const el = $("msg");
  el.textContent = text;
  el.title = text; // 长消息截断时悬停看全文
}

function syncActionButton() {
  $("confirm").textContent = state.run ? t("header.cancel") : t("header.confirm");
}

const LANG_ITEMS = () => [...$("lang-menu").querySelectorAll("[data-lang]")];

function syncLanguageMenu() {
  const preference = i18n.readPreference();
  for (const item of LANG_ITEMS()) {
    const selected = item.dataset.lang === preference;
    item.setAttribute("aria-checked", String(selected));
    item.classList.toggle("bg-blue-50", selected);
    item.classList.toggle("text-blue-600", selected);
  }
}

function setLangMenuOpen(open, focus = false) {
  $("lang-menu").classList.toggle("hidden", !open);
  $("lang-toggle").setAttribute("aria-expanded", String(open));
  if (open && focus) {
    const checked = $("lang-menu").querySelector('[aria-checked="true"]');
    (checked || LANG_ITEMS()[0])?.focus();
  }
}

$("lang-toggle").addEventListener("click", (e) => {
  e.stopPropagation();
  const open = $("lang-menu").classList.contains("hidden");
  syncLanguageMenu();
  setLangMenuOpen(open, open);
});

$("lang-menu").addEventListener("click", async (e) => {
  const item = e.target.closest?.("[data-lang]");
  if (!item) return;
  e.stopPropagation();
  await i18n.setLocale(item.dataset.lang);
  syncLanguageMenu();
  setLangMenuOpen(false);
  $("lang-toggle").focus();
});

document.addEventListener("mousedown", (e) => {
  if ($("lang-menu").classList.contains("hidden")) return;
  if (
    !$("lang-toggle").contains(e.target) &&
    !$("lang-menu").contains(e.target)
  ) {
    setLangMenuOpen(false);
  }
});

document.addEventListener("keydown", (e) => {
  if ($("lang-menu").classList.contains("hidden")) return;
  if (e.key === "Escape") {
    setLangMenuOpen(false);
    $("lang-toggle").focus();
    return;
  }
  if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
  const items = LANG_ITEMS();
  const index = items.indexOf(document.activeElement);
  let next;
  if (e.key === "Home") next = 0;
  else if (e.key === "End") next = items.length - 1;
  else if (e.key === "ArrowDown") next = index < 0 ? 0 : (index + 1) % items.length;
  else next = index < 0 ? items.length - 1 : (index - 1 + items.length) % items.length;
  items[next]?.focus();
  e.preventDefault();
});

window.addEventListener("localechange", () => {
  renderStatus();
  renderTable(state.rows);
  renderPlanSummary();
  renderPlanDialog();
  updatePwHint();
  syncActionButton();
  syncLanguageMenu();
  setDropHover(dropHover, true);
});

// —— 大小显示 ——
function fmtBytes(n) {
  if (!Number.isFinite(n) || n < 0) return "-";
  const [unit, divisor, digits] =
    n >= 1024 ** 3
      ? ["size.gb", 1024 ** 3, 2]
      : n >= 1024 ** 2
        ? ["size.mb", 1024 ** 2, 1]
        : n >= 1024
          ? ["size.kb", 1024, 1]
          : ["size.b", 1, 0];
  const value = new Intl.NumberFormat(i18n.locale, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  }).format(n / divisor);
  return `${value} ${t(unit)}`;
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
  clearPlan();
  syncConfirm();
}

// —— 任务互斥的 UI 侧: 任务期间锁控件(确认钮另行切换成取消) ——
const LOCK_IDS = [
  "mode-compress", "mode-extract", "src", "dst",
  "opt-split", "opt-merge-below", "opt-bundle-max", "opt-max-entries",
  "opt-password", "opt-random", "opt-force",
  "preset-compat", "preset-balanced", "opt-advanced", "opt-format", "opt-level",
  "opt-exclude", "opt-verify",
];
// 确认钮两种形态: 常规蓝 / 运行中红色「取消」
const BTN_CONFIRM =
  "px-6 py-1.5 rounded-lg bg-blue-600 text-white font-medium enabled:hover:bg-blue-700 disabled:opacity-40 disabled:cursor-not-allowed transition-colors";
const BTN_CANCEL =
  "px-6 py-1.5 rounded-lg bg-red-600 text-white font-medium enabled:hover:bg-red-700 disabled:opacity-40 disabled:cursor-not-allowed transition-colors";
function setLocked(locked) {
  for (const id of LOCK_IDS) $(id).disabled = locked;
  if (locked) setDropHover(null); // 任务期不允许拖拽更换路径
}

// —— 模式切换: 切换高亮 + 选项显隐; 已有扫描结果则作废待重扫 ——
function setMode(mode) {
  const switched = state.mode !== mode;
  state.mode = mode;
  // 作废进行中的拖拽校验, 并清掉拖拽高亮
  dropToken++;
  setDropHover(null);
  const on = "px-5 py-1.5 font-medium bg-blue-600 text-white transition-colors";
  const off =
    "px-5 py-1.5 font-medium bg-white text-slate-600 hover:bg-slate-50 transition-colors";
  $("mode-compress").className = mode === "compress" ? on : off;
  $("mode-extract").className = mode === "extract" ? on : off + " border-l border-slate-300";
  // 选项显隐: 压缩=策略/随机名, 解压=覆盖; 密码两模式共用
  $("strategy-row").classList.toggle("hidden", mode !== "compress");
  $("strategy-row").classList.toggle("flex", mode === "compress");
  $("th-package").classList.toggle("hidden", mode !== "compress");
  $("opt-random-wrap").classList.toggle("hidden", mode !== "compress");
  $("opt-verify-wrap").classList.toggle("hidden", mode !== "compress");
  $("opt-force-wrap").classList.toggle("hidden", mode !== "extract");
  $("opt-force-wrap").classList.toggle("flex", mode === "extract");
  $("opt-preset-wrap").classList.toggle("hidden", mode !== "compress");
  $("opt-preset-wrap").classList.toggle("flex", mode === "compress");
  $("opt-advanced-wrap").classList.toggle("hidden", mode !== "compress");
  $("opt-advanced-wrap").classList.toggle("flex", mode === "compress");
  if (switched) clearPlan();
  if (switched && state.rows.length > 0) {
    $("src").value = "";
    clearTable();
    setStatus("status.modeSwitched");
  }
}

// —— 进度事件(扫描/压缩/解压共用一条 Channel 协议) ——
function onProgress(ev) {
  if (ev.phase === "scanning") {
    setStatus("status.scanning", { done: ev.done, total: ev.total, entry: ev.entry });
    return;
  }
  if (!state.run) return;
  state.run.done = ev.done;
  state.run.total = ev.total;
  setStatus(state.mode === "compress" ? "status.compressing" : "status.extracting", {
    done: ev.done,
    total: ev.total,
    entry: ev.entry,
    doneBytes: ev.doneBytes,
    totalBytes: ev.totalBytes,
  });
  renderTable(state.rows);
}

// 排除模式输入: 换行/逗号分隔, 去空白, 空段剔除(高级区)
function parseExcludes() {
  return $("opt-exclude").value
    .split(/[\n,]/)
    .map((s) => s.trim())
    .filter(Boolean);
}

// —— 打包策略输入 / 勾选名单 ——
function readPackaging() {
  const maxRaw = $("opt-max-entries").value.trim();
  return {
    mergeBelow: $("opt-merge-below").value.trim() || null,
    splitAbove: $("opt-split").value.trim() || null,
    bundleMax: $("opt-bundle-max").value.trim() || null,
    maxEntries: maxRaw === "" ? null : Number(maxRaw),
  };
}

function selectedRowIndexes() {
  return state.rows
    .map((r, i) => (r.status === "pending" && r.checked ? i : -1))
    .filter((i) => i >= 0);
}

// 浏览器预览: 简单复刻 strategy 的分类/FFD, 只用于 mock 模式
function parseMockSize(text, fallback) {
  if (text == null || String(text).trim() === "") return fallback;
  const m = /^(\d+(?:\.\d+)?)\s*([kmg])?$/i.exec(String(text).trim());
  if (!m) return fallback;
  const n = Number(m[1]);
  const unit = (m[2] || "").toLowerCase();
  const mult = unit === "g" ? 1024 ** 3 : unit === "m" ? 1024 ** 2 : unit === "k" ? 1024 : 1;
  return Math.round(n * mult);
}

function mockPlan(indexes) {
  const p = readPackaging();
  const merge = parseMockSize(p.mergeBelow, 100 * 1024 ** 2);
  const split = parseMockSize(p.splitAbove, 2 * 1024 ** 3);
  const bundleMax = parseMockSize(p.bundleMax, split ?? 2 * 1024 ** 3);
  const maxEntries = p.maxEntries && p.maxEntries > 0 ? p.maxEntries : null;

  const packages = [];
  const candidates = [];
  indexes.forEach((idx) => {
    const row = state.rows[idx];
    const item = { index: idx, size: row.size, entries: 1 };
    if (split != null && row.size > split) {
      packages.push({ kind: "split", members: [item] });
    } else if (merge != null && row.size <= merge) {
      candidates.push(item);
    } else {
      packages.push({ kind: "direct", members: [item] });
    }
  });

  const bundles = [];
  candidates
    .sort((a, b) => b.size - a.size || a.index - b.index)
    .forEach((item) => {
      const target = bundles.find(
        (b) =>
          b.bytes + item.size <= bundleMax &&
          (!maxEntries || b.entries + item.entries <= maxEntries)
      );
      if (target) {
        target.members.push(item);
        target.bytes += item.size;
        target.entries += item.entries;
      } else {
        bundles.push({ members: [item], bytes: item.size, entries: item.entries });
      }
    });
  bundles.forEach((b) => {
    if (b.members.length === 1) {
      packages.push({ kind: "direct", members: b.members });
    } else {
      packages.push({ kind: "bundle", members: b.members });
    }
  });

  const out = packages.map((pkg, i) => ({
    id: `p${i}`,
    kind: pkg.kind,
    memberIndexes: pkg.members.map((m) => m.index),
    inputBytes: pkg.members.reduce((s, m) => s + m.size, 0),
    entryCount: pkg.members.reduce((s, m) => s + m.entries, 0),
  }));
  const count = (kind) => out.filter((pkg) => pkg.kind === kind).length;
  const maxDirectOrBundleBytes = out
    .filter((pkg) => pkg.kind !== "split")
    .reduce((max, pkg) => Math.max(max, pkg.inputBytes), 0);
  const maxBundleEntries = out
    .filter((pkg) => pkg.kind === "bundle")
    .reduce((max, pkg) => Math.max(max, pkg.entryCount), 0);
  return {
    planId: `mock-${indexes.join(",")}-${JSON.stringify(p)}`,
    packages: out,
    summary: {
      totalPackages: out.length,
      direct: count("direct"),
      split: count("split"),
      bundle: count("bundle"),
      selectedBytes: indexes.reduce((s, i) => s + state.rows[i].size, 0),
      maxDirectOrBundleBytes,
      maxBundleEntries,
      effective: {
        mergeBelow: merge,
        splitAbove: split,
        bundleMax,
        maxEntries,
      },
    },
    warnings: [],
  };
}

// —— 打包方案: 计算 / 行映射 / 摘要 / 抽屉 ——
let planTimer = null;

function invalidatePlan() {
  state.planSeq++;
  state.plan = null;
  state.planError = null;
  state.planning = true;
  applyPlanToRows();
  renderTable(state.rows);
  renderPlanSummary();
  if (state.planOpen) renderPlanDialog();
  syncConfirm();
}

function schedulePlan() {
  if (state.mode !== "compress") return;
  invalidatePlan();
  if (planTimer) clearTimeout(planTimer);
  planTimer = setTimeout(() => {
    planTimer = null;
    refreshPlan();
  }, 180);
}

function clearPlan() {
  state.planSeq++;
  state.plan = null;
  state.planning = false;
  state.planError = null;
  state.planOpen = false;
  $("plan-backdrop")?.classList.add("hidden");
  state.rows.forEach((row) => {
    delete row.packageId;
    delete row.packageKind;
    delete row.packageIndex;
    delete row.packageOrdinal;
    delete row.packageTip;
    delete row.packageWarning;
  });
  renderPlanSummary();
  syncConfirm();
}

function applyPlanToRows() {
  state.rows.forEach((row) => {
    delete row.packageId;
    delete row.packageKind;
    delete row.packageIndex;
    delete row.packageOrdinal;
    delete row.packageTip;
    delete row.packageWarning;
  });
  if (!state.plan) return;
  const eff = state.plan.summary?.effective ?? {};
  const warnings = new Map(
    (state.plan.warnings ?? []).map((warning) => [warning.memberIndex, warning])
  );
  const ordinals = { direct: 0, split: 0, bundle: 0 };
  state.plan.packages.forEach((pkg, packageIndex) => {
    ordinals[pkg.kind] = (ordinals[pkg.kind] ?? 0) + 1;
    pkg.memberIndexes.forEach((rowIndex) => {
      const row = state.rows[rowIndex];
      if (!row) return;
      row.packageId = pkg.id;
      row.packageKind = pkg.kind;
      row.packageIndex = packageIndex;
      row.packageOrdinal = ordinals[pkg.kind];
      const warning = warnings.get(rowIndex);
      if (warning) {
        row.packageWarning = true;
        row.packageTip = t("plan.warning.tooManyEntries", {
          name: row.name,
          count: warning.entryCount,
          limit: warning.limit,
        });
      } else if (pkg.kind === "split") {
        row.packageTip = t("plan.reason.split", {
          size: fmtBytes(row.size),
          limit: fmtBytes(eff.splitAbove ?? 0),
        });
      } else if (pkg.kind === "bundle") {
        row.packageTip = t("plan.reason.merge", {
          size: fmtBytes(row.size),
          limit: fmtBytes(eff.mergeBelow ?? 0),
        });
      } else {
        row.packageTip = t("plan.reason.direct", { size: fmtBytes(row.size) });
      }
    });
  });
}

function packageLabel(row) {
  if (!row.packageKind) return "—";
  if (row.packageWarning) return t("plan.kind.directOver");
  if (row.packageKind === "bundle") {
    return t("plan.kind.bundleN", { n: row.packageOrdinal });
  }
  return t(`plan.kind.${row.packageKind}`);
}

function packageBadgeClass(row) {
  if (!row.packageKind) return PKG_BADGE.none;
  if (row.packageWarning) return PKG_BADGE.warning;
  return PKG_BADGE[row.packageKind] ?? PKG_BADGE.none;
}

function planSummaryText() {
  if (state.mode !== "compress") return "";
  if (!state.rows.length) return t("plan.summary.idle");
  if (!selectedRowIndexes().length) return t("plan.summary.noSelection");
  if (state.planError) return t("plan.summary.error", { error: state.planError });
  if (state.planning) return t("plan.summary.planning");
  if (!state.plan) return t("plan.summary.noSelection");
  const s = state.plan.summary;
  let text = t("plan.summary.text", {
    total: s.totalPackages,
    direct: s.direct,
    split: s.split,
    bundle: s.bundle,
    max: fmtBytes(s.maxDirectOrBundleBytes),
  });
  if (s.split > 0) {
    text += " · " + t("plan.summary.splitNote", {
      count: s.split,
      limit: fmtBytes(s.effective?.splitAbove ?? 0),
    });
  }
  if ((state.plan.warnings ?? []).length > 0) {
    text += " · " + t("plan.summary.warnings", {
      count: state.plan.warnings.length,
    });
  }
  return text;
}

function renderPlanSummary() {
  const el = $("plan-summary");
  const button = $("plan-open");
  if (!el || !button) return;
  const text = planSummaryText();
  el.textContent = text;
  el.title = text;
  el.className = "text-xs " +
    (state.planError ? "text-red-600" : state.planning ? "text-slate-400" : "text-slate-600");
  button.disabled =
    state.mode !== "compress" || !state.plan || state.planning || !!state.planError;
}

async function refreshPlan() {
  if (state.mode !== "compress" || !$("src").value || !state.rows.length) {
    clearPlan();
    return;
  }
  const indexes = selectedRowIndexes();
  if (!indexes.length) {
    clearPlan();
    renderTable(state.rows);
    return;
  }

  const seq = ++state.planSeq;
  state.planning = true;
  state.planError = null;
  renderPlanSummary();
  syncConfirm();
  const names = indexes.map((i) => state.rows[i].name);
  try {
    const result = hasTauri()
      ? await invoke("plan_packages", {
          src: $("src").value,
          entries: names,
          packaging: readPackaging(),
        })
      : mockPlan(indexes);
    if (seq !== state.planSeq) return;
    state.plan = result;
    state.planning = false;
    state.planError = null;
    applyPlanToRows();
    renderTable(state.rows);
    renderPlanSummary();
    if (state.planOpen) renderPlanDialog();
  } catch (error) {
    if (seq !== state.planSeq) return;
    state.plan = null;
    state.planning = false;
    state.planError = String(error);
    applyPlanToRows();
    renderTable(state.rows);
    renderPlanSummary();
    if (state.planOpen) renderPlanDialog();
  }
  syncConfirm();
}

function planKindLabel(pkg) {
  if (pkg.kind === "bundle") {
    const n =
      state.plan.packages.filter((p) => p.kind === "bundle").indexOf(pkg) + 1;
    return t("plan.kind.bundleN", { n });
  }
  return t(`plan.kind.${pkg.kind}`);
}

function formatLimit(value) {
  return value == null ? "—" : fmtBytes(value);
}

function renderPlanCard(pkg) {
  const card = document.createElement("div");
  card.className = "rounded-lg border border-slate-200 overflow-hidden";

  const header = document.createElement("div");
  header.className = "flex flex-wrap items-center gap-2 bg-slate-50 px-3 py-2";
  const badge = document.createElement("span");
  badge.className =
    "inline-block rounded-full px-2 py-0.5 text-xs " +
    (PKG_BADGE[pkg.kind] ?? PKG_BADGE.none);
  badge.textContent = planKindLabel(pkg);
  const meta = document.createElement("span");
  meta.className = "text-xs text-slate-500";
  meta.textContent = t("plan.packageMeta", {
    size: fmtBytes(pkg.inputBytes),
    members: pkg.memberIndexes.length,
    entries: pkg.entryCount,
  });
  header.append(badge, meta);

  const body = document.createElement("div");
  body.className = "space-y-1 px-3 py-2";
  pkg.memberIndexes.slice(0, 20).forEach((rowIndex) => {
    const row = state.rows[rowIndex];
    if (!row) return;
    const line = document.createElement("button");
    line.type = "button";
    line.dataset.rowIndex = String(rowIndex);
    line.className =
      "flex w-full items-center justify-between gap-3 rounded px-1.5 py-1 text-left text-xs text-slate-700 hover:bg-blue-50";
    const name = document.createElement("span");
    name.className = "truncate";
    name.textContent = row.name;
    const size = document.createElement("span");
    size.className = "shrink-0 tabular-nums text-slate-500";
    size.textContent = fmtBytes(row.size);
    line.append(name, size);
    body.append(line);
  });
  if (pkg.memberIndexes.length > 20) {
    const more = document.createElement("div");
    more.className = "px-1.5 text-xs text-slate-400";
    more.textContent = t("plan.moreMembers", {
      count: pkg.memberIndexes.length - 20,
    });
    body.append(more);
  }
  if (pkg.kind === "split") {
    const note = document.createElement("p");
    note.className = "px-1.5 pt-1 text-xs text-amber-700";
    note.textContent = t("plan.splitCardNote", {
      limit: formatLimit(state.plan?.summary?.effective?.splitAbove),
    });
    body.append(note);
  }

  card.append(header, body);
  return card;
}

function renderPlanDialog() {
  const backdrop = $("plan-backdrop");
  if (!backdrop) return;
  backdrop.classList.toggle("hidden", !state.planOpen);
  if (!state.planOpen) return;

  const plan = state.plan;
  const eff = plan?.summary?.effective ?? {};
  $("plan-strategy").textContent = plan
    ? t("plan.strategyLine", {
        merge: formatLimit(eff.mergeBelow),
        split: formatLimit(eff.splitAbove),
        bundle: formatLimit(eff.bundleMax),
        entries: eff.maxEntries == null ? "∞" : eff.maxEntries,
      })
    : "";

  const counts = {
    all: plan?.packages.length ?? 0,
    direct: plan?.summary?.direct ?? 0,
    split: plan?.summary?.split ?? 0,
    bundle: plan?.summary?.bundle ?? 0,
  };
  document.querySelectorAll("[data-plan-filter]").forEach((button) => {
    const filter = button.dataset.planFilter;
    button.textContent = `${t(`plan.filter.${filter}`)} ${counts[filter] ?? 0}`;
    const active = state.planFilter === filter;
    button.className =
      "rounded-full border px-2 py-0.5 " +
      (active
        ? "border-blue-500 bg-blue-50 text-blue-700"
        : "border-slate-300 text-slate-600");
  });

  const warnings = $("plan-warnings");
  if (plan?.warnings?.length) {
    warnings.classList.remove("hidden");
    warnings.textContent = plan.warnings
      .map((warning) => {
        const row = state.rows[warning.memberIndex];
        return t("plan.warning.tooManyEntries", {
          name: row?.name ?? "?",
          count: warning.entryCount,
          limit: warning.limit,
        });
      })
      .join("；");
  } else {
    warnings.classList.add("hidden");
    warnings.textContent = "";
  }

  const list = $("plan-list");
  list.textContent = "";
  if (!plan) {
    const empty = document.createElement("p");
    empty.className = "py-10 text-center text-sm text-slate-400";
    empty.textContent = state.planning
      ? t("plan.summary.planning")
      : t("plan.summary.noSelection");
    list.append(empty);
    return;
  }
  const packages = plan.packages.filter(
    (pkg) => state.planFilter === "all" || pkg.kind === state.planFilter
  );
  if (!packages.length) {
    const empty = document.createElement("p");
    empty.className = "py-10 text-center text-sm text-slate-400";
    empty.textContent = t("plan.empty");
    list.append(empty);
    return;
  }
  const limit = 200;
  packages.slice(0, limit).forEach((pkg) => list.append(renderPlanCard(pkg)));
  if (packages.length > limit) {
    const more = document.createElement("p");
    more.className = "py-2 text-center text-xs text-slate-400";
    more.textContent = t("plan.morePackages", { count: packages.length - limit });
    list.append(more);
  }
}

function setPlanOpen(open) {
  state.planOpen = open;
  renderPlanDialog();
  if (open) $("plan-close")?.focus();
  else $("plan-open")?.focus();
}

function highlightRow(rowIndex) {
  const tr = $("rows").querySelector(`tr[data-row-index="${rowIndex}"]`);
  if (!tr) return;
  tr.scrollIntoView?.({ block: "center" });
  tr.classList.add("bg-amber-50");
  setTimeout(() => tr.classList.remove("bg-amber-50"), 1200);
}

$("plan-open").addEventListener("click", () => setPlanOpen(true));
$("plan-close").addEventListener("click", () => setPlanOpen(false));
$("plan-backdrop").addEventListener("click", (event) => {
  if (event.target === $("plan-backdrop")) setPlanOpen(false);
});
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && state.planOpen) setPlanOpen(false);
});
$("plan-filters").addEventListener("click", (event) => {
  const button = event.target.closest?.("[data-plan-filter]");
  if (!button) return;
  state.planFilter = button.dataset.planFilter;
  renderPlanDialog();
});
$("plan-list").addEventListener("click", (event) => {
  const target = event.target.closest?.("[data-row-index]");
  if (target) highlightRow(Number(target.dataset.rowIndex));
});

// —— 扫描: invoke("scan"), 结果渲染表格并缓存在外壳 ——
async function doScan(path) {
  clearTable();
  setLocked(true);
  setStatus("status.scanningStart");
  try {
    if (!hasTauri()) {
      const items = mockScan(path);
      renderTable(items);
      setStatus("status.previewScan", { path, count: items.length });
      await refreshPlan();
      return;
    }
    const args = { mode: state.mode, path, onProgress: makeChannel() };
    if (state.mode === "compress") args.excludes = parseExcludes();
    const res = await invoke("scan", args);
    renderTable(res.items);
    setStatus("status.scanDone", {
      path,
      count: res.items.length,
      bytes: res.totalBytes,
      warnings: res.warnings,
    });
    await refreshPlan();
  } catch (e) {
    clearPlan();
    setStatus("status.scanFailed", { error: String(e) });
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
            ? t(
                state.mode === "compress"
                  ? "dialog.selectSourceCompress"
                  : "dialog.selectSourceExtract"
              )
            : t("dialog.selectDst"),
      });
    } else {
      setStatus("status.dialogUnavailable");
      return;
    }
  } catch (e) {
    setStatus("status.dialogFailed", { error: String(e) });
    return;
  }
  if (!path) return; // 用户取消, 保留原值
  dropToken++; // 作废可能在跑的拖拽校验
  applyFolder(target, path);
}

// 落定一个路径: 源路径立即扫描; 目标只保存
function applyFolder(target, path) {
  $(target).value = path;
  if (target === "src") doScan(path);
  else setStatus("status.dstPicked", { path });
}

// —— 原生拖拽选择文件夹(Tauri drag-drop 事件; 坐标为物理像素) ——
function setDropHover(zone, force = false) {
  if (!force && zone === dropHover) return;
  dropHover = zone;
  for (const id of DROP_ZONE_IDS) {
    const input = $(id);
    const row = input.closest("[data-drop]");
    const on = id === zone;
    row?.classList.toggle("bg-blue-50", on);
    // CSS 中 bg-slate-50 规则晚于 bg-blue-50, 高亮时必须移除原背景
    input.classList.toggle("bg-slate-50", !on);
    input.classList.toggle("bg-blue-50", on);
    input.placeholder = on
      ? t("path.dropRelease")
      : t(DROP_PLACEHOLDER_KEYS[id]);
  }
}

// 物理坐标 → CSS 坐标 → 命中的投放区("src" / "dst")
function zoneAt(position) {
  if (!position || $("src").disabled || $("dst").disabled) return null;
  const scale = window.devicePixelRatio || 1;
  const el =
    typeof document.elementFromPoint === "function"
      ? document.elementFromPoint(position.x / scale, position.y / scale)
      : null;
  return el?.closest?.("[data-drop]")?.dataset.drop ?? null;
}

// 拖入校验: 单路径 + 必须已存在文件夹; token 丢弃过期异步结果
async function handleDrop(target, paths) {
  if ($("src").disabled || $("dst").disabled) {
    setStatus("status.dropBusy");
    return;
  }
  if (!target) {
    setStatus("status.dropWrongZone");
    return;
  }
  if (!Array.isArray(paths) || paths.length !== 1) {
    setStatus("status.dropOne");
    return;
  }
  const token = ++dropToken;
  try {
    await invoke("validate_dir", { path: paths[0] });
  } catch (e) {
    if (token === dropToken) setStatus("status.dropInvalid", { error: String(e) });
    return;
  }
  if (token !== dropToken) return; // 已切换模式 / 重选路径 / 新的拖拽
  if ($("src").disabled || $("dst").disabled) return; // 校验期间已开始任务
  applyFolder(target, paths[0]);
}

// 注册 Tauri 原生拖拽: 高亮随 enter/over 移动, drop 后按落点(回退最近 hover)落值
function initDragDrop() {
  if (!hasTauri()) return;
  const webview = window.__TAURI__.webview;
  if (!webview?.getCurrentWebview) return;
  webview
    .getCurrentWebview()
    .onDragDropEvent(({ payload }) => {
      if (payload.type === "leave") {
        setDropHover(null);
        return;
      }
      if (payload.type === "enter" || payload.type === "over") {
        setDropHover(zoneAt(payload.position));
        return;
      }
      if (payload.type === "drop") {
        const target = zoneAt(payload.position) ?? dropHover;
        setDropHover(null);
        handleDrop(target, payload.paths);
      }
    })
    .catch((e) => {
      console.error("文件夹拖拽初始化失败", e);
      setStatus("status.dragUnavailable", { error: String(e) });
    });
}

initDragDrop();

// —— 行状态推导: pos < done → 完成, pos == done → 进行中, 其余待处理 ——
// pos 是该行在"勾选名单"里的位次(表格行序 = scan 返回序);
// 终态(完成/失败)在任务收尾时烘焙进行对象的 status, 不依赖 run 存活
function displayStatus(i) {
  const r = state.rows[i];
  if (r.status !== "pending") return r.status; // meta / warning / done / failed
  if (!state.run) return "pending";
  if (state.plan && r.packageIndex != null) {
    if (r.packageIndex < state.run.done) return "done";
    if (r.packageIndex === state.run.done) {
      return state.run.finished ? "done" : "running";
    }
    return "pending";
  }
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
    cb.setAttribute("aria-label", t("row.select", { name: r.name }));
    cb.className = "accent-blue-600 align-middle";
    cb.addEventListener("change", () => {
      state.rows[i].checked = cb.checked;
      syncConfirm();
      schedulePlan();
    });
    td0.appendChild(cb);

    const td1 = document.createElement("td");
    td1.className = "py-2 pr-2 truncate max-w-0";
    // 悬停看全名 + 附注(排除原因 / 原名 / 告警原因 / 元数据行数)
    const tip = [
      r.excluded,
      r.original ? t("row.original", { name: r.original }) : null,
      r.detail,
      r.packageTip,
    ]
      .filter(Boolean)
      .join(", ");
    td1.title = tip ? `${r.name}(${tip})` : r.name;
    td1.textContent = r.name;

    const td2 = document.createElement("td");
    td2.className = "py-2 pr-4 text-right tabular-nums text-slate-600";
    td2.textContent = fmtBytes(r.size);

    const tdPkg = document.createElement("td");
    tdPkg.className = "py-2 pr-2 text-xs";
    tdPkg.classList.toggle("hidden", state.mode !== "compress");
    const pkgBadge = document.createElement("span");
    pkgBadge.className =
      "inline-block rounded-full px-2 py-0.5 text-xs " + packageBadgeClass(r);
    pkgBadge.textContent = packageLabel(r);
    if (r.packageTip) pkgBadge.title = r.packageTip;
    tdPkg.appendChild(pkgBadge);

    const td3 = document.createElement("td");
    td3.className = "py-2 pr-4";
    const badge = document.createElement("span");
    badge.className =
      "inline-block px-2 py-0.5 rounded-full text-xs " + BADGE[st];
    badge.textContent =
      st === "pending"
        ? t(
            state.mode === "compress"
              ? "row.status.pendingCompress"
              : "row.status.pendingExtract"
          )
        : st === "done"
          ? t("row.status.done")
          : st === "running"
            ? t("row.status.running")
            : st === "failed"
              ? t("row.status.failed")
              : st === "warning"
                ? t("row.status.warning")
                : t("row.status.meta");
    if (st === "failed") badge.title = r.failMsg || "";
    if (st === "warning") badge.title = r.detail || "";
    td3.appendChild(badge);

    tr.dataset.rowIndex = String(i);
    tr.append(td0, td1, td2, tdPkg, td3);
    tbody.appendChild(tr);
  }
  syncConfirm();
}

function syncConfirm() {
  // 全选框只统计可勾选行(meta/warning 禁用, 不参与)
  const selectable = state.rows.filter((r) => r.status === "pending");
  const n = selectable.filter((r) => r.checked).length;
  const all = selectable.length > 0 && n === selectable.length;
  const needPlan =
    state.mode === "compress" &&
    (!!state.planError || (!state.plan && !state.planning));
  $("confirm").disabled = !!state.run ? false : n === 0 || needPlan;
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
  schedulePlan();
});

// —— 确认 → 执行 / 运行中 → 取消 ——
$("confirm").addEventListener("click", async () => {
  if (state.run) {
    // 取消: 通知外壳, 当前条目/组完成后停止
    if (hasTauri()) invoke("cancel_current");
    else mockCancel?.();
    return;
  }
  const sel = selectedRowIndexes();
  if (sel.length === 0) {
    setStatus("status.needSelection");
    return;
  }
  if (state.mode === "compress" && (!state.plan || state.planning)) {
    // 用户刚改勾选/阈值就点确认: 等最新方案算完, 不拿旧方案开工
    if (planTimer) {
      clearTimeout(planTimer);
      planTimer = null;
    }
    await refreshPlan();
  }
  if (!$("dst").value.trim()) {
    setStatus("status.needDst");
    $("dst").focus();
    return;
  }
  const isCompress = state.mode === "compress";
  if (isCompress && !state.plan) {
    setStatus("status.needPlan");
    return;
  }
  const names = sel.map((i) => state.rows[i].name);
  // 强度留空 = 不传级别(后端默认);Number("") 是 0 = 仅存储, 空值必须显式归 null
  const lvl = $("opt-level").value.trim();
  const options = isCompress
    ? {
        splitSize: null,
        password: $("opt-password").value || null,
        randomName: $("opt-random").checked,
        format: $("opt-format").value,
        level: lvl === "" ? null : Number(lvl),
        excludes: parseExcludes(),
        verify: $("opt-verify").checked,
        packaging: readPackaging(),
      }
    : {
        password: $("opt-password").value || null,
        force: $("opt-force").checked,
      };
  const total = isCompress && state.plan ? state.plan.packages.length : sel.length;
  state.run = { sel, done: 0, total, failed: false, finished: false };
  setLocked(true);
  $("confirm").className = BTN_CANCEL;
  syncActionButton();
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
        planId: state.plan?.planId ?? null,
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
    if (isCompress && state.plan) {
      state.run.sel.forEach((rowIdx) => {
        const packageIndex = state.rows[rowIdx].packageIndex;
        if (packageIndex != null && packageIndex < doneCount) {
          state.rows[rowIdx].status = "done";
        }
      });
    } else {
      state.run.sel.forEach((rowIdx, pos) => {
        if (pos < doneCount) state.rows[rowIdx].status = "done";
      });
    }
    // 结果汇报: 压缩=包数; 解压=组数; 告警统一由 renderStatus 追加在消息尾
    if (res.cancelled) {
      setStatus("status.cancelled", {
        done: state.run.done,
        total: state.run.total,
        warnings: res.warnings,
      });
    } else if (isCompress) {
      setStatus("status.compressDone", {
        count: res.packages?.length ?? res.entries?.length ?? 0,
        dst: $("dst").value,
        warnings: res.warnings,
      });
    } else {
      setStatus("status.extractDone", {
        count: res.groups.length,
        dst: $("dst").value,
        warnings: res.warnings,
      });
    }
  } catch (e) {
    // 失败: 当前包/行标"失败"(悬停徽章看原因), 已完成的行保留完成态(宁停不错)
    const done = state.run.done;
    if (isCompress && state.plan) {
      state.run.sel.forEach((rowIdx) => {
        const packageIndex = state.rows[rowIdx].packageIndex;
        if (packageIndex != null && packageIndex < done) {
          state.rows[rowIdx].status = "done";
        } else if (packageIndex === done) {
          state.rows[rowIdx].status = "failed";
          state.rows[rowIdx].failMsg = String(e);
        }
      });
    } else {
      state.run.sel.forEach((rowIdx, pos) => {
        if (pos < done) state.rows[rowIdx].status = "done";
        else if (pos === done) {
          state.rows[rowIdx].status = "failed";
          state.rows[rowIdx].failMsg = String(e);
        }
      });
    }
    setStatus(isCompress ? "status.compressFailed" : "status.extractFailed", {
      error: String(e),
    });
  } finally {
    state.run = null;
    setLocked(false);
    $("confirm").className = BTN_CONFIRM;
    syncActionButton();
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
// 打包阈值改动只重算方案(不重扫); 立即作废旧方案, 防误按确认
for (const id of ["opt-merge-below", "opt-split", "opt-bundle-max", "opt-max-entries"]) {
  $(id).addEventListener("input", schedulePlan);
}
// 密码语义随格式: zip=ZipCrypto(兼容优先, 强度弱), 7z=AES-256
function updatePwHint() {
  const hint = $("pw-hint");
  const pw = $("opt-password").value;
  if (!pw || state.mode !== "compress") {
    hint.classList.add("hidden");
    return;
  }
  hint.textContent = t(
    $("opt-format").value === "7z" ? "options.pwHint7z" : "options.pwHintZip"
  );
  hint.classList.remove("hidden");
}
$("opt-password").addEventListener("input", updatePwHint);
$("opt-format").addEventListener("change", updatePwHint);
// 初始状态 = 压缩模式 + 最大兼容性预设
// 显隐统一由 setMode 管理, 启动必须跑一次: 否则 preset/advanced 的初始 hidden 不会解除,
// 首次打开要切去解压再切回来才看得到
setMode(state.mode);
applyPreset("compat");
renderStatus();
renderPlanSummary();
syncActionButton();
syncLanguageMenu();

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
  const packages = state.mode === "compress" ? state.plan?.packages ?? null : null;
  const total = packages ? packages.length : names.length;
  return new Promise((resolve) => {
    let done = 0;
    const t = setInterval(() => {
      done += 1;
      let frac;
      let entry;
      if (packages) {
        frac = packages
          .slice(0, done)
          .reduce((sum, pkg) => sum + pkg.inputBytes, 0);
        const pkg = packages[done - 1];
        entry = pkg.kind === "bundle"
          ? t("plan.kind.bundleN", {
              n: packages.filter((p) => p.kind === "bundle").indexOf(pkg) + 1,
            })
          : state.rows[pkg.memberIndexes[0]]?.name ?? pkg.id;
      } else {
        frac = names.slice(0, done).reduce((sum, name) => {
          const row = state.rows.find((x) => x.name === name);
          return sum + (row ? row.size : 0);
        }, 0);
        entry = names[done - 1];
      }
      onProgress({
        phase,
        done,
        total,
        doneBytes: frac,
        totalBytes,
        entry,
      });
      if (done >= total) {
        clearInterval(t);
        if (packages) {
          resolve({
            cancelled: false,
            entries: [],
            packages: packages.map((pkg) => ({
              id: pkg.id,
              kind: pkg.kind,
              archive: `C:\\out\\${pkg.id}.zip`,
              members: pkg.memberIndexes.map((idx) => state.rows[idx]?.name),
              inputBytes: pkg.inputBytes,
              entryCount: pkg.entryCount,
            })),
            warnings: [],
          });
        } else {
          resolve({
            cancelled: false,
            groups: names.map((n) => ({ entry: n, original: n, files: 3 })),
            warnings: [],
          });
        }
      }
    }, 400);
    mockCancel = () => {
      clearInterval(t);
      resolve({ cancelled: true, entries: [], packages: [], groups: [], warnings: [] });
    };
  });
}

// —— 状态栏版本号 ——
Promise.resolve(window.__TAURI__?.app?.getVersion())
  .then((v) => ($("ver").textContent = v ? "v" + v : "dev"))
  .catch(() => ($("ver").textContent = "dev"));
