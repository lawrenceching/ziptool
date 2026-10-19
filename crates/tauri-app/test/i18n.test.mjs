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
  assert.ok(w.document.querySelector('[data-i18n-aria-label="table.selectAll"]'));
  assert.deepEqual(
    [...w.document.querySelectorAll("#lang option")].map((o) => o.value),
    ["system", "zh-CN", "zh-HK", "zh-TW", "en", "ru"]
  );
});
