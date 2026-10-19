import { readFileSync, writeFileSync } from "node:fs";
import vm from "node:vm";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const source = resolve(root, "ui/locales/zh-CN.js");
const sandbox = { window: {} };
vm.runInNewContext(readFileSync(source, "utf8"), sandbox);
const translation = sandbox.window.__ZIPTOOL_LOCALES__["zh-CN"].translation;

const banner =
  "// TEMP: 由 scripts/scaffold-locales.mjs 生成的 zh-CN 占位词条, Task 7 必须替换为用户提供的最终文案。\n";

for (const code of ["zh-HK", "zh-TW", "en", "ru"]) {
  const body = JSON.stringify({ translation }, null, 2);
  writeFileSync(
    resolve(root, `ui/locales/${code}.js`),
    `${banner}window.__ZIPTOOL_LOCALES__ ??= {};\nwindow.__ZIPTOOL_LOCALES__[${JSON.stringify(code)}] = ${body};\n`
  );
}
console.log("scaffolded: zh-HK zh-TW en ru (temporary zh-CN copy)");
