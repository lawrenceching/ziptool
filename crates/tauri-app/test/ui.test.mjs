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

// 回放一条 Rust → JS 的进度消息(真实 Channel 排序协议)
function sendProgress(callbacks, channel, i, message) {
  callbacks.get(channel.id)({ index: i, message });
}

/**
 * 构建一个加载了 app.js 的页面。
 * - withTauri=false: 浏览器预览形态(无 __TAURI__, 走内置 mock)
 * - withTauri=true: 真实包表面 + mock IPC 传输; invokeImpl 按命令名分发
 */
function makeHarness(withTauri, invokeImpl) {
  const dom = new JSDOM(readFileSync(INDEX_HTML, "utf8"), {
    url: "http://localhost/",
    runScripts: "outside-only", // window.eval 跑在带完整全局(document 等)的上下文里
  });
  const { window } = dom;
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
        return Promise.resolve(invokeImpl({ cmd, args, callbacks, calls }));
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

  // scan 契约: 命令名 + mode/path + onProgress 是真实 core.Channel 且序列化为 __CHANNEL__:<id>
  const scan = h.calls.find((c) => c.cmd === "scan");
  assert.ok(scan, "应调用 scan 命令");
  assert.equal(scan.args.mode, "compress");
  assert.equal(scan.args.path, "C:\\demo\\data");
  assert.ok(scan.args.onProgress instanceof coreApi.Channel, "onProgress 必须是 core.Channel 实例");
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
  h.doc.getElementById("dst").click();
  await h.flush();
  await h.flush();
  // 取消第二行勾选(media)
  h.rows()[1].querySelector("input").click();
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
    splitSize: "2g",
    password: "pw",
    randomName: true,
    format: "zip",
    level: 9,
    excludes: [],
    verify: true,
  });
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
  assert.ok(h.doc.getElementById("opt-split-wrap").classList.contains("hidden"));
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
  assert.ok(h.doc.getElementById("opt-split-wrap").classList.contains("hidden") === false);
  assert.ok(h.doc.getElementById("opt-force-wrap").classList.contains("hidden"), "解压专属「覆盖」初始隐藏");
  assert.ok(h.doc.getElementById("opt-format-wrap").classList.contains("hidden"), "高级子项仍等勾选展开");
});

test("行为: 压缩模式预设 — 默认最大兼容性, 预设联动参数, 手动改参数变自定义", async () => {
  const h = makeHarness(false, () => null);
  // 默认 = 最大兼容性预设(zip + 强度 9)
  assert.equal(h.doc.getElementById("opt-format").value, "zip");
  assert.equal(h.doc.getElementById("opt-level").value, "9");
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
  h.doc.getElementById("dst").click();
  await h.flush();
  await h.flush();
  h.doc.getElementById("preset-balanced").click();
  h.doc.getElementById("confirm").click();
  await h.flush();
  await h.flush();
  const comp = h.calls.find((c) => c.cmd === "compress");
  assert.ok(comp, "应调用 compress");
  jsonEq(comp.args.options, {
    splitSize: null,
    password: null,
    randomName: false,
    format: "7z",
    level: 5,
    excludes: [],
    verify: true,
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
