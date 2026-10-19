import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const src = resolve(root, "node_modules/i18next/dist/umd/i18next.min.js");
const license = resolve(root, "node_modules/i18next/LICENSE");
const outDir = resolve(root, "ui/vendor");

mkdirSync(outDir, { recursive: true });
copyFileSync(src, resolve(outDir, "i18next.min.js"));
copyFileSync(license, resolve(outDir, "i18next.LICENSE"));
console.log("vendored i18next -> ui/vendor/");
