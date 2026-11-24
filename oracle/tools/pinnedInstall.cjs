// Writes a package.json and package-lock.json that install only <package> and its dependencies,
// with the versions, URLs and integrity hashes of <lockfile>, for `npm ci` in a new <dir>.
// usage: node pinnedInstall.cjs <lockfile> <dir> <package>
const fs = require("node:fs");
const path = require("node:path");
const [lockfile, dir, root] = process.argv.slice(2);
const { lockfileVersion, packages } = JSON.parse(fs.readFileSync(lockfile, "utf8"));
const picked = {};
function pick(name) {
  const key = `node_modules/${name}`;
  if (picked[key]) return;
  const entry = packages[key];
  if (!entry?.integrity?.startsWith("sha512-") || !entry.resolved.startsWith("https://registry.npmjs.org/")) {
    throw new Error(`${lockfile}: ${key} is not pinned to a registry tarball`);
  }
  if (Object.keys(packages).some((k) => k.startsWith(`${key}/node_modules/`))) {
    throw new Error(`${lockfile}: ${key} has nested dependencies`);
  }
  const { dev, ...rest } = entry;
  picked[key] = rest;
  Object.keys(entry.dependencies ?? {}).forEach(pick);
}
pick(root);
const manifest = { private: true, dependencies: { [root]: picked[`node_modules/${root}`].version } };
fs.mkdirSync(path.dirname(dir), { recursive: true });
fs.mkdirSync(dir);
fs.writeFileSync(path.join(dir, "package.json"), JSON.stringify(manifest, null, 2) + "\n");
fs.writeFileSync(
  path.join(dir, "package-lock.json"),
  JSON.stringify({ lockfileVersion, requires: true, packages: { "": manifest, ...picked } }, null, 2) + "\n"
);
