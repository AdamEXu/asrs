import type { Config } from "@docusaurus/types";
import fs from "node:fs";
import path from "node:path";
import embed from "./docusaurus-embed.config";

const LISTING = "THIRD_PARTY_LICENSES.txt";
const PACKAGE = /^(.*[\\/]node_modules[\\/](?:@[^\\/]+[\\/])?[^\\/]+)[\\/]/;

const packages = new Set<string>();

function collect(modules: any[] | undefined) {
  for (const module of modules ?? []) {
    const resource = (module.nameForCondition ?? "").split("?")[0];
    const match = PACKAGE.exec(resource);
    if (match) packages.add(match[1]);
    collect(module.modules);
  }
}

const recordPackages = {
  apply(compiler: any) {
    compiler.hooks.done.tap("asrs-licenses", (stats: any) => {
      collect(
        stats.toJson({ all: false, modules: true, nestedModules: true, modulesSpace: Infinity, nestedModulesSpace: Infinity }).modules
      );
    });
  }
};

function readme(dir: string): string[] {
  const name = fs.readdirSync(dir).find((name) => /^readme/i.test(name));
  if (name === undefined) return [];
  const text = fs.readFileSync(path.join(dir, name), "utf8").replace(/\r\n/g, "\n");
  const start = text.search(/^#+ License$/im);
  return start < 0 ? [] : [text.slice(start).replace(/&lt;/g, "<").replace(/&gt;/g, ">").trim()];
}

function notice(dir: string): string {
  const info = JSON.parse(fs.readFileSync(path.join(dir, "package.json"), "utf8"));
  const supplied = path.join(__dirname, "asrs-licenses", `${info.name}.txt`);
  const files = fs
    .readdirSync(dir)
    .filter((name) => /^(licen[cs]e|copying|notice)/i.test(name))
    .sort()
    .map((name) => path.join(dir, name));
  if (files.length === 0 && fs.existsSync(supplied)) files.push(supplied);
  const texts = files.map((file) => fs.readFileSync(file, "utf8").replace(/\r\n/g, "\n").trim());
  if (texts.length === 0) texts.push(...readme(dir));
  if (texts.length === 0) throw new Error(`${info.name} ${info.version} (${info.license}) ships no license text`);
  return `${info.name} ${info.version} (${info.license})\n\n${texts.join("\n\n")}\n`;
}

export default {
  ...embed,
  plugins: [
    ...(embed.plugins ?? []),
    () => ({
      name: "asrs-licenses",
      configureWebpack: () => ({ plugins: [recordPackages] }),
      async postBuild({ outDir }: { outDir: string }) {
        const rule = `\n${"-".repeat(80)}\n\n`;
        const notices = [...new Set([...packages].map(notice))].sort();
        if (notices.length === 0) throw new Error("no bundled packages were recorded");
        fs.writeFileSync(path.join(outDir, LISTING), notices.join(rule));
      }
    })
  ]
} satisfies Config;
