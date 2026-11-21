import * as fs from "node:fs";
import * as path from "node:path";

export function generatedFiles(dir: string, keep: (rel: string) => boolean): string[] {
  const name = path.basename(dir);
  const manifestPath = path.join(process.cwd(), "parity/corpus/synthetic", `${name}.manifest.json`);
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  const expected: string[] = manifest.list
    .flatMap((c: { files: { path: string }[] }) => c.files.map((f) => f.path))
    .filter(keep);
  const missing = expected.filter((rel) => !fs.existsSync(path.join(dir, rel)));
  if (missing.length > 0) {
    console.error(
      `corpus ${name}: ${missing.length} of ${expected.length} expected files are missing under ${dir} ` +
        `(first: ${missing[0]}); run \`cargo xtask corpus gen ${name}\``
    );
    process.exit(1);
  }
  return expected;
}
