// 桌面 UI 多语言入口(经典脚本, 无打包器)。
// 加载顺序: vendor/i18next.min.js → locales/*.js → 本文件 → app.js
(function () {
  "use strict";

  const SUPPORTED = ["zh-CN", "zh-HK", "zh-TW", "en", "ru"];
  const FALLBACK = "en";
  const STORAGE_KEY = "ziptool.locale";

  function canonicalTag(tag) {
    if (typeof tag !== "string") return null;
    const cleaned = tag.trim().replace(/_/g, "-");
    if (!cleaned) return null;
    try {
      return Intl.getCanonicalLocales(cleaned)[0] || null;
    } catch {
      return cleaned;
    }
  }

  function partsOf(tag) {
    const canonical = canonicalTag(tag);
    if (!canonical) return null;
    const chunks = canonical.split("-");
    const lang = chunks[0].toLowerCase();
    let script = null;
    let region = null;
    for (const chunk of chunks.slice(1)) {
      // 单字母子标签开始 extension / private-use 区段 (如 -u-nu-hanidec), 之后不再解析
      if (chunk.length === 1) break;
      if (/^[A-Za-z]{4}$/.test(chunk)) script = chunk.toLowerCase();
      else if (/^([A-Za-z]{2}|\d{3})$/.test(chunk)) region = chunk.toUpperCase();
    }
    return { lang, script, region };
  }

  function matchLocale(tag) {
    const parts = partsOf(tag);
    if (!parts) return null;
    if (parts.lang === "en") return "en";
    if (parts.lang === "ru") return "ru";
    if (parts.lang !== "zh") return null;
    if (parts.script === "hans") return "zh-CN";
    if (parts.script === "hant") {
      return parts.region === "HK" || parts.region === "MO" ? "zh-HK" : "zh-TW";
    }
    if (parts.region === "HK" || parts.region === "MO") return "zh-HK";
    if (parts.region === "TW") return "zh-TW";
    return "zh-CN";
  }

  function systemTags() {
    if (Array.isArray(navigator.languages) && navigator.languages.length > 0) {
      return navigator.languages.slice();
    }
    return navigator.language ? [navigator.language] : [];
  }

  function resolveLocale(tags) {
    for (const tag of tags || []) {
      const hit = matchLocale(tag);
      if (hit) return hit;
    }
    return FALLBACK;
  }

  function readPreference() {
    try {
      const raw = localStorage.getItem(STORAGE_KEY);
      return raw === "system" || SUPPORTED.includes(raw) ? raw : "system";
    } catch {
      return "system";
    }
  }

  function writePreference(value) {
    try {
      localStorage.setItem(STORAGE_KEY, value);
    } catch {
      // 隐私模式/禁用存储: 仅本次生效
    }
  }

  function localeFor(preference) {
    return preference === "system" ? resolveLocale(systemTags()) : preference;
  }

  const hasI18next =
    typeof i18next !== "undefined" && typeof i18next.init === "function";
  let currentLocale = hasI18next ? localeFor(readPreference()) : "zh-CN";
  let initialized = false;

  function getLocale() {
    return hasI18next && i18next.language ? i18next.language : currentLocale;
  }

  function t(key, params) {
    if (!initialized) return key;
    try {
      return i18next.t(key, params);
    } catch {
      return key;
    }
  }

  function applyDom(root) {
    if (!root || root === document) {
      // 首屏防闪: 先解除隐藏, 即使 i18next 初始化失败也不留白屏
      document.documentElement.dataset.i18nReady = "1";
    }
    if (!initialized) return;

    const scope = root || document;
    for (const el of scope.querySelectorAll("[data-i18n]")) {
      el.textContent = t(el.getAttribute("data-i18n"));
    }
    for (const el of scope.querySelectorAll("[data-i18n-placeholder]")) {
      el.setAttribute("placeholder", t(el.getAttribute("data-i18n-placeholder")));
    }
    for (const el of scope.querySelectorAll("[data-i18n-title]")) {
      el.setAttribute("title", t(el.getAttribute("data-i18n-title")));
    }
    for (const el of scope.querySelectorAll("[data-i18n-aria-label]")) {
      el.setAttribute("aria-label", t(el.getAttribute("data-i18n-aria-label")));
    }
    if (!root || root === document) {
      document.documentElement.lang = getLocale();
    }
  }

  function notify() {
    window.dispatchEvent(
      new window.CustomEvent("localechange", { detail: { locale: getLocale() } })
    );
  }

  async function setLocale(choice) {
    const preference =
      choice === "system" || SUPPORTED.includes(choice) ? choice : "system";
    const target = localeFor(preference);
    const previous = getLocale();

    if (hasI18next && initialized && target !== previous) {
      try {
        await i18next.changeLanguage(target);
        currentLocale = i18next.language;
      } catch (err) {
        console.error("i18n changeLanguage failed", err);
        return previous;
      }
    } else if (!hasI18next) {
      currentLocale = "zh-CN";
    }

    writePreference(preference);
    applyDom();
    if (target !== previous) notify();
    return getLocale();
  }

  function init() {
    if (!hasI18next) {
      applyDom();
      return currentLocale;
    }
    try {
      i18next.init({
        resources: window.__ZIPTOOL_LOCALES__ || {},
        lng: currentLocale,
        fallbackLng: FALLBACK,
        supportedLngs: SUPPORTED,
        initAsync: false,
        returnEmptyString: false,
        interpolation: { escapeValue: false },
      });
      initialized = true;
      currentLocale = i18next.language;
    } catch (err) {
      console.error("i18n init failed", err);
    }
    applyDom();
    return currentLocale;
  }

  window.zipToolI18n = {
    SUPPORTED,
    FALLBACK,
    STORAGE_KEY,
    matchLocale,
    resolveLocale,
    systemTags,
    readPreference,
    localeFor,
    init,
    t,
    setLocale,
    applyDom,
    get locale() {
      return getLocale();
    },
  };

  init();
})();
