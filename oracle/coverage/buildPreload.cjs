"use strict";
// Loaded into build.mjs by `oracle coverage`: the same bundles with source maps, and a file URL
// on the page bundles, which reach V8 as an eval or inside another script.
const fs = require("node:fs");
const path = require("node:path");
const Module = require("node:module");
const { pathToFileURL } = require("node:url");

const load = Module._load;
Module._load = function (request, ...rest) {
  const exported = load.call(this, request, ...rest);
  if (request !== "esbuild") return exported;
  return {
    ...exported,
    build: async (options) => {
      const result = await exported.build({ ...options, sourcemap: "linked" });
      if (options.format === "iife") {
        const file = path.resolve(options.outfile);
        fs.appendFileSync(file, `//# sourceURL=${pathToFileURL(file).href}\n`);
      }
      return result;
    }
  };
};
