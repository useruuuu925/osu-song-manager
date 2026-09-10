// key 一致性 lint：zh.ts / en.ts 的 key 集合必须全等（缺译/多译都算失败）。
// tsconfig 的 Record<keyof typeof zh, string> 已在 vue-tsc 层兜底，这里做独立于 TS 的双保险。
// 运行：node scripts/check-locales.mjs（npm run build 首步自动执行）。
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

function keysOf(file) {
  const text = readFileSync(join(root, file), "utf-8");
  const keys = new Set();
  // 只匹配字符串字面量 key（所有 key 都带引号，含点号）
  const re = /"([A-Za-z0-9_.]+)":\s*"/g;
  let m;
  while ((m = re.exec(text)) !== null) keys.add(m[1]);
  return keys;
}

const zh = keysOf("src/i18n/zh.ts");
const en = keysOf("src/i18n/en.ts");

const problems = [];
for (const k of zh) if (!en.has(k)) problems.push(`en 缺少 key: ${k}`);
for (const k of en) if (!zh.has(k)) problems.push(`en 多出 key: ${k}`);
if (zh.size === 0) problems.push("zh.ts 未解析到任何 key（解析器失效？）");

if (problems.length) {
  console.error(`[check-locales] ${problems.length} 处不一致：`);
  for (const p of problems) console.error("  - " + p);
  process.exit(1);
}
console.log(`[check-locales] OK: zh/en key 集合全等（${zh.size} keys）`);
