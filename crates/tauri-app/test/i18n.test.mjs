import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { JSDOM } from "jsdom";

const read = (rel) => readFileSync(new URL(rel, import.meta.url), "utf8");
const LOCALES = ["zh-CN", "zh-HK", "zh-TW", "en", "ru"];

function makeDom({ preference = "zh-CN", languages = ["en-US"] } = {}) {
  const dom = new JSDOM(read("../ui/index.html"), {
    url: "http://localhost/",
    runScripts: "outside-only",
  });
  const { window } = dom;
  window.localStorage.setItem("ziptool.locale", preference);
  Object.defineProperty(window.navigator, "languages", {
    value: languages,
    configurable: true,
  });
  window.eval(read("../ui/vendor/i18next.min.js"));
  for (const code of LOCALES) window.eval(read(`../ui/locales/${code}.js`));
  window.eval(read("../ui/i18n.js"));
  return window;
}

test("语言标签匹配: 港/台/简与英语俄语", () => {
  const w = makeDom();
  const cases = [
    ["zh-HK", "zh-HK"],
    ["zh-Hant-HK", "zh-HK"],
    ["zh-MO", "zh-HK"],
    ["zh-Hant-MO", "zh-HK"],
    ["zh-TW", "zh-TW"],
    ["zh-Hant-TW", "zh-TW"],
    ["zh-Hant", "zh-TW"],
    ["zh-Hant-CN", "zh-TW"],
    ["zh-CN", "zh-CN"],
    ["zh-Hans-HK", "zh-CN"],
    ["zh-HK-u-nu-hanidec", "zh-HK"],
    ["zh-Hant-TW-u-ca-chinese", "zh-TW"],
    ["zh-SG", "zh-CN"],
    ["zh", "zh-CN"],
    ["en-GB", "en"],
    ["ru-RU", "ru"],
    ["ja-JP", null],
    ["de-DE", null],
  ];
  for (const [tag, expected] of cases) {
    assert.equal(w.zipToolI18n.matchLocale(tag), expected, tag);
  }
});

test("偏好顺序与回退: 第一个受支持语言生效, 全不支持回退英语", () => {
  const w = makeDom();
  assert.equal(w.zipToolI18n.resolveLocale(["ja-JP", "zh-HK"]), "zh-HK");
  assert.equal(w.zipToolI18n.resolveLocale(["ja-JP", "de-DE"]), "en");
  assert.equal(w.zipToolI18n.resolveLocale([]), "en");
});

test("启动偏好: 具体语言覆盖系统, system 跟随系统, 非法值回退 system", () => {
  assert.equal(
    makeDom({ preference: "en", languages: ["ru-RU"] }).zipToolI18n.locale,
    "en"
  );
  assert.equal(
    makeDom({ preference: "system", languages: ["zh-TW"] }).zipToolI18n.locale,
    "zh-TW"
  );
  assert.equal(
    makeDom({ preference: "bogus", languages: ["ru-RU"] }).zipToolI18n.locale,
    "ru"
  );
});

test("切换语言: 更新 html lang、持久化并派发 localechange", async () => {
  const w = makeDom({ preference: "zh-CN" });
  let seen = null;
  w.addEventListener("localechange", (e) => (seen = e.detail.locale));
  const result = await w.zipToolI18n.setLocale("en");
  assert.equal(result, "en");
  assert.equal(w.zipToolI18n.locale, "en");
  assert.equal(w.document.documentElement.lang, "en");
  assert.equal(w.localStorage.getItem("ziptool.locale"), "en");
  assert.equal(seen, "en");
});

test("缺失 key 回退英文", () => {
  const w = makeDom({ preference: "zh-HK" });
  w.i18next.addResourceBundle(
    "en",
    "translation",
    { onlyEnglish: "EN only" },
    true,
    true
  );
  assert.equal(w.zipToolI18n.t("onlyEnglish"), "EN only");
});

function localeCatalog(code) {
  const sandbox = { window: {} };
  vm.runInNewContext(read(`../ui/locales/${code}.js`), sandbox);
  return sandbox.window.__ZIPTOOL_LOCALES__[code].translation;
}

const PLURAL = /_(zero|one|two|few|many|other)$/;
const baseKey = (key) => key.replace(PLURAL, "");

function flatten(obj, prefix = "") {
  const out = {};
  for (const [key, value] of Object.entries(obj)) {
    const full = prefix ? `${prefix}.${key}` : key;
    if (value && typeof value === "object") Object.assign(out, flatten(value, full));
    else out[full] = value;
  }
  return out;
}

test("五份词条 base key 集合一致, 且每个复数 key 都有 _other", () => {
  const catalogs = Object.fromEntries(
    LOCALES.map((code) => [code, flatten(localeCatalog(code))])
  );
  const reference = [...new Set(Object.keys(catalogs["zh-CN"]).map(baseKey))].sort();

  for (const code of LOCALES) {
    const keys = [...new Set(Object.keys(catalogs[code]).map(baseKey))].sort();
    assert.deepEqual(keys, reference, `${code} key 集合与 zh-CN 不一致`);
  }

  const pluralBases = new Set();
  for (const catalog of Object.values(catalogs)) {
    for (const key of Object.keys(catalog)) {
      if (PLURAL.test(key)) pluralBases.add(baseKey(key));
    }
  }
  for (const base of pluralBases) {
    for (const code of LOCALES) {
      assert.ok(`${base}_other` in catalogs[code], `${code} 缺少 ${base}_other`);
    }
  }
});

test("启动时读取已保存偏好", () => {
  const w = makeDom({ preference: "en" });
  assert.equal(w.zipToolI18n.readPreference(), "en");
  assert.equal(w.zipToolI18n.locale, "en");
  assert.equal(w.document.documentElement.lang, "en");
});

test("index.html 静态文案与语言下拉结构完整", () => {
  const w = makeDom();
  assert.equal(w.document.documentElement.getAttribute("data-i18n-ready"), "1");
  const keys = new Set(
    [...w.document.querySelectorAll("[data-i18n]")].map((el) =>
      el.getAttribute("data-i18n")
    )
  );
  for (const key of [
    "header.compress",
    "header.extract",
    "header.confirm",
    "menu.app",
    "menu.reportIssue",
    "menu.quit",
    "menu.settings",
    "menu.openSettings",
    "path.src",
    "path.dst",
    "options.split",
    "options.password",
    "options.randomName",
    "options.verify",
    "options.preset",
    "options.presetCompat",
    "options.presetBalanced",
    "options.presetCustom",
    "options.advanced",
    "options.format",
    "options.level",
    "options.exclude",
    "options.force",
    "table.caption",
    "table.name",
    "table.size",
    "table.status",
    "table.empty",
    "status.ready",
    "lang.system",
  ]) {
    assert.ok(keys.has(key), `缺少 data-i18n: ${key}`);
  }
  assert.ok(w.document.querySelector('[data-i18n-placeholder="path.srcPlaceholder"]'));
  assert.ok(w.document.querySelector('[data-i18n-placeholder="path.dstPlaceholder"]'));
  assert.ok(w.document.querySelector('[data-i18n-title="options.verifyTitle"]'));
  assert.ok(w.document.querySelector('[data-i18n-title="options.presetCompatTitle"]'));
  assert.ok(w.document.querySelector('[data-i18n-title="options.presetBalancedTitle"]'));
  assert.ok(w.document.querySelector('[data-i18n-title="options.levelTitle"]'));
  assert.ok(w.document.querySelector('[data-i18n-aria-label="header.language"]'));
  assert.ok(w.document.querySelector('[data-i18n-aria-label="menu.label"]'));
  assert.ok(w.document.querySelector('[data-i18n-aria-label="table.selectAll"]'));
  assert.deepEqual(
    [...w.document.querySelectorAll("#lang-menu [data-lang]")].map(
      (el) => el.dataset.lang
    ),
    ["system", "zh-CN", "zh-HK", "zh-TW", "en", "ru"]
  );

  const toggle = w.document.getElementById("lang-toggle");
  const menu = w.document.getElementById("lang-menu");
  const ver = w.document.getElementById("ver");
  assert.ok(
    w.document.querySelector("footer").contains(toggle),
    "语言按钮应在状态栏"
  );
  assert.ok(
    !w.document.querySelector("header").contains(toggle),
    "语言按钮不应在顶栏"
  );
  assert.ok(
    toggle.compareDocumentPosition(ver) & w.Node.DOCUMENT_POSITION_FOLLOWING,
    "语言按钮应在版本号左侧"
  );
  assert.ok(menu.classList.contains("hidden"), "语言菜单默认收起");
});

test("英文界面关键文案为英文, 不支持语言回退英文", () => {
  const enCopy = localeCatalog("en");
  const zhCopy = localeCatalog("zh-CN");
  assert.notEqual(enCopy.header.compress, zhCopy.header.compress);

  const en = makeDom({ preference: "en" });
  assert.equal(
    en.document.getElementById("mode-compress").textContent,
    enCopy.header.compress
  );
  assert.equal(
    en.document.getElementById("mode-extract").textContent,
    enCopy.header.extract
  );
  assert.equal(
    en.document.getElementById("confirm").textContent,
    enCopy.header.confirm
  );

  const fallback = makeDom({ preference: "system", languages: ["ja-JP"] });
  assert.equal(fallback.zipToolI18n.locale, "en");
  assert.equal(
    fallback.document.getElementById("mode-compress").textContent,
    enCopy.header.compress
  );
});

test("英文与俄语复数形态齐全", () => {
  const catalogs = Object.fromEntries(
    LOCALES.map((code) => [code, flatten(localeCatalog(code))])
  );
  const pluralBases = new Set();
  for (const catalog of Object.values(catalogs)) {
    for (const key of Object.keys(catalog)) {
      if (PLURAL.test(key)) pluralBases.add(baseKey(key));
    }
  }
  assert.ok(pluralBases.size > 0, "应存在复数 key");

  for (const base of pluralBases) {
    for (const code of LOCALES) {
      assert.ok(`${base}_other` in catalogs[code], `${code} 缺少 ${base}_other`);
    }
    assert.ok(`${base}_one` in catalogs.en, `en 缺少 ${base}_one`);
    for (const suffix of ["_one", "_few", "_many", "_other"]) {
      assert.ok(`${base}${suffix}` in catalogs.ru, `ru 缺少 ${base}${suffix}`);
    }
  }
});

test("俄语 1/2/5 使用不同复数形态", () => {
  const w = makeDom({ preference: "ru" });
  const one = w.zipToolI18n.t("status.scanDone", {
    count: 1,
    path: "p",
    bytes: "1 B",
  });
  const few = w.zipToolI18n.t("status.scanDone", {
    count: 2,
    path: "p",
    bytes: "1 B",
  });
  const many = w.zipToolI18n.t("status.scanDone", {
    count: 5,
    path: "p",
    bytes: "1 B",
  });
  assert.equal(new Set([one, few, many]).size, 3, `${one} | ${few} | ${many}`);
});

test("五种语言占位符与 zh-CN 一致", () => {
  const catalogs = Object.fromEntries(
    LOCALES.map((code) => [code, flatten(localeCatalog(code))])
  );
  const zh = catalogs["zh-CN"];
  const placeholders = (value) =>
    [...value.matchAll(/\{\{(\w+)\}\}/g)].map((match) => match[1]);

  for (const code of LOCALES) {
    for (const [key, value] of Object.entries(catalogs[code])) {
      const base = baseKey(key);
      const reference = zh[base] ?? zh[`${base}_other`];
      assert.ok(reference, `${code}: ${key} 没有 zh-CN 参考`);
      assert.deepEqual(
        placeholders(value).sort(),
        placeholders(reference).sort(),
        `${code}: ${key} 占位符与 zh-CN 不一致`
      );
    }
  }
});

test("英文/俄语词条不含中文字符", () => {
  for (const code of ["en", "ru"]) {
    const catalog = flatten(localeCatalog(code));
    for (const [key, value] of Object.entries(catalog)) {
      assert.ok(!/[\u4e00-\u9fff]/.test(value), `${code}: ${key} 含中文: ${value}`);
    }
  }
});

test("五种语言启动时关键文案与词条一致", () => {
  for (const code of LOCALES) {
    const copy = localeCatalog(code);
    const w = makeDom({ preference: code });

    assert.equal(w.document.documentElement.lang, code);
    assert.equal(
      w.document.getElementById("mode-compress").textContent,
      copy.header.compress
    );
    assert.equal(
      w.document.getElementById("mode-extract").textContent,
      copy.header.extract
    );
    assert.equal(
      w.document.getElementById("confirm").textContent,
      copy.header.confirm
    );
    assert.equal(
      w.document.getElementById("src").placeholder,
      copy.path.srcPlaceholder
    );
    assert.equal(w.document.getElementById("empty").textContent, copy.table.empty);
    assert.equal(w.document.getElementById("msg").textContent, copy.status.ready);
    assert.equal(
      w.document.querySelector("#menu-app-trigger span").textContent,
      copy.menu.app
    );
    assert.equal(
      w.document.querySelector("#menu-settings-open span").textContent,
      copy.menu.openSettings
    );
    assert.equal(
      w.document.querySelector('[data-i18n="table.name"]').textContent,
      copy.table.name
    );
  }
});

test("i18next 缺失时解除首屏隐藏并回退中文文案", () => {
  const dom = new JSDOM(read("../ui/index.html"), {
    url: "http://localhost/",
    runScripts: "outside-only",
  });
  const w = dom.window;
  w.localStorage.setItem("ziptool.locale", "zh-CN");
  w.eval(read("../ui/locales/zh-CN.js"));
  w.eval(read("../ui/i18n.js"));
  w.eval(read("../ui/app.js"));

  assert.equal(w.document.documentElement.getAttribute("data-i18n-ready"), "1");
  assert.equal(w.zipToolI18n.t("header.compress"), "压缩");
  assert.equal(
    w.zipToolI18n.t("status.scanDone", { count: 3, path: "p", bytes: "1 B" }),
    "已扫描 p: 3 个条目, 共 1 B"
  );
  assert.equal(w.document.getElementById("mode-compress").textContent.trim(), "压缩");
  assert.equal(w.document.getElementById("msg").textContent, "就绪");
  assert.equal(w.document.getElementById("confirm").textContent, "确认");
});

test("i18next.init 抛错时解除首屏隐藏并回退中文文案", () => {
  const dom = new JSDOM(read("../ui/index.html"), {
    url: "http://localhost/",
    runScripts: "outside-only",
  });
  const w = dom.window;
  w.localStorage.setItem("ziptool.locale", "zh-CN");
  w.console.error = () => {};
  w.eval(read("../ui/locales/zh-CN.js"));
  w.i18next = {
    init() {
      throw new Error("boom");
    },
    t() {
      throw new Error("i18next.t should not be used before init");
    },
  };
  w.eval(read("../ui/i18n.js"));
  w.eval(read("../ui/app.js"));

  assert.equal(w.document.documentElement.getAttribute("data-i18n-ready"), "1");
  assert.equal(w.document.getElementById("msg").textContent, "就绪");
  assert.equal(w.document.getElementById("confirm").textContent, "确认");
});
