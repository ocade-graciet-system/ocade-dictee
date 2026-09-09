// Vérifie que chaque clé i18n référencée littéralement dans le code source
// (`t("a.b.c")`, `t('a.b.c')`, `i18nKey="a.b.c"`) existe dans la seule locale
// embarquée, fr/translation.json. Les clés construites dynamiquement
// (`t(\`x.${y}\`)`) ne sont pas vérifiables et sont ignorées.
import fs from "fs";
import path from "path";
import { fileURLToPath } from "url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const FR_PATH = path.join(
  ROOT,
  "src",
  "i18n",
  "locales",
  "fr",
  "translation.json",
);
const fr = JSON.parse(fs.readFileSync(FR_PATH, "utf8")) as Record<
  string,
  unknown
>;

function hasKey(obj: Record<string, unknown>, dotted: string): boolean {
  let cur: unknown = obj;
  for (const part of dotted.split(".")) {
    if (typeof cur !== "object" || cur === null || !(part in (cur as object)))
      return false;
    cur = (cur as Record<string, unknown>)[part];
  }
  return true;
}

function sourceFiles(dir: string, out: string[] = []): string[] {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name !== "node_modules") sourceFiles(full, out);
    } else if (
      /\.(ts|tsx)$/.test(entry.name) &&
      !entry.name.endsWith(".d.ts") &&
      entry.name !== "bindings.ts"
    ) {
      out.push(full);
    }
  }
  return out;
}

const PATTERNS = [
  /\bt\(\s*(["'])([A-Za-z0-9_.-]+)\1/g,
  /i18nKey=(["'])([A-Za-z0-9_.-]+)\1/g,
];
const missing: string[] = [];
for (const file of sourceFiles(path.join(ROOT, "src"))) {
  const src = fs.readFileSync(file, "utf8");
  for (const re of PATTERNS) {
    for (const match of src.matchAll(re)) {
      const key = match[2];
      if (!hasKey(fr, key))
        missing.push(`${path.relative(ROOT, file)} → ${key}`);
    }
  }
}

if (missing.length > 0) {
  console.error(
    `❌ ${missing.length} clé(s) i18n absente(s) de fr/translation.json :`,
  );
  for (const line of [...new Set(missing)].sort()) console.error(`   ${line}`);
  process.exit(1);
}
console.log("✅ Toutes les clés i18n utilisées existent en français.");
