// UI 单测(node --test): 契约 + 行为, 不需要起 Tauri 进程。
//
// 关键设计——表面真实、传输造假:
//   window.__TAURI__ 挂的是**真实 @tauri-apps/api (core/app/webview/event) 与
//   @tauri-apps/plugin-dialog 包对象**
//   (Tauri 的 withGlobalTauri 全局 bundle 就是这些模块挂到 __TAURI__ 上), 只有最底层
//   的 window.__TAURI_INTERNALS__(IPC 传输)被 mock。因此:
//   - UI 引用了不存在的全局 API 路径(如 __TAURI__.ipc.Channel)→ 这里必然 TypeError,
//     在 npm test 就爆掉, 而不是等到真机手工测试;
//   - invoke 的命令名 / 参数形状 / Channel 序列化格式都按真实链路校验。
// 进度回放走真实 Channel 协议: Rust 侧回调收到 { index, message }, message 已是解析后的对象。
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { JSDOM } from "jsdom";
import * as coreApi from "@tauri-apps/api/core";
import * as appApi from "@tauri-apps/api/app";
import * as webviewApi from "@tauri-apps/api/webview";
import { TauriEvent } from "@tauri-apps/api/event";
import * as dialogApi from "@tauri-apps/plugin-dialog";

const APP_JS = new URL("../ui/app.js", import.meta.url);
const I18N_JS = new URL("../ui/i18n.js", import.meta.url);
const I18N_VENDOR = new URL("../ui/vendor/i18next.min.js", import.meta.url);
const I18N_LOCALES = ["zh-CN", "zh-HK", "zh-TW", "en", "ru"].map(
  (code) => new URL(`../ui/locales/${code}.js`, import.meta.url)
);
const INDEX_HTML = new URL("../ui/index.html", import.meta.url);

// —— 壳层 DTO 形状的扫描结果(与 crates/tauri-app/src/lib.rs 的 DTO 保持一致) ——
const ITEM = (name, kind, size, status, extra = {}) => ({
  name, kind, size, status, original: null, detail: null, ...extra,
});
const SCAN_COMPRESS = {
  mode: "compress",
  items: [
    ITEM("Novel", "dir", 2_600_000_000, "pending"),
    ITEM("media", "dir", 890_000_000, "pending"),
    ITEM("photo.jpg", "file", 204_800, "pending"),
  ],
  warnings: [],
  totalBytes: 3_490_204_800,
};
const SCAN_EXTRACT = {
  mode: "extract",
  items: [
    ITEM("Novel.zip", "file", 2_600_000_000, "pending", { original: "Novel" }),
    ITEM("media.zip.001", "file", 890_000_000, "pending", { original: "media" }),
    ITEM("photo.jpg.zip", "file", 204_800, "pending"),
    ITEM("mapping.csv", "file", 1024, "meta", { detail: "3 行" }),
    ITEM("notes.txt", "file", 0, "warning", { detail: "非压缩包文件, 已跳过" }),
  ],
  warnings: [],
  totalBytes: 3_490_204_800,
};

// 默认 plan 响应: 测试只关心 UI, 不重复 strategy 的完整装箱; 每个勾选项一个包。
function defaultPlan(args) {
  const items = SCAN_COMPRESS.items;
  const packages = (args.entries ?? []).map((name, i) => {
    const index = items.findIndex((item) => item.name === name);
    const size = items[index]?.size ?? 0;
    return {
      id: `p${i}`,
      kind: size > 2 * 1024 ** 3 ? "split" : "direct",
      memberIndexes: [index],
      inputBytes: size,
      entryCount: 1,
    };
  });
  const count = (kind) => packages.filter((pkg) => pkg.kind === kind).length;
  return {
    planId: "test-plan",
    packages,
    summary: {
      totalPackages: packages.length,
      direct: count("direct"),
      split: count("split"),
      bundle: count("bundle"),
      selectedBytes: packages.reduce((sum, pkg) => sum + pkg.inputBytes, 0),
      maxDirectOrBundleBytes: packages
        .filter((pkg) => pkg.kind !== "split")
        .reduce((max, pkg) => Math.max(max, pkg.inputBytes), 0),
      maxBundleEntries: 0,
      effective: {
        mergeBelow: 100 * 1024 ** 2,
        splitAbove: 2 * 1024 ** 3,
        bundleMax: 2 * 1024 ** 3,
        maxEntries: 50_000,
      },
    },
    warnings: [],
  };
}

// 回放一条 Rust → JS 的进度消息(真实 Channel 排序协议)
function sendProgress(callbacks, channel, i, message) {
  callbacks.get(channel.id)({ index: i, message });
}

/**
 * 构建一个加载了 app.js 的页面。
 * - withTauri=false: 浏览器预览形态(无 __TAURI__, 走内置 mock)
 * - withTauri=true: 真实包表面 + mock IPC 传输; invokeImpl 按命令名分发
 */
function makeHarness(withTauri, invokeImpl, options = {}) {
  const dom = new JSDOM(readFileSync(INDEX_HTML, "utf8"), {
    url: "http://localhost/",
    runScripts: "outside-only", // window.eval 跑在带完整全局(document 等)的上下文里
  });
  const { window } = dom;
  window.localStorage.setItem("ziptool.locale", options.locale ?? "zh-CN");
  if (options.languages) {
    Object.defineProperty(window.navigator, "languages", {
      value: options.languages,
      configurable: true,
    });
  }
  if (options.userAgent) {
    Object.defineProperty(window.navigator, "userAgent", {
      value: options.userAgent,
      configurable: true,
    });
  }
  const calls = []; // { cmd, args }
  const callbacks = new Map(); // channel id → Rust 侧回调
  const eventListeners = new Map(); // event name → [listener callback id]
  let channelSeq = 1;

  if (withTauri) {
    window.__TAURI_INTERNALS__ = {
      transformCallback(cb) {
        const id = channelSeq++;
        callbacks.set(id, cb);
        return id;
      },
      unregisterCallback(id) {
        callbacks.delete(id);
      },
      invoke(cmd, args) {
        calls.push({ cmd, args });
        // 事件监听走真实 @tauri-apps/api/event 链路; 这里只记录回调 id 供 emit 回放
        if (cmd === "plugin:event|listen") {
          const ids = eventListeners.get(args.event) ?? [];
          ids.push(args.handler);
          eventListeners.set(args.event, ids);
          return Promise.resolve(channelSeq++);
        }
        if (cmd === "plugin:event|unlisten") return Promise.resolve(null);
        const result = invokeImpl({ cmd, args, callbacks, calls });
        // 本文件多数用例只关心压缩/解压; plan_packages 未显式 mock 时给默认方案
        if (cmd === "plan_packages" && (result === undefined || result === null)) {
          return Promise.resolve(defaultPlan(args));
        }
        return Promise.resolve(result);
      },
      convertFileSrc: (p) => p,
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
      plugins: {},
    };
    window.__TAURI__ = {
      core: coreApi,
      app: appApi,
      webview: webviewApi,
      dialog: dialogApi,
    };
    globalThis.__TAURI_INTERNALS__ = window.__TAURI_INTERNALS__;
  }
  // 真实包模块运行在 Node 全局, 其中的 window 自由变量解析到 globalThis;
  // 指向 JSDOM window 使 mock 传输对真实代码可见(同文件测试串行, 互不干扰)
  globalThis.window = window;
  window.eval(readFileSync(I18N_VENDOR, "utf8"));
  for (const url of I18N_LOCALES) window.eval(readFileSync(url, "utf8"));
  window.eval(readFileSync(I18N_JS, "utf8"));
  window.eval(readFileSync(APP_JS, "utf8"));
  const flush = () => new Promise((r) => setTimeout(r, 0));
  // 回放一条 Tauri 事件(拖拽事件监听由 app.js 通过真实 webview API 注册)
  const emit = async (event, payload) => {
    for (const id of eventListeners.get(event) ?? []) {
      callbacks.get(id)?.({ event, id, payload });
    }
    await flush();
  };
  return {
    window, doc: window.document, calls, callbacks, eventListeners, flush, emit,
    badges: () =>
      [...window.document.querySelectorAll("#rows td:last-child span")].map((b) => b.textContent),
    rows: () => [...window.document.querySelectorAll("#rows tr")],
  };
}

/** 选源(mock dialog 返回固定路径)并等扫描完成 */
async function pickSource(h, path = "C:\\demo\\data") {
  h.doc.getElementById("src").click();
  await h.flush();
  await h.flush();
  assert.equal(h.doc.getElementById("src").value, path);
}

/** 勾选「智能打包」并等 180ms 防抖的方案计算完成(默认关闭) */
async function enableSmart(h) {
  h.doc.getElementById("opt-smart").click();
  await sleep(260);
  await h.flush();
  await h.flush();
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** 通过语言菜单切换语言并等 changeLanguage/重渲染完成 */
async function switchLang(h, code) {
  h.doc.getElementById("lang-toggle").click();
  h.doc.querySelector(`#lang-menu [data-lang="${code}"]`).click();
  await h.flush();
  await h.flush();
}

/**
 * 模拟一次原生拖拽(enter → over → drop)。
 * target=null 表示落在两个投放区之外; jsdom 无布局, elementFromPoint 用 mock 指向目标行。
 */
async function dragDrop(h, target, paths, position = { x: 1, y: 1 }) {
  await h.flush(); // 等 app.js 通过真实 webview API 注册完 4 个 drag 监听
  const row = target ? h.doc.querySelector(`[data-drop="${target}"]`) : null;
  h.doc.elementFromPoint = () => row;
  await h.emit(TauriEvent.DRAG_ENTER, { paths, position });
  await h.emit(TauriEvent.DRAG_OVER, { position });
  await h.emit(TauriEvent.DRAG_DROP, { paths, position });
  await h.flush();
  await h.flush();
}

// 断言跨 realm 值: app.js 跑在 JSDOM 上下文, 其对象/数组的原型与 Node 侧字面量
// 不同 realm, assert.deepStrictEqual 会因原型引用不等而假失败 → 用 JSON 形状比较
const jsonEq = (a, b) => assert.equal(JSON.stringify(a), JSON.stringify(b));

test("契约: 有 Tauri 时扫描走真实表面, scan 命令参数与 Channel 序列化正确", async () => {
  const h = makeHarness(true, ({ cmd, args }) => {
    if (cmd === "plugin:app|version") return "1.2.3";
    if (cmd === "plugin:dialog|open") return "C:\\demo\\data";
    if (cmd === "scan") return SCAN_COMPRESS;
    return null;
  });
  await pickSource(h);

  // dialog 契约: 系统文件夹选择, 单选目录
  const dlg = h.calls.find((c) => c.cmd === "plugin:dialog|open");
  assert.ok(dlg, "应通过真实 plugin-dialog 打开选择框");
  assert.equal(dlg.args.options.directory, true);
  assert.equal(dlg.args.options.multiple, false);

  // scan 契约: 命令名 + mode/path + onScan 是真实 core.Channel 且序列化为 __CHANNEL__:<id>
  const scan = h.calls.find((c) => c.cmd === "scan");
  assert.ok(scan, "应调用 scan 命令");
  assert.equal(scan.args.mode, "compress");
  assert.equal(scan.args.path, "C:\\demo\\data");
  assert.ok(scan.args.onScan instanceof coreApi.Channel, "onScan 必须是 core.Channel 实例");
  assert.ok(!("onProgress" in scan.args), "scan 不再有 onProgress 参数");
  assert.match(JSON.stringify(scan.args), /__CHANNEL__:\d+/);

  // 渲染契约: 表格 3 行, 状态栏带总数, 版本号来自真实 app 表面
  assert.deepEqual(h.badges(), ["待压缩", "待压缩", "待压缩"]);
  assert.ok(h.doc.getElementById("msg").textContent.includes("3 个条目"));
  assert.equal(h.doc.getElementById("ver").textContent, "v1.2.3");
});

test("行为: 压缩全流程 — 勾选筛选、进度行状态、终态烘焙、选项透传", async () => {
  const prog = [];
  const h = makeHarness(true, ({ cmd, args, callbacks }) => {
    if (cmd === "plugin:dialog|open")
      return args.options.title?.includes("目标") ? "C:\\out" : "C:\\demo\\data";
    if (cmd === "scan") return SCAN_COMPRESS;
    if (cmd === "compress") {
      // Rust 侧逐条目回放进度(对象已解析, 与真实链路一致)
      sendProgress(callbacks, args.onProgress, 0, {
        phase: "compressing", done: 1, total: 2, doneBytes: 2_600_000_000,
        totalBytes: 2_600_204_800, entry: "Novel",
      });
      prog.push(...h.badges());
      return {
        cancelled: false,
        entries: [{ archive: "C:\\out\\Novel.zip", original: "Novel" }, { archive: "C:\\out\\photo.jpg.zip", original: "photo.jpg" }],
        warnings: ["skipped hidden file"],
      };
    }
    return null;
  });

  await pickSource(h);
  await enableSmart(h); // 智能打包 = 方案路径
  h.doc.getElementById("dst").click();
  await h.flush();
  await h.flush();
  // 取消第二行勾选(media) → 勾选变化会重算方案(确认在方案就绪前禁用)
  h.rows()[1].querySelector("input").click();
  await sleep(260);
  await h.flush();
  // 选项: 分包 2g + 密码(壳层负责解析, UI 只透传非空值)
  h.doc.getElementById("opt-split").value = " 2g ";
  h.doc.getElementById("opt-password").value = "pw";
  h.doc.getElementById("opt-random").click();
  h.doc.getElementById("confirm").click();
  await h.flush();
  await h.flush();

  // compress 契约: 名单 = 勾选项(表格序), 选项非空才透传
  const comp = h.calls.find((c) => c.cmd === "compress");
  assert.ok(comp, "应调用 compress 命令");
  jsonEq(comp.args.entries, ["Novel", "photo.jpg"]);
  jsonEq(comp.args.options, {
    splitSize: null,
    password: "pw",
    randomName: true,
    format: "zip",
    level: null, // 默认预设不传级别(后端默认 zip -6); Number("") 是 0=store, 空值必须显式归 null
    filters: { enabledPolicies: ["os-hidden", "lang-temp"] },
    excludes: [],
    verify: true,
    packaging: {
      mergeBelow: "100m",
      splitAbove: "2g",
      bundleMax: null,
      maxEntries: 50000,
    },
  });
  assert.equal(comp.args.planId, "test-plan");
  assert.equal(comp.args.src, "C:\\demo\\data");
  assert.equal(comp.args.dst, "C:\\out");

  // 运行中第一帧(进度 done=1): sel=[Novel, photo.jpg] → Novel 完成, photo.jpg 进行中
  assert.deepEqual(prog, ["完成", "待压缩", "进行中"]);
  // 终态: 勾选的两行完成, 未勾选保持待压缩; 告警走状态栏
  assert.deepEqual(h.badges(), ["完成", "待压缩", "完成"]);
  assert.ok(h.doc.getElementById("msg").textContent.includes("压缩完成: 2 个压缩包"));
  assert.ok(h.doc.getElementById("msg").textContent.includes("1 条告警"));
  // 完成行不再可勾选; 确认钮恢复但无新勾选可处理 → 禁用
  assert.ok(h.rows()[0].querySelector("input").disabled);
  assert.ok(h.doc.getElementById("confirm").disabled);
});

test("行为: 运行中「取消」— 确认钮变形、cancel_current 被调、部分完成保留", async () => {
  let resolveCompress;
  const h = makeHarness(true, ({ cmd, args, callbacks }) => {
    if (cmd === "plugin:dialog|open")
      return args.options.title?.includes("目标") ? "C:\\out" : "C:\\demo\\data";
    if (cmd === "scan") return SCAN_COMPRESS;
    if (cmd === "compress")
      return new Promise((res) => {
        resolveCompress = res;
        sendProgress(callbacks, args.onProgress, 0, {
          phase: "compressing", done: 1, total: 2, doneBytes: 1, totalBytes: 2, entry: "Novel",
        });
      });
    if (cmd === "cancel_current") return null;
    return null;
  });

  await pickSource(h);
  h.doc.getElementById("dst").click();
  await h.flush();
  await h.flush();
  h.doc.getElementById("confirm").click();
  await h.flush();

  // 运行中: 确认钮变取消(红), 其余行待处理, 第一条已完成
  assert.equal(h.doc.getElementById("confirm").textContent, "取消");
  assert.ok(h.doc.getElementById("confirm").className.includes("bg-red-600"));
  assert.deepEqual(h.badges(), ["完成", "进行中", "待压缩"]);
  assert.ok(h.doc.getElementById("src").disabled, "任务期锁控件");

  h.doc.getElementById("confirm").click(); // → 取消
  await h.flush();
  assert.ok(h.calls.some((c) => c.cmd === "cancel_current"), "应通知外壳取消");

  resolveCompress({ cancelled: true, entries: [], warnings: [] });
  await h.flush();
  await h.flush();
  assert.equal(h.doc.getElementById("confirm").textContent, "确认");
  assert.ok(h.doc.getElementById("msg").textContent.includes("已取消(完成 1/2)"));
  assert.deepEqual(h.badges(), ["完成", "待压缩", "待压缩"], "取消后未完成的行回到待处理");
  assert.ok(h.doc.getElementById("confirm").className.includes("bg-blue-600"));
});

test("行为: 压缩命令失败 — 当前行标失败, 徽章悬停看原因", async () => {
  const h = makeHarness(true, ({ cmd, args }) => {
    if (cmd === "plugin:dialog|open")
      return args.options.title?.includes("目标") ? "C:\\out" : "C:\\demo\\data";
    if (cmd === "scan") return SCAN_COMPRESS;
    if (cmd === "compress") return Promise.reject("entry disappeared after scan: Novel");
    return null;
  });
  await pickSource(h);
  h.doc.getElementById("dst").click();
  await h.flush();
  await h.flush();
  h.doc.getElementById("confirm").click();
  await h.flush();
  await h.flush();

  const badge = h.rows()[0].querySelector("td:last-child span");
  assert.equal(badge.textContent, "失败");
  assert.equal(badge.title, "entry disappeared after scan: Novel");
  assert.ok(h.doc.getElementById("msg").textContent.includes("压缩失败"));
});

test("行为: 解压模式 — 选项显隐、meta/warning 行、extract_archive 参数", async () => {
  const h = makeHarness(true, ({ cmd, args, callbacks }) => {
    if (cmd === "plugin:dialog|open")
      return args.options.title?.includes("目标") ? "C:\\out" : "C:\\demo\\data";
    if (cmd === "scan") return SCAN_EXTRACT;
    if (cmd === "extract_archive") {
      sendProgress(callbacks, args.onProgress, 0, {
        phase: "extracting", done: 1, total: 1, doneBytes: 1, totalBytes: 1, entry: "Novel.zip",
      });
      return { cancelled: false, groups: [{ entry: "Novel.zip", original: "Novel", files: 5 }], warnings: [] };
    }
    return null;
  });

  h.doc.getElementById("mode-extract").click();
  assert.ok(h.doc.getElementById("strategy-row").classList.contains("hidden"));
  assert.ok(h.doc.getElementById("opt-random-wrap").classList.contains("hidden"));
  assert.ok(!h.doc.getElementById("opt-force-wrap").classList.contains("hidden"));

  await pickSource(h);
  assert.deepEqual(
    h.badges(),
    ["待解压", "待解压", "待解压", "元数据", "告警"],
    "组行 + 元数据行 + 告警行"
  );
  // meta/warning 行禁勾; 告警行悬停看原因; 组行悬停看原名
  const checkboxes = h.rows().map((tr) => tr.querySelector("input"));
  assert.ok(checkboxes[3].disabled && checkboxes[4].disabled);
  assert.equal(h.rows()[4].querySelector("td:last-child span").title, "非压缩包文件, 已跳过");
  assert.match(h.rows()[0].querySelector("td:nth-child(2)").title, /原名 Novel/);

  // 默认全选(pending 行)→ 取消后两组, 只留第一组 + 勾覆盖
  checkboxes[1].click();
  checkboxes[2].click();
  h.doc.getElementById("opt-force").click();
  h.doc.getElementById("dst").click();
  await h.flush();
  await h.flush();
  h.doc.getElementById("confirm").click();
  await h.flush();
  await h.flush();

  const ext = h.calls.find((c) => c.cmd === "extract_archive");
  assert.ok(ext, "应调用 extract_archive 命令");
  jsonEq(ext.args.entries, ["Novel.zip"]);
  jsonEq(ext.args.options, { password: null, force: true });
  assert.deepEqual(h.badges(), ["完成", "待解压", "待解压", "元数据", "告警"]);
  assert.ok(h.doc.getElementById("msg").textContent.includes("解压完成: 1 组"));
});

test("行为: 模式切换作废旧扫描(源清空、表格清空、提示重扫)", async () => {
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "plugin:dialog|open") return "C:\\demo\\data";
    if (cmd === "scan") return SCAN_COMPRESS;
    return null;
  });
  await pickSource(h);
  assert.equal(h.rows().length, 3);

  h.doc.getElementById("mode-extract").click();
  await h.flush();
  assert.equal(h.doc.getElementById("src").value, "");
  assert.equal(h.rows().length, 0);
  assert.ok(h.doc.getElementById("msg").textContent.includes("模式已切换"));
});

// ———— 智能打包开关 ————
test("智能打包: 默认关闭 — 策略/打包列隐藏, 不算方案, compress 传 null 打包参数", async () => {
  let planCalls = 0;
  const h = makeHarness(true, ({ cmd, args }) => {
    if (cmd === "plugin:dialog|open")
      return args.options.title?.includes("目标") ? "C:\\out" : "C:\\demo\\data";
    if (cmd === "scan") return SCAN_COMPRESS;
    if (cmd === "plan_packages") {
      planCalls += 1;
      return defaultPlan(args);
    }
    if (cmd === "compress") return { cancelled: false, entries: [], warnings: [] };
    return null;
  });
  const el = (id) => h.doc.getElementById(id);
  assert.equal(el("opt-smart").checked, false, "默认不勾选");
  assert.ok(el("strategy-row").classList.contains("hidden"), "策略行隐藏");
  assert.ok(el("th-package").classList.contains("hidden"), "打包列表头隐藏");

  // 阈值改动也不得触发方案计算(未勾选时没有方案概念)
  const merge = el("opt-merge-below");
  merge.value = "50m";
  merge.dispatchEvent(new h.window.Event("input", { bubbles: true }));
  await sleep(260);
  assert.equal(planCalls, 0, "未勾选时阈值变化不得调用 plan_packages");

  await pickSource(h);
  assert.equal(planCalls, 0, "未勾选时扫描后不得调用 plan_packages");
  assert.equal(h.rows().length, 3);
  assert.ok(
    h.rows().every((tr) => tr.children[3].classList.contains("hidden")),
    "行内打包列隐藏"
  );
  assert.equal(el("plan-summary").textContent, "");
  assert.ok(el("plan-open").disabled);

  el("dst").click();
  await h.flush();
  await h.flush();
  el("confirm").click();
  await h.flush();
  await h.flush();
  const comp = h.calls.find((c) => c.cmd === "compress");
  assert.ok(comp, "应调用 compress");
  assert.equal(comp.args.options.packaging, null, "未勾选时 packaging=null");
  assert.equal(comp.args.planId, null, "未勾选时 planId=null");
  assert.equal(comp.args.options.splitSize, null, "仍走逐条目压缩路径");
  jsonEq(comp.args.entries, ["Novel", "media", "photo.jpg"]);
});

test("智能打包: 勾选后恢复方案(摘要/抽屉/确认校验), 取消勾选清方案", async () => {
  let planCalls = 0;
  const h = makeHarness(true, ({ cmd, args }) => {
    if (cmd === "plugin:dialog|open") return "C:\\demo\\data";
    if (cmd === "scan") return SCAN_COMPRESS;
    if (cmd === "plan_packages") {
      planCalls += 1;
      return defaultPlan(args);
    }
    return null;
  });
  const el = (id) => h.doc.getElementById(id);
  await pickSource(h);
  assert.equal(planCalls, 0, "默认关闭时扫描后不算方案");

  // 勾选: 立即显示策略行/打包列, 方案计算中禁用确认
  el("opt-smart").click();
  assert.ok(!el("strategy-row").classList.contains("hidden"), "策略行显示");
  assert.ok(!el("th-package").classList.contains("hidden"), "打包列显示");
  assert.equal(el("confirm").disabled, true, "方案计算中确认禁用");
  await sleep(260);
  await h.flush();
  assert.equal(planCalls, 1, "勾选后计算一次方案");
  assert.equal(el("confirm").disabled, false, "方案就绪后可确认");
  assert.match(el("plan-summary").textContent, /预计/);
  assert.equal(el("plan-open").disabled, false);
  assert.deepEqual(
    h.rows().map((tr) => tr.children[3].textContent),
    ["分包", "直接", "直接"]
  );

  el("plan-open").click();
  assert.ok(!el("plan-backdrop").classList.contains("hidden"), "可打开方案抽屉");
  el("plan-close").click();
  assert.ok(el("plan-backdrop").classList.contains("hidden"));

  // 再取消勾选: 清方案/隐藏策略行与打包列, 不再请求方案, 确认恢复可用
  el("opt-smart").click();
  await sleep(260);
  assert.equal(planCalls, 1, "取消勾选不得再调用 plan_packages");
  assert.ok(el("strategy-row").classList.contains("hidden"));
  assert.ok(el("th-package").classList.contains("hidden"));
  assert.ok(h.rows().every((tr) => tr.children[3].classList.contains("hidden")));
  assert.equal(el("plan-summary").textContent, "");
  assert.equal(el("confirm").disabled, false, "无方案也可确认");
});

// ———— 扫描不锁 UI: 只锁确认钮 + 流式表格 ————
test("扫描中: 只锁确认钮(spinner/aria-busy/文案), 骨架行 + 大小列 spinner, row 事件填大小", async () => {
  const ITEMS = [
    { name: "Novel", kind: "dir", status: "pending", original: null, detail: null, excluded: null },
    { name: "media", kind: "dir", status: "pending", original: null, detail: null, excluded: null },
    { name: "photo.jpg", kind: "file", status: "pending", original: null, detail: null, excluded: null },
  ];
  let finishScan;
  let scanArgs;
  const h = makeHarness(true, ({ cmd, args, callbacks }) => {
    if (cmd === "load_settings") return { settings: SETTINGS_DEFAULTS, warnings: [] };
    if (cmd === "plugin:dialog|open") return "C:\\demo\\data";
    if (cmd === "scan") {
      scanArgs = args;
      sendProgress(callbacks, args.onScan, 0, { event: "rows", mode: "compress", items: ITEMS });
      return new Promise((resolve) => (finishScan = resolve));
    }
    return null;
  });
  await h.flush();
  await h.flush();
  h.doc.getElementById("src").click();
  await h.flush();
  await h.flush();

  const confirm = h.doc.getElementById("confirm");
  assert.ok(confirm.disabled, "扫描中确认钮禁用");
  assert.equal(confirm.getAttribute("aria-busy"), "true");
  assert.ok(confirm.querySelector("span.animate-spin"), "确认钮内 spinner");
  assert.ok(confirm.textContent.includes("扫描中…"), confirm.textContent);
  // 其它控件全部可用(只锁确认钮)
  assert.equal(h.doc.getElementById("src").disabled, false);
  assert.equal(h.doc.getElementById("dst").disabled, false);
  assert.equal(h.doc.getElementById("mode-extract").disabled, false);
  assert.equal(h.doc.getElementById("opt-smart").disabled, false);
  assert.equal(h.doc.getElementById("opt-password").disabled, false);
  assert.equal(h.doc.getElementById("menu-settings-open").disabled, false);

  // rows 事件: 骨架表格 + 大小列 spinner + 默认勾选 pending 行
  assert.equal(h.rows().length, 3);
  assert.ok(
    h.rows()[0].querySelector('[data-cell="size"] span.animate-spin'),
    "大小列 spinner"
  );
  assert.ok(h.rows().every((tr) => tr.querySelector("input").checked));
  assert.ok(h.doc.getElementById("msg").textContent.includes("扫描中 0/3"));
  // 扫描期间允许勾选行(最终渲染按行下标保留)
  h.rows()[1].querySelector("input").click();
  assert.equal(h.rows()[1].querySelector("input").checked, false);
  assert.ok(confirm.disabled);

  // row 事件: 原地替换大小(按当前语言格式化) + 进度推进
  sendProgress(h.callbacks, scanArgs.onScan, 1, { event: "row", index: 0, size: 2_600_000_000 });
  assert.equal(h.rows()[0].children[2].textContent, "2.42 GB");
  assert.ok(h.doc.getElementById("msg").textContent.includes("扫描中 1/3 Novel"));
  sendProgress(h.callbacks, scanArgs.onScan, 2, { event: "row", index: 2, size: 4096 });
  assert.equal(h.rows()[2].children[2].textContent, "4.0 KB");
  assert.ok(confirm.disabled, "大小填完也仍在扫描中");

  finishScan(SCAN_COMPRESS);
  for (let i = 0; i < 4; i += 1) await h.flush();
  assert.equal(confirm.textContent, "确认");
  assert.equal(confirm.hasAttribute("aria-busy"), false);
  assert.equal(confirm.disabled, false, "扫描结束后恢复确认");
  assert.ok(h.doc.getElementById("msg").textContent.includes("3 个条目"));
  assert.equal(h.rows()[1].querySelector("input").checked, false, "扫描期间的勾选按行下标保留");
});

test("扫描流式: 重新选择源文件夹 → 旧扫描结果/事件丢弃, 新扫描生效", async () => {
  const OLD_ITEMS = [
    { name: "old", kind: "file", status: "pending", original: null, detail: null, excluded: null },
  ];
  const scans = [];
  const pending = [];
  const h = makeHarness(true, ({ cmd, args, callbacks }) => {
    if (cmd === "plugin:dialog|open") return "C:\\demo\\data";
    if (cmd === "scan") {
      scans.push(args);
      if (scans.length === 1) {
        sendProgress(callbacks, args.onScan, 0, { event: "rows", mode: "compress", items: OLD_ITEMS });
      }
      return new Promise((resolve, reject) => pending.push({ resolve, reject }));
    }
    return null;
  });

  h.doc.getElementById("src").click();
  await h.flush();
  await h.flush();
  assert.equal(h.rows().length, 1, "第一轮扫描骨架");
  assert.ok(h.doc.getElementById("msg").textContent.includes("扫描中 0/1"));
  const confirm = h.doc.getElementById("confirm");
  assert.equal(confirm.getAttribute("aria-busy"), "true");

  // 扫描中再选源 → 壳层自动取消旧扫描, 前端直接发起新扫描
  h.doc.getElementById("src").click();
  await h.flush();
  await h.flush();
  assert.equal(scans.length, 2, "应发起第二次 scan");
  assert.equal(h.rows().length, 0, "新扫描清空旧骨架");
  assert.ok(confirm.disabled);

  // 旧扫描 reject(被壳层取消)不得改状态
  pending[0].reject("cancelled by shell");
  await h.flush();
  await h.flush();
  assert.equal(h.rows().length, 0, "旧扫描失败不得回填/清空新扫描");
  assert.ok(!h.doc.getElementById("msg").textContent.includes("扫描失败"));
  assert.ok(confirm.disabled, "仍处于新扫描中");

  // 旧扫描晚到的 rows 事件也按 scanSeq 丢弃
  sendProgress(h.callbacks, scans[0].onScan, 1, {
    event: "rows",
    mode: "compress",
    items: OLD_ITEMS,
  });
  assert.equal(h.rows().length, 0, "旧 channel 事件不得渲染");

  pending[1].resolve(SCAN_COMPRESS);
  for (let i = 0; i < 4; i += 1) await h.flush();
  assert.equal(h.rows().length, 3, "新扫描结果生效");
  assert.ok(h.doc.getElementById("msg").textContent.includes("3 个条目"));
  assert.equal(confirm.textContent, "确认");
});

test("扫描中切换模式: 作废当前扫描并恢复确认钮", async () => {
  const ITEMS = [
    { name: "Novel", kind: "dir", status: "pending", original: null, detail: null, excluded: null },
    { name: "media", kind: "dir", status: "pending", original: null, detail: null, excluded: null },
  ];
  let finishScan;
  const h = makeHarness(true, ({ cmd, args, callbacks }) => {
    if (cmd === "plugin:dialog|open") return "C:\\demo\\data";
    if (cmd === "scan") {
      sendProgress(callbacks, args.onScan, 0, { event: "rows", mode: "compress", items: ITEMS });
      return new Promise((resolve) => (finishScan = resolve));
    }
    return null;
  });
  h.doc.getElementById("src").click();
  await h.flush();
  await h.flush();
  assert.equal(h.rows().length, 2);
  const confirm = h.doc.getElementById("confirm");
  assert.equal(confirm.getAttribute("aria-busy"), "true");

  h.doc.getElementById("mode-extract").click();
  await h.flush();
  assert.equal(h.doc.getElementById("src").value, "");
  assert.equal(h.rows().length, 0);
  assert.ok(h.doc.getElementById("msg").textContent.includes("模式已切换"));
  assert.equal(confirm.textContent, "确认", "确认钮恢复常规文案");
  assert.equal(confirm.hasAttribute("aria-busy"), false);
  assert.equal(confirm.disabled, true, "无勾选行");

  finishScan(SCAN_COMPRESS);
  for (let i = 0; i < 4; i += 1) await h.flush();
  assert.equal(h.rows().length, 0, "旧扫描结果不得回填");
  assert.ok(h.doc.getElementById("msg").textContent.includes("模式已切换"), "旧扫描不得改状态");
});

test("扫描失败: 清表、恢复确认钮、状态栏报错, 不锁 UI", async () => {
  let rejectScan;
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "load_settings") return { settings: SETTINGS_DEFAULTS, warnings: [] };
    if (cmd === "plugin:dialog|open") return "C:\\demo\\data";
    if (cmd === "scan") return new Promise((_resolve, reject) => (rejectScan = reject));
    return null;
  });
  h.doc.getElementById("src").click();
  await h.flush();
  await h.flush();
  const confirm = h.doc.getElementById("confirm");
  assert.equal(confirm.getAttribute("aria-busy"), "true");

  rejectScan("permission denied");
  for (let i = 0; i < 4; i += 1) await h.flush();
  assert.equal(h.rows().length, 0, "失败后清空表格");
  assert.ok(h.doc.getElementById("msg").textContent.includes("扫描失败"));
  assert.equal(confirm.textContent, "确认");
  assert.equal(confirm.hasAttribute("aria-busy"), false);
  assert.equal(h.doc.getElementById("src").disabled, false, "扫描失败也不锁 UI");
  assert.equal(h.doc.getElementById("mode-extract").disabled, false);
  assert.equal(h.doc.getElementById("menu-settings-open").disabled, false);
});

test("扫描中: 勾选行不会用未完成大小算方案, 扫描结束后再算", async () => {
  let planCalls = 0;
  let finishScan;
  const h = makeHarness(true, ({ cmd, args, callbacks }) => {
    if (cmd === "plugin:dialog|open") return "C:\\demo\\data";
    if (cmd === "plan_packages") {
      planCalls += 1;
      return defaultPlan(args);
    }
    if (cmd === "scan") {
      sendProgress(callbacks, args.onScan, 0, {
        event: "rows",
        mode: "compress",
        items: [
          { name: "Novel", kind: "dir", status: "pending", original: null, detail: null, excluded: null },
          { name: "media", kind: "dir", status: "pending", original: null, detail: null, excluded: null },
          { name: "photo.jpg", kind: "file", status: "pending", original: null, detail: null, excluded: null },
        ],
      });
      return new Promise((resolve) => (finishScan = resolve));
    }
    return null;
  });
  h.doc.getElementById("opt-smart").click(); // 扫描前开启智能打包
  await sleep(260);
  await h.flush();
  h.doc.getElementById("src").click();
  await h.flush();
  await h.flush();
  assert.equal(planCalls, 0, "扫描中不算方案");

  h.rows()[1].querySelector("input").click(); // 扫描中改勾选
  await sleep(260);
  await h.flush();
  assert.equal(planCalls, 0, "扫描中勾选变化不得调用 plan_packages");

  finishScan(SCAN_COMPRESS);
  for (let i = 0; i < 5; i += 1) await h.flush();
  assert.equal(planCalls, 1, "扫描结束后算一次方案");
  assert.equal(h.doc.getElementById("confirm").disabled, false);
});

test("浏览器预览(无 Tauri): mock 扫描与 mock 运行仍可用", async () => {
  const h = makeHarness(false, () => {
    throw new Error("无 Tauri 环境不应触碰 IPC");
  });
  await pickSource(h);
  assert.deepEqual(
    h.badges(),
    ["待压缩", "待压缩", "待压缩", "待压缩", "待压缩"],
    "mock 含隐藏/命中行(显示不勾选)"
  );
  const boxes = h.rows().map((tr) => tr.querySelector("input"));
  assert.ok(!boxes[3].checked && !boxes[4].checked, "被标行默认不勾选");
  assert.ok(h.doc.getElementById("msg").textContent.includes("浏览器预览假数据"));
  assert.equal(h.doc.getElementById("ver").textContent, "dev");

  h.doc.getElementById("dst").click();
  await h.flush();
  h.doc.getElementById("confirm").click();
  // mock 每 400ms 一条进度; 勾选名单 = 前 3 行(被标行未勾选), 3 条约 1.2s
  await new Promise((r) => setTimeout(r, 500));
  const mid = h.badges();
  await new Promise((r) => setTimeout(r, 1600));
  assert.deepEqual(mid, ["完成", "进行中", "待压缩", "待压缩", "待压缩"]);
  assert.deepEqual(h.badges(), ["完成", "完成", "完成", "待压缩", "待压缩"]);
  assert.ok(h.doc.getElementById("msg").textContent.includes("压缩完成: 3 个压缩包"));
});

test("行为: 首次打开(未点过模式)— 压缩模式控件初始可见, 无需切换模式触发", () => {
  // 回归: opt-preset-wrap / opt-advanced-wrap 初始 hidden 只能被 setMode 解除,
  // 启动时不调 setMode 的话, 第一次打开要切去解压再切回来才看得到
  const h = makeHarness(false, () => null);
  assert.ok(!h.doc.getElementById("opt-preset-wrap").classList.contains("hidden"), "「模式」预设开关初始可见");
  assert.ok(!h.doc.getElementById("opt-advanced-wrap").classList.contains("hidden"), "「高级」checkbox 初始可见");
  // 智能打包默认关闭: 策略行/打包列初始隐藏, 开关本身在压缩模式常显
  const smart = h.doc.getElementById("opt-smart");
  assert.ok(!smart.checked, "智能打包默认不勾选");
  assert.ok(!h.doc.getElementById("opt-smart-wrap").classList.contains("hidden"), "智能打包开关压缩模式可见");
  assert.ok(h.doc.getElementById("strategy-row").classList.contains("hidden"), "策略行默认隐藏");
  assert.ok(h.doc.getElementById("th-package").classList.contains("hidden"), "打包列表头默认隐藏");
  assert.ok(h.doc.getElementById("opt-force-wrap").classList.contains("hidden"), "解压专属「覆盖」初始隐藏");
  assert.ok(h.doc.getElementById("opt-format-wrap").classList.contains("hidden"), "高级子项仍等勾选展开");
});

test("行为: 压缩模式预设 — 默认最大兼容性, 预设联动参数, 手动改参数变自定义", async () => {
  const h = makeHarness(false, () => null);
  // 默认 = 最大兼容性预设(zip + 后端默认强度; 9 太慢, 强度留空 = 不传级别)
  assert.equal(h.doc.getElementById("opt-format").value, "zip");
  assert.equal(h.doc.getElementById("opt-level").value, "");
  assert.ok(h.doc.getElementById("opt-preset-custom").classList.contains("hidden"));
  assert.ok(h.doc.getElementById("preset-compat").className.includes("bg-blue-600"));

  // 切平衡预设 → 底层参数联动
  h.doc.getElementById("preset-balanced").click();
  assert.equal(h.doc.getElementById("opt-format").value, "7z");
  assert.equal(h.doc.getElementById("opt-level").value, "5");
  assert.ok(h.doc.getElementById("preset-balanced").className.includes("bg-blue-600"));
  assert.ok(!h.doc.getElementById("preset-compat").className.includes("bg-blue-600"));

  // 手动改强度 → 自定义(高亮清除)
  const lvl = h.doc.getElementById("opt-level");
  lvl.value = "2";
  lvl.dispatchEvent(new h.window.Event("input", { bubbles: true }));
  assert.ok(!h.doc.getElementById("opt-preset-custom").classList.contains("hidden"));
  assert.ok(!h.doc.getElementById("preset-balanced").className.includes("bg-blue-600"));

  // 高级区显隐开关
  assert.ok(h.doc.getElementById("opt-format-wrap").classList.contains("hidden"));
  h.doc.getElementById("opt-advanced").click();
  assert.ok(!h.doc.getElementById("opt-format-wrap").classList.contains("hidden"));
  assert.ok(!h.doc.getElementById("opt-level-wrap").classList.contains("hidden"));

  // 解压模式下预设/高级控件隐藏, 切回压缩恢复
  h.doc.getElementById("mode-extract").click();
  assert.ok(h.doc.getElementById("opt-preset-wrap").classList.contains("hidden"));
  h.doc.getElementById("mode-compress").click();
  assert.ok(!h.doc.getElementById("opt-preset-wrap").classList.contains("hidden"));
});

test("契约: 平衡预设的参数透传到 compress 调用", async () => {
  const h = makeHarness(true, ({ cmd, args }) => {
    if (cmd === "plugin:dialog|open")
      return args.options.title?.includes("目标") ? "C:\\out" : "C:\\demo\\data";
    if (cmd === "scan") return SCAN_COMPRESS;
    if (cmd === "compress") return { cancelled: false, entries: [], warnings: [] };
    return null;
  });
  await pickSource(h);
  await enableSmart(h); // 打包参数只在智能打包开启时透传
  h.doc.getElementById("dst").click();
  await h.flush();
  await h.flush();
  h.doc.getElementById("preset-balanced").click();
  h.doc.getElementById("confirm").click();
  await h.flush();
  await h.flush();
  const comp = h.calls.find((c) => c.cmd === "compress");
  assert.ok(comp, "应调用 compress");
  assert.equal(comp.args.planId, "test-plan");
  jsonEq(comp.args.options, {
    splitSize: null,
    password: null,
    randomName: false,
    format: "7z",
    level: 5,
    filters: { enabledPolicies: ["os-hidden", "lang-temp"] },
    excludes: [],
    verify: true,
    packaging: {
      mergeBelow: "100m",
      splitAbove: "2g",
      bundleMax: null,
      maxEntries: 50000,
    },
  });
});

test("行为: 压缩成功后校验默认开启, 关闭后透传 verify=false, 解压模式隐藏", async () => {
  const h = makeHarness(true, ({ cmd, args }) => {
    if (cmd === "plugin:dialog|open")
      return args.options.title?.includes("目标") ? "C:\\out" : "C:\\demo\\data";
    if (cmd === "scan") return SCAN_COMPRESS;
    if (cmd === "compress") return { cancelled: false, entries: [], warnings: [] };
    return null;
  });

  const box = h.doc.getElementById("opt-verify");
  const wrap = h.doc.getElementById("opt-verify-wrap");
  assert.ok(box.checked, "默认开启");
  assert.ok(!wrap.classList.contains("hidden"), "压缩模式可见");

  h.doc.getElementById("mode-extract").click();
  assert.ok(wrap.classList.contains("hidden"), "解压模式隐藏");
  h.doc.getElementById("mode-compress").click();
  assert.ok(!wrap.classList.contains("hidden"), "切回压缩模式恢复可见");

  box.click(); // 关闭校验
  await pickSource(h);
  h.doc.getElementById("dst").click();
  await h.flush();
  await h.flush();
  h.doc.getElementById("confirm").click();
  await h.flush();
  await h.flush();

  const comp = h.calls.find((c) => c.cmd === "compress");
  assert.ok(comp, "应调用 compress");
  assert.equal(comp.args.options.verify, false);
});

test("行为: 密码提示随格式切换(zip=ZipCrypto / 7z=AES-256)", async () => {
  const h = makeHarness(false, () => null);
  const hint = h.doc.getElementById("pw-hint");
  h.doc.getElementById("opt-password").value = "pw";
  h.doc.getElementById("opt-password").dispatchEvent(
    new h.window.Event("input", { bubbles: true })
  );
  assert.ok(!hint.classList.contains("hidden"), "zip+密码应有提示");
  assert.ok(hint.textContent.includes("ZipCrypto"), hint.textContent);

  // 切平衡预设 → 参数与提示一起联动(无需手动触发 change)
  h.doc.getElementById("preset-balanced").click();
  assert.ok(hint.textContent.includes("AES-256"), hint.textContent);

  h.doc.getElementById("opt-password").value = "";
  h.doc.getElementById("opt-password").dispatchEvent(
    new h.window.Event("input", { bubbles: true })
  );
  assert.ok(hint.classList.contains("hidden"), "无密码时提示隐藏");
});

// 带排除标记的扫描结果(与壳层 DTO 的 excluded 字段一致)
const SCAN_EXCLUDED = {
  mode: "compress",
  items: [
    ITEM("Novel", "dir", 2_600_000_000, "pending"),
    ITEM(".DS_Store", "file", 4096, "pending", { excluded: "隐藏文件" }),
    ITEM("cache", "dir", 12_000, "pending", { excluded: "匹配排除模式: cache" }),
  ],
  warnings: [],
  totalBytes: 2_600_004_096,
};

test("行为: 排除行显示但不勾选, 模式输入透传 scan/compress, 勾选可覆盖", async () => {
  const h = makeHarness(true, ({ cmd, args }) => {
    if (cmd === "plugin:dialog|open")
      return args.options.title?.includes("目标") ? "C:\\out" : "C:\\demo\\data";
    if (cmd === "scan") {
      // 跨 realm 数组 → 用 JSON 形状比较(见下方 jsonEq 注释)
      jsonEq(args.excludes, ["Thumbs.db", "*.tmp"]);
      return SCAN_EXCLUDED;
    }
    if (cmd === "compress") return { cancelled: false, entries: [], warnings: [] };
    return null;
  });

  // 高级区填排除模式(换行/逗号混排, 带空白)再扫描
  h.doc.getElementById("opt-advanced").click();
  h.doc.getElementById("opt-exclude").value = " Thumbs.db,\n*.tmp ";
  await pickSource(h);

  // 被标行显示(待压缩徽章)但默认不勾选; 名字悬停看原因
  assert.deepEqual(h.badges(), ["待压缩", "待压缩", "待压缩"]);
  const boxes = h.rows().map((tr) => tr.querySelector("input"));
  assert.ok(boxes[0].checked, "普通行默认勾选");
  assert.ok(!boxes[1].checked, "隐藏行默认不勾选");
  assert.ok(!boxes[2].checked, "模式命中行默认不勾选");
  assert.match(h.rows()[1].querySelector("td:nth-child(2)").title, /隐藏文件/);
  assert.match(h.rows()[2].querySelector("td:nth-child(2)").title, /匹配排除模式: cache/);

  // 全选切换覆盖所有 pending 行(含被标行); 手动勾选了就会被压缩
  h.doc.getElementById("check-all").click();
  assert.ok(h.rows().every((tr) => tr.querySelector("input").checked));

  h.doc.getElementById("dst").click();
  await h.flush();
  await h.flush();
  h.doc.getElementById("confirm").click();
  await h.flush();
  await h.flush();
  const comp = h.calls.find((c) => c.cmd === "compress");
  assert.ok(comp, "应调用 compress");
  jsonEq(comp.args.entries, ["Novel", ".DS_Store", "cache"]);
  jsonEq(comp.args.options.excludes, ["Thumbs.db", "*.tmp"]);
});

test("契约: 原生拖拽文件夹 — validate_dir 校验 + 源自动扫描 / 目标只写路径", async () => {
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "plugin:app|version") return "1.2.3";
    if (cmd === "validate_dir") return null;
    if (cmd === "scan") return SCAN_COMPRESS;
    return null;
  });

  await dragDrop(h, "src", ["D:\\data"]);
  const vd = h.calls.find((c) => c.cmd === "validate_dir");
  assert.ok(vd, "拖入后应调用 validate_dir");
  jsonEq(vd.args, { path: "D:\\data" });
  assert.equal(h.doc.getElementById("src").value, "D:\\data");
  const scan = h.calls.find((c) => c.cmd === "scan");
  assert.ok(scan, "源拖入后应自动扫描");
  assert.equal(scan.args.path, "D:\\data");
  assert.deepEqual(h.badges(), ["待压缩", "待压缩", "待压缩"]);

  await dragDrop(h, "dst", ["E:\\out"]);
  assert.equal(h.doc.getElementById("dst").value, "E:\\out");
  assert.equal(h.calls.filter((c) => c.cmd === "scan").length, 1, "目标拖入不应重扫");
  const validations = h.calls.filter((c) => c.cmd === "validate_dir");
  assert.equal(validations.length, 2, "源/目标各自校验一次");
  jsonEq(validations[1].args, { path: "E:\\out" });
});

test("行为: 拖拽高亮/离开、非文件夹/多项/落点外/锁定拒绝", async () => {
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "validate_dir") return Promise.reject("只能拖入文件夹");
    return null;
  });
  await h.flush();

  // 拖入高亮 + 占位提示, 离开恢复
  const srcRow = h.doc.querySelector('[data-drop="src"]');
  h.doc.elementFromPoint = () => srcRow;
  await h.emit(TauriEvent.DRAG_ENTER, { paths: ["D:\\a.txt"], position: { x: 1, y: 1 } });
  assert.ok(srcRow.classList.contains("bg-blue-50"), "拖入时整行高亮");
  assert.ok(h.doc.getElementById("src").classList.contains("bg-blue-50"), "输入框高亮");
  assert.equal(h.doc.getElementById("src").placeholder, "松开鼠标以选择此文件夹");
  await h.emit(TauriEvent.DRAG_LEAVE, {});
  await h.flush();
  assert.ok(!srcRow.classList.contains("bg-blue-50"), "离开后清除高亮");
  assert.ok(!h.doc.getElementById("src").classList.contains("bg-blue-50"));
  assert.notEqual(h.doc.getElementById("src").placeholder, "松开鼠标以选择此文件夹");

  // 校验失败(文件): 不落值
  await dragDrop(h, "src", ["D:\\a.txt"]);
  assert.equal(h.doc.getElementById("src").value, "");
  assert.ok(h.doc.getElementById("msg").textContent.includes("无法使用拖入的文件夹"));

  // 多项: 不校验、不落值
  await dragDrop(h, "src", ["D:\\a", "D:\\b"]);
  assert.ok(h.doc.getElementById("msg").textContent.includes("一次只能拖入一个文件夹"));
  assert.equal(h.calls.filter((c) => c.cmd === "validate_dir").length, 1);

  // 落点外: 给出提示
  await dragDrop(h, null, ["D:\\a"]);
  assert.ok(h.doc.getElementById("msg").textContent.includes("请将文件夹拖到"));

  // 任务锁定: 不落值并提示
  h.doc.getElementById("dst").disabled = true;
  await dragDrop(h, "dst", ["E:\\out"]);
  assert.equal(h.doc.getElementById("dst").value, "");
  assert.ok(h.doc.getElementById("msg").textContent.includes("任务进行中"));
});

test("i18n: 切换语言重渲染最后状态与按钮", async () => {
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "plugin:dialog|open") return "C:\\demo\\data";
    if (cmd === "scan") return SCAN_COMPRESS;
    return null;
  });
  await pickSource(h);

  h.window.i18next.addResourceBundle(
    "en",
    "translation",
    {
      status: {
        scanDone_one: "Scanned {{path}}: {{count}} item, {{bytes}}",
        scanDone_other: "Scanned {{path}}: {{count}} items, {{bytes}}",
      },
      header: { confirm: "Confirm" },
    },
    true,
    true
  );

  await switchLang(h, "en");

  assert.equal(h.doc.documentElement.lang, "en");
  assert.ok(h.doc.getElementById("lang-menu").classList.contains("hidden"));
  assert.equal(h.doc.getElementById("lang-toggle").getAttribute("aria-expanded"), "false");
  assert.equal(
    h.doc.querySelector('#lang-menu [data-lang="en"]').getAttribute("aria-checked"),
    "true"
  );
  assert.equal(
    h.doc.getElementById("msg").textContent,
    "Scanned C:\\demo\\data: 3 items, 3.25 GB"
  );
  assert.deepEqual(h.badges(), [
    "Pending compression",
    "Pending compression",
    "Pending compression",
  ]);
  assert.equal(
    h.rows()[0].querySelector("input").getAttribute("aria-label"),
    "Select Novel"
  );
  assert.equal(h.doc.getElementById("confirm").textContent, "Confirm");
  assert.equal(h.window.localStorage.getItem("ziptool.locale"), "en");
});

test("i18n: 已显示内容在切换语言后同步刷新", async () => {
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "plugin:dialog|open") return "C:\\demo\\data";
    if (cmd === "scan") return SCAN_COMPRESS;
    return null;
  });

  // 先用中文渲染出全部动态内容
  await pickSource(h);
  h.doc.getElementById("opt-password").value = "pw";
  h.doc
    .getElementById("opt-password")
    .dispatchEvent(new h.window.Event("input", { bubbles: true }));
  assert.ok(h.doc.getElementById("pw-hint").textContent.includes("ZipCrypto"));
  assert.deepEqual(h.badges(), ["待压缩", "待压缩", "待压缩"]);

  const srcRow = h.doc.querySelector('[data-drop="src"]');
  h.doc.elementFromPoint = () => srcRow;
  await h.emit(TauriEvent.DRAG_ENTER, {
    paths: ["D:\\data"],
    position: { x: 1, y: 1 },
  });
  assert.equal(h.doc.getElementById("src").placeholder, "松开鼠标以选择此文件夹");

  // 再切换英文: 状态栏/徽章/aria/密码提示/拖拽占位都应重渲染
  h.window.i18next.addResourceBundle(
    "en",
    "translation",
    {
      dialog: {
        selectSourceCompress: "Select folder to compress",
        selectSourceExtract: "Select folder to extract",
        selectDst: "Select destination folder",
      },
      options: {
        pwHintZip: "zip format: ZipCrypto",
        pwHint7z: "7z format: AES-256",
      },
      path: { dropRelease: "Release to choose this folder" },
      row: {
        status: { pendingCompress: "Pending", pendingExtract: "Pending extract" },
        select: "Select {{name}}",
      },
    },
    true,
    true
  );
  await switchLang(h, "en");

  assert.ok(
    h.doc
      .getElementById("msg")
      .textContent.includes("Scanned C:\\demo\\data: 3 items, 3.25 GB"),
    h.doc.getElementById("msg").textContent
  );
  assert.deepEqual(h.badges(), ["Pending", "Pending", "Pending"]);
  assert.equal(
    h.rows()[0].querySelector("input").getAttribute("aria-label"),
    "Select Novel"
  );
  assert.equal(h.doc.getElementById("pw-hint").textContent, "zip format: ZipCrypto");
  assert.equal(
    h.doc.getElementById("src").placeholder,
    "Release to choose this folder"
  );
  await h.emit(TauriEvent.DRAG_LEAVE, {});

  // 文件选择框标题在下次打开时使用新语言
  h.doc.getElementById("src").click();
  await h.flush();
  await h.flush();
  const dlg = h.calls.filter((c) => c.cmd === "plugin:dialog|open").at(-1);
  assert.equal(dlg.args.options.title, "Select folder to compress");
});

test("i18n: 运行中切换语言刷新进度与取消按钮", async () => {
  let resolveCompress;
  const pending = new Promise((resolve) => (resolveCompress = resolve));
  const h = makeHarness(true, ({ cmd, args, callbacks }) => {
    if (cmd === "plugin:dialog|open") {
      return args.options.title?.includes("目标") ? "C:\\out" : "C:\\demo\\data";
    }
    if (cmd === "scan") return SCAN_COMPRESS;
    if (cmd === "compress") {
      sendProgress(callbacks, args.onProgress, 0, {
        phase: "compressing",
        done: 1,
        total: 2,
        doneBytes: 1_300_000_000,
        totalBytes: 2_600_204_800,
        entry: "Novel",
      });
      return pending;
    }
    return null;
  });

  await pickSource(h);
  h.doc.getElementById("dst").click();
  await h.flush();
  await h.flush();
  assert.equal(h.doc.getElementById("dst").value, "C:\\out");

  h.window.i18next.addResourceBundle(
    "en",
    "translation",
    {
      status: {
        compressing:
          "Compressing {{done}}/{{total}} {{entry}} ({{doneBytes}} / {{totalBytes}})",
      },
      header: { cancel: "Cancel" },
    },
    true,
    true
  );

  h.doc.getElementById("confirm").click();
  await h.flush();
  await h.flush();
  assert.equal(h.doc.getElementById("confirm").textContent, "取消");
  assert.ok(
    h.doc.getElementById("msg").textContent.startsWith("压缩中 1/2 Novel"),
    h.doc.getElementById("msg").textContent
  );

  await switchLang(h, "en");
  assert.equal(h.doc.getElementById("confirm").textContent, "Cancel");
  assert.ok(
    h.doc.getElementById("msg").textContent.startsWith("Compressing 1/2 Novel"),
    h.doc.getElementById("msg").textContent
  );

  resolveCompress({
    cancelled: false,
    entries: [{ archive: "C:\\out\\Novel.zip", original: "Novel" }],
    warnings: [],
  });
  await h.flush();
  await h.flush();
});

test("i18n: 表格大小按语言格式化字节与单位", async () => {
  const make = (locale) =>
    makeHarness(
      true,
      ({ cmd }) => {
        if (cmd === "plugin:dialog|open") return "C:\\demo\\data";
        if (cmd === "scan") return SCAN_COMPRESS;
        return null;
      },
      { locale }
    );

  const en = make("en");
  await pickSource(en);
  assert.deepEqual(
    en.rows().map((tr) => tr.children[2].textContent),
    ["2.42 GB", "848.8 MB", "200.0 KB"]
  );

  const ru = make("ru");
  await pickSource(ru);
  assert.deepEqual(
    ru.rows().map((tr) => tr.children[2].textContent),
    ["2,42 ГБ", "848,8 МБ", "200,0 КБ"]
  );
  assert.ok(
    ru.doc.getElementById("msg").textContent.includes("3,25 ГБ"),
    ru.doc.getElementById("msg").textContent
  );
});

test("i18n: 语言菜单点击外部/Esc 关闭", async () => {
  const h = makeHarness(false, () => null);
  const toggle = h.doc.getElementById("lang-toggle");
  const menu = h.doc.getElementById("lang-menu");

  toggle.click();
  assert.ok(!menu.classList.contains("hidden"));
  assert.equal(toggle.getAttribute("aria-expanded"), "true");

  h.doc.body.dispatchEvent(new h.window.Event("mousedown", { bubbles: true }));
  assert.ok(menu.classList.contains("hidden"));
  assert.equal(toggle.getAttribute("aria-expanded"), "false");

  toggle.click();
  h.doc.dispatchEvent(
    new h.window.KeyboardEvent("keydown", { key: "Escape", bubbles: true })
  );
  assert.ok(menu.classList.contains("hidden"));
  assert.equal(h.doc.activeElement, toggle);
});

test("浏览器预览: 菜单栏可用, 设置页可从菜单打开", async () => {
  const h = makeHarness(false, () => null);
  await h.flush();
  await openSettingsPage(h);
  assert.ok(!h.doc.getElementById("view-settings").classList.contains("hidden"));
});

test("契约: 扫描后自动算方案; 阈值改动只重算方案, 不重扫", async () => {
  let planCalls = 0;
  const h = makeHarness(true, ({ cmd, args }) => {
    if (cmd === "plugin:app|version") return "1.2.3";
    if (cmd === "plugin:dialog|open") return "C:\\demo\\data";
    if (cmd === "scan") return SCAN_COMPRESS;
    if (cmd === "plan_packages") {
      planCalls += 1;
      return defaultPlan(args);
    }
    return null;
  });

  // 智能打包默认关闭 → 先勾选, 扫描完成后才会自动算方案
  h.doc.getElementById("opt-smart").click();
  await pickSource(h);
  await h.flush();
  assert.equal(planCalls, 1, "扫描完成后应自动算一次方案");
  assert.match(h.doc.getElementById("plan-summary").textContent, /预计/);
  assert.equal(h.doc.getElementById("plan-open").disabled, false);

  const merge = h.doc.getElementById("opt-merge-below");
  merge.value = "50m";
  merge.dispatchEvent(new h.window.Event("input", { bubbles: true }));
  await sleep(260);
  await h.flush();

  assert.equal(planCalls, 2, "阈值改动应重算方案");
  assert.equal(
    h.calls.filter((call) => call.cmd === "scan").length,
    1,
    "阈值改动不应重新扫描"
  );
  const lastPlan = h.calls.filter((call) => call.cmd === "plan_packages").at(-1);
  assert.equal(lastPlan.args.packaging.mergeBelow, "50m");
  assert.equal(lastPlan.args.packaging.splitAbove, "2g");
});

test("行为: 摘要/打包列/方案抽屉展示只读方案", async () => {
  const plan = {
    planId: "custom-plan",
    packages: [
      {
        id: "p0",
        kind: "split",
        memberIndexes: [0],
        inputBytes: 2_600_000_000,
        entryCount: 1,
      },
      {
        id: "p1",
        kind: "bundle",
        memberIndexes: [1, 2],
        inputBytes: 890_204_800,
        entryCount: 2,
      },
    ],
    summary: {
      totalPackages: 2,
      direct: 0,
      split: 1,
      bundle: 1,
      selectedBytes: 3_490_204_800,
      maxDirectOrBundleBytes: 890_204_800,
      maxBundleEntries: 2,
      effective: {
        mergeBelow: 100 * 1024 ** 2,
        splitAbove: 2 * 1024 ** 3,
        bundleMax: 2 * 1024 ** 3,
        maxEntries: 50_000,
      },
    },
    warnings: [],
  };
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "plugin:app|version") return "1.2.3";
    if (cmd === "plugin:dialog|open") return "C:\\demo\\data";
    if (cmd === "scan") return SCAN_COMPRESS;
    if (cmd === "plan_packages") return plan;
    return null;
  });

  h.doc.getElementById("opt-smart").click(); // 智能打包: 展示策略/打包列/抽屉
  await pickSource(h);
  assert.match(h.doc.getElementById("plan-summary").textContent, /2 个逻辑包/);
  assert.deepEqual(
    h.rows().map((tr) => tr.children[3].textContent),
    ["分包", "合并 #1", "合并 #1"]
  );

  h.doc.getElementById("plan-open").click();
  assert.ok(!h.doc.getElementById("plan-backdrop").classList.contains("hidden"));
  assert.equal(h.doc.querySelectorAll("#plan-list > div").length, 2);
  assert.match(h.doc.getElementById("plan-list").textContent, /media/);
  assert.match(h.doc.getElementById("plan-list").textContent, /photo\.jpg/);

  h.doc.getElementById("plan-filter-bundle").click();
  assert.equal(h.doc.querySelectorAll("#plan-list > div").length, 1);
  assert.match(h.doc.getElementById("plan-list").textContent, /media/);

  h.doc.getElementById("plan-close").click();
  assert.ok(h.doc.getElementById("plan-backdrop").classList.contains("hidden"));
});

test("行为: 条目超限 warning 显示在摘要/行/抽屉", async () => {
  const plan = {
    planId: "warn-plan",
    packages: [
      { id: "p0", kind: "direct", memberIndexes: [0], inputBytes: 2_600_000_000, entryCount: 1 },
      { id: "p1", kind: "direct", memberIndexes: [1], inputBytes: 890_000_000, entryCount: 60000 },
      { id: "p2", kind: "direct", memberIndexes: [2], inputBytes: 204_800, entryCount: 1 },
    ],
    summary: {
      totalPackages: 3,
      direct: 3,
      split: 0,
      bundle: 0,
      selectedBytes: 3_490_204_800,
      maxDirectOrBundleBytes: 2_600_000_000,
      maxBundleEntries: 0,
      effective: {
        mergeBelow: 100 * 1024 ** 2,
        splitAbove: 2 * 1024 ** 3,
        bundleMax: 2 * 1024 ** 3,
        maxEntries: 50000,
      },
    },
    warnings: [
      { code: "tooManyEntries", memberIndex: 1, entryCount: 60000, limit: 50000 },
    ],
  };
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "plugin:app|version") return "1.2.3";
    if (cmd === "plugin:dialog|open") return "C:\\demo\\data";
    if (cmd === "scan") return SCAN_COMPRESS;
    if (cmd === "plan_packages") return plan;
    return null;
  });

  h.doc.getElementById("opt-smart").click(); // 智能打包: 方案提醒需展示在摘要/行/抽屉
  await pickSource(h);
  assert.match(h.doc.getElementById("plan-summary").textContent, /1 条提醒/);
  assert.deepEqual(
    h.rows().map((tr) => tr.children[3].textContent),
    ["直接", "直接 · 超限", "直接"]
  );

  h.doc.getElementById("plan-open").click();
  assert.match(h.doc.getElementById("plan-warnings").textContent, /media/);
  assert.match(h.doc.getElementById("plan-warnings").textContent, /60000/);
  h.doc.getElementById("plan-close").click();
});

test("行为: 包级进度失败时, 合并包所有成员一起标失败", async () => {
  const plan = {
    planId: "progress-plan",
    packages: [
      { id: "p0", kind: "direct", memberIndexes: [0], inputBytes: 2_600_000_000, entryCount: 1 },
      { id: "p1", kind: "bundle", memberIndexes: [1, 2], inputBytes: 890_204_800, entryCount: 2 },
    ],
    summary: {
      totalPackages: 2,
      direct: 1,
      split: 0,
      bundle: 1,
      selectedBytes: 3_490_204_800,
      maxDirectOrBundleBytes: 2_600_000_000,
      maxBundleEntries: 2,
      effective: {
        mergeBelow: 100 * 1024 ** 2,
        splitAbove: 2 * 1024 ** 3,
        bundleMax: 2 * 1024 ** 3,
        maxEntries: 50000,
      },
    },
    warnings: [],
  };
  const h = makeHarness(true, ({ cmd, args, callbacks }) => {
    if (cmd === "plugin:app|version") return "1.2.3";
    if (cmd === "plugin:dialog|open")
      return args.options.title?.includes("目标") ? "C:\\out" : "C:\\demo\\data";
    if (cmd === "scan") return SCAN_COMPRESS;
    if (cmd === "plan_packages") return plan;
    if (cmd === "compress") {
      sendProgress(callbacks, args.onProgress, 0, {
        phase: "compressing",
        done: 1,
        total: 2,
        doneBytes: 2_600_000_000,
        totalBytes: 3_490_204_800,
        entry: "Novel",
      });
      return Promise.reject("boom");
    }
    return null;
  });

  await pickSource(h);
  await enableSmart(h); // 包级进度按方案映射
  h.doc.getElementById("dst").click();
  await h.flush();
  await h.flush();
  h.doc.getElementById("confirm").click();
  await h.flush();
  await h.flush();

  assert.deepEqual(h.badges(), ["完成", "失败", "失败"]);
  assert.ok(h.doc.getElementById("msg").textContent.includes("压缩失败"));
});

// ———— 应用内菜单栏 ————
const menuVisible = (h, id) => !h.doc.getElementById(id).classList.contains("hidden");
const menuActionCalls = (h) => h.calls.filter((c) => c.cmd === "menu_action");

test("菜单栏: 应用内自绘, 默认收起; 应用 > 报告问题 / 退出 走壳层命令", async () => {
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "load_settings") return { settings: SETTINGS_DEFAULTS, warnings: [] };
    return null;
  });
  await h.flush();
  await h.flush();

  assert.ok(h.doc.getElementById("app-menu"), "应有应用内菜单栏");
  assert.equal(h.doc.getElementById("settings-open"), null, "header 齿轮应移除");
  assert.ok(!menuVisible(h, "menu-app"), "菜单默认收起");
  assert.ok(!menuVisible(h, "menu-settings"), "设置菜单默认收起");

  h.doc.getElementById("menu-app-trigger").click();
  assert.ok(menuVisible(h, "menu-app"), "点击「应用」展开");
  assert.equal(h.doc.getElementById("menu-app-trigger").getAttribute("aria-expanded"), "true");

  h.doc.getElementById("menu-report-issue").click();
  await h.flush();
  assert.ok(!menuVisible(h, "menu-app"), "选中动作后收起");
  jsonEq(menuActionCalls(h).at(-1).args, { action: "report-issue" });

  h.doc.getElementById("menu-app-trigger").click();
  h.doc.getElementById("menu-quit").click();
  await h.flush();
  jsonEq(menuActionCalls(h).at(-1).args, { action: "quit" });
});

test("菜单栏: 设置 > 设置… 打开设置页; 切换语言更新菜单文案", async () => {
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "load_settings") return { settings: SETTINGS_DEFAULTS, warnings: [] };
    return null;
  });
  await openSettingsPage(h);
  assert.ok(!h.doc.getElementById("view-settings").classList.contains("hidden"));

  // 设置页已打开时「设置…」禁用
  assert.ok(h.doc.getElementById("menu-settings-open").disabled);

  await switchLang(h, "en");
  assert.equal(h.doc.querySelector("#menu-app-trigger span").textContent, "App");
  assert.equal(h.doc.querySelector("#menu-report-issue span").textContent, "Report Issue");
  assert.equal(h.doc.querySelector("#menu-quit span").textContent, "Quit");
  assert.equal(h.doc.querySelector("#menu-settings-trigger span").textContent, "Settings");
  assert.equal(h.doc.querySelector("#menu-settings-open span").textContent, "Settings…");
});

test("菜单栏: 点击外部 / Escape 关闭, Escape 后焦点回到触发按钮", async () => {
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "load_settings") return { settings: SETTINGS_DEFAULTS, warnings: [] };
    return null;
  });
  await h.flush();
  await h.flush();
  const trigger = h.doc.getElementById("menu-app-trigger");

  trigger.click();
  h.doc.body.dispatchEvent(new h.window.Event("mousedown", { bubbles: true }));
  assert.ok(!menuVisible(h, "menu-app"), "点击菜单外应收起");

  trigger.click();
  h.doc
    .getElementById("menu-quit")
    .dispatchEvent(new h.window.KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  assert.ok(!menuVisible(h, "menu-app"), "Escape 应收起");
  assert.equal(h.doc.activeElement, trigger, "Escape 后焦点回到触发按钮");
});

test("结构: App = MenuBar / Content / StatusBar; 菜单栏语义、快捷键、destructive 退出项", async () => {
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "load_settings") return { settings: SETTINGS_DEFAULTS, warnings: [] };
    return null;
  });
  await h.flush();

  // App = MenuBar / Content / StatusBar 三段式结构
  const body = h.doc.body;
  assert.equal(body.id, "app");
  const menuBar = h.doc.getElementById("app-menu-bar");
  const content = h.doc.getElementById("app-content");
  const statusBar = h.doc.getElementById("app-status-bar");
  assert.equal(menuBar.tagName, "HEADER");
  assert.equal(content.tagName, "MAIN");
  assert.equal(statusBar.tagName, "FOOTER");
  const order = [menuBar, content, statusBar].map((el, i) => ({ el, i }));
  for (let i = 1; i < order.length; i += 1) {
    assert.ok(
      order[i - 1].el.compareDocumentPosition(order[i].el) &
        h.window.Node.DOCUMENT_POSITION_FOLLOWING,
      "MenuBar / Content / StatusBar 顺序正确"
    );
  }
  assert.ok(content.contains(h.doc.getElementById("view-main")));
  assert.ok(content.contains(h.doc.getElementById("view-settings")));

  const nav = h.doc.getElementById("app-menu");
  assert.ok(menuBar.contains(nav));
  assert.equal(nav.getAttribute("role"), "menubar");
  const triggers = [...nav.querySelectorAll(".app-menu-group > .app-menu-trigger")];
  assert.equal(triggers.length, 2);
  for (const trigger of triggers) {
    assert.equal(trigger.getAttribute("role"), "menuitem");
    assert.equal(trigger.getAttribute("aria-haspopup"), "menu");
  }
  // roving tabindex: 菜单栏整体只占一个 Tab 位
  assert.deepEqual(triggers.map((t) => t.tabIndex), [0, -1]);
  // 菜单项不显示图标(顶层入口同样是纯文字)
  assert.equal(nav.querySelectorAll("svg").length, 0, "菜单栏不应有图标");
  // 菜单项右侧显示平台快捷键
  assert.equal(h.doc.getElementById("menu-quit-shortcut").textContent, "Ctrl+Q");
  assert.equal(h.doc.getElementById("menu-settings-shortcut").textContent, "Ctrl+S");
  assert.ok(h.doc.getElementById("menu-quit").classList.contains("app-menu-item--destructive"));
  assert.ok(h.doc.querySelector("#menu-app .app-menu-separator"));
});

test("菜单栏: ←/→ 移动顶层焦点; 菜单打开时 ←/→ 切换菜单", async () => {
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "load_settings") return { settings: SETTINGS_DEFAULTS, warnings: [] };
    return null;
  });
  await h.flush();
  const app = h.doc.getElementById("menu-app-trigger");
  const settings = h.doc.getElementById("menu-settings-trigger");
  const key = (target, value) =>
    target.dispatchEvent(new h.window.KeyboardEvent("keydown", { key: value, bubbles: true }));

  app.focus();
  key(app, "ArrowRight");
  assert.equal(h.doc.activeElement, settings, "→ 移到下一个顶层项");
  key(settings, "ArrowLeft");
  assert.equal(h.doc.activeElement, app, "← 回到上一个顶层项");

  app.click();
  assert.equal(h.doc.activeElement, h.doc.getElementById("menu-report-issue"));
  key(h.doc.getElementById("menu-report-issue"), "ArrowRight");
  assert.ok(menuVisible(h, "menu-settings"), "菜单打开时 → 切换到下一个菜单");
  assert.ok(!menuVisible(h, "menu-app"));
  assert.equal(h.doc.activeElement, h.doc.getElementById("menu-settings-open"));
});

test("菜单栏: 打开菜单后 hover 另一个顶层项直接切换", async () => {
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "load_settings") return { settings: SETTINGS_DEFAULTS, warnings: [] };
    return null;
  });
  await h.flush();
  h.doc.getElementById("menu-app-trigger").click();
  assert.ok(menuVisible(h, "menu-app"));

  h.doc
    .getElementById("menu-settings-trigger")
    .dispatchEvent(new h.window.MouseEvent("mouseenter"));
  assert.ok(menuVisible(h, "menu-settings"), "hover 切换到设置菜单");
  assert.ok(!menuVisible(h, "menu-app"), "旧菜单收起");
});

test("菜单栏: 菜单项支持首字 typeahead; Tab 收起菜单", async () => {
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "load_settings") return { settings: SETTINGS_DEFAULTS, warnings: [] };
    return null;
  });
  await h.flush();
  h.doc.getElementById("menu-app-trigger").click();
  const key = (target, value) =>
    target.dispatchEvent(new h.window.KeyboardEvent("keydown", { key: value, bubbles: true }));

  key(h.doc.getElementById("menu-report-issue"), "退");
  assert.equal(h.doc.activeElement, h.doc.getElementById("menu-quit"), "首字匹配到「退出」");

  key(h.doc.getElementById("menu-quit"), "Tab");
  assert.ok(!menuVisible(h, "menu-app"), "Tab 收起菜单");
});

test("菜单栏: 快捷键 Ctrl+Q 退出 / Ctrl+S 打开设置", async () => {
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "load_settings") return { settings: SETTINGS_DEFAULTS, warnings: [] };
    return null;
  });
  await h.flush();
  await h.flush();
  const press = (key) =>
    h.doc.dispatchEvent(
      new h.window.KeyboardEvent("keydown", { key, ctrlKey: true, bubbles: true })
    );

  press("s");
  await h.flush();
  assert.ok(
    !h.doc.getElementById("view-settings").classList.contains("hidden"),
    "Ctrl+S 打开设置页"
  );

  press("q");
  await h.flush();
  jsonEq(menuActionCalls(h).at(-1).args, { action: "quit" });
});

test("菜单栏: macOS 显示 ⌘ 快捷键并响应 Cmd+Q / Cmd+S", async () => {
  const h = makeHarness(
    true,
    ({ cmd }) => {
      if (cmd === "load_settings") return { settings: SETTINGS_DEFAULTS, warnings: [] };
      return null;
    },
    { userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15" }
  );
  await h.flush();
  await h.flush();
  assert.equal(h.doc.getElementById("menu-quit-shortcut").textContent, "⌘Q");
  assert.equal(h.doc.getElementById("menu-settings-shortcut").textContent, "⌘S");

  const press = (key, extra = {}) =>
    h.doc.dispatchEvent(new h.window.KeyboardEvent("keydown", { key, bubbles: true, ...extra }));
  press("q", { ctrlKey: true });
  await h.flush();
  assert.equal(menuActionCalls(h).length, 0, "macOS 下 Ctrl+Q 不触发");

  press("q", { metaKey: true });
  await h.flush();
  jsonEq(menuActionCalls(h).at(-1).args, { action: "quit" });

  press("s", { metaKey: true });
  await h.flush();
  assert.ok(
    !h.doc.getElementById("view-settings").classList.contains("hidden"),
    "Cmd+S 打开设置页"
  );
});

test("菜单栏: 扫描期间设置可用(不锁 UI), 结束后仍可用", async () => {
  let finishScan;
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "load_settings") return { settings: SETTINGS_DEFAULTS, warnings: [] };
    if (cmd === "plugin:dialog|open") return "C:\\demo\\data";
    if (cmd === "scan") return new Promise((resolve) => (finishScan = resolve));
    return null;
  });
  await h.flush();
  await h.flush();
  assert.ok(!h.doc.getElementById("menu-settings-open").disabled, "设置加载完成后启用");
  h.doc.getElementById("src").click();
  await h.flush();
  await h.flush();
  assert.ok(!h.doc.getElementById("menu-settings-open").disabled, "扫描期间设置可用");

  // 扫描期间可以从菜单进入设置页(其它控件都不锁)
  h.doc.getElementById("menu-settings-trigger").click();
  h.doc.getElementById("menu-settings-open").click();
  await h.flush();
  await h.flush();
  assert.ok(
    !h.doc.getElementById("view-settings").classList.contains("hidden"),
    "扫描期间「设置…」应能打开设置页"
  );
  h.doc.getElementById("settings-back").click();
  await h.flush();
  await h.flush();

  finishScan(SCAN_COMPRESS);
  for (let i = 0; i < 5; i += 1) await h.flush();
  assert.ok(!h.doc.getElementById("menu-settings-open").disabled, "扫描结束后仍启用");
  assert.ok(h.doc.getElementById("msg").textContent.includes("3 个条目"));
});

test("菜单栏: 报告问题失败时状态栏提示", async () => {
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "load_settings") return { settings: SETTINGS_DEFAULTS, warnings: [] };
    if (cmd === "menu_action") return Promise.reject("no browser");
    return null;
  });
  await h.flush();
  await h.flush();
  h.doc.getElementById("menu-app-trigger").click();
  h.doc.getElementById("menu-report-issue").click();
  await h.flush();
  assert.match(h.doc.getElementById("msg").textContent, /无法打开浏览器/);
});

// ———— 设置页 ————
const SETTINGS_DEFAULTS = {
  general: { shutdownAfterCompress: false },
  filters: { enabledPolicies: ["os-hidden", "lang-temp"] },
};

async function openSettingsPage(h) {
  await h.flush();
  // 设置入口在菜单栏「设置」里: 展开菜单 → 点「设置…」
  h.doc.getElementById("menu-settings-trigger").click();
  h.doc.getElementById("menu-settings-open").click();
  await h.flush();
}

test("设置页: 打开、边栏两项、默认策略都开、关机占位置灰", async () => {
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "load_settings") return { settings: SETTINGS_DEFAULTS, warnings: [] };
    if (cmd === "scan") return SCAN_COMPRESS;
    return null;
  });
  await openSettingsPage(h);

  assert.ok(!h.doc.getElementById("view-settings").classList.contains("hidden"));
  assert.ok(h.doc.getElementById("view-main").classList.contains("hidden"));
  assert.equal(h.doc.querySelectorAll("#settings-nav [data-section]").length, 2);
  assert.equal(h.doc.querySelector("#settings-nav [aria-current=page]").dataset.section, "general");
  assert.ok(!h.doc.querySelector('[data-panel="general"]').classList.contains("hidden"));
  assert.ok(h.doc.querySelector('[data-panel="filters"]').classList.contains("hidden"));

  const shutdown = h.doc.getElementById("set-shutdown");
  assert.ok(shutdown.disabled, "关机占位控件应置灰");
  assert.ok(!shutdown.checked);
  assert.ok(h.doc.getElementById("policy-os-hidden").checked);
  assert.ok(h.doc.getElementById("policy-lang-temp").checked);
});

test("设置页: 切换侧栏、策略切换立即保存并在关闭时按新策略重扫", async () => {
  const saves = [];
  const h = makeHarness(true, ({ cmd, args }) => {
    if (cmd === "plugin:dialog|open")
      return args.options.title?.includes("目标") ? "C:\\out" : "C:\\demo\\data";
    if (cmd === "load_settings") return { settings: SETTINGS_DEFAULTS, warnings: [] };
    if (cmd === "scan") return SCAN_COMPRESS;
    if (cmd === "save_settings") {
      saves.push(args.settings);
      return args.settings;
    }
    return null;
  });
  await pickSource(h);
  await openSettingsPage(h);
  h.doc.querySelector('[data-section="filters"]').click();
  assert.ok(!h.doc.querySelector('[data-panel="filters"]').classList.contains("hidden"));
  assert.equal(h.doc.querySelector('[data-section="filters"]').getAttribute("aria-current"), "page");

  h.doc.getElementById("policy-lang-temp").click();
  await h.flush();
  await h.flush();
  jsonEq(saves.at(-1).filters.enabledPolicies, ["os-hidden"]);

  h.doc.getElementById("settings-back").click();
  await h.flush();
  await h.flush();
  await h.flush();
  const scans = h.calls.filter((c) => c.cmd === "scan");
  assert.equal(scans.length, 2, "策略变化后应重扫一次");
  jsonEq(scans.at(-1).args.filters, { enabledPolicies: ["os-hidden"] });
});

test("设置页: 保存失败回滚开关并提示", async () => {
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "load_settings") return { settings: SETTINGS_DEFAULTS, warnings: [] };
    if (cmd === "save_settings") return Promise.reject("disk full");
    return null;
  });
  await openSettingsPage(h);
  const os = h.doc.getElementById("policy-os-hidden");
  os.click();
  await h.flush();
  await h.flush();
  assert.ok(os.checked, "保存失败应回滚为开");
  assert.match(h.doc.getElementById("msg").textContent, /保存设置失败/);
});

test("设置页: 关机字段为 true 时展示 disabled+checked, 保存其它设置原样保留", async () => {
  const saves = [];
  const h = makeHarness(true, ({ cmd, args }) => {
    if (cmd === "load_settings") {
      return {
        settings: {
          general: { shutdownAfterCompress: true },
          filters: { enabledPolicies: ["os-hidden", "lang-temp"] },
        },
        warnings: [],
      };
    }
    if (cmd === "save_settings") {
      saves.push(args.settings);
      return args.settings;
    }
    return null;
  });
  await openSettingsPage(h);
  const shutdown = h.doc.getElementById("set-shutdown");
  assert.ok(shutdown.disabled);
  assert.ok(shutdown.checked);

  h.doc.getElementById("policy-lang-temp").click();
  await h.flush();
  await h.flush();
  assert.equal(saves.at(-1).general.shutdownAfterCompress, true);
  assert.ok(!h.calls.some((c) => c.cmd === "power_off"), "本期不应有电源命令");
});

test("设置页: 切换语言更新静态文案", async () => {
  const h = makeHarness(true, ({ cmd }) => {
    if (cmd === "load_settings") return { settings: SETTINGS_DEFAULTS, warnings: [] };
    return null;
  });
  await openSettingsPage(h);
  await switchLang(h, "en");
  assert.equal(h.doc.querySelector("#view-settings h1").textContent, "Settings");
  assert.equal(h.doc.getElementById("policy-os-hidden-title").textContent, "OS hidden files");
});
