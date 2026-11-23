import path from "node:path";
import { pathToFileURL } from "node:url";

// Upstream's own bundles with source maps, written apart from the ones the oracle runs. Run with
// upstream as the working directory and $ASRS_COVERAGE_BUNDLES as the output directory.
const { default: upstream } = await import(pathToFileURL(path.resolve("rolldown.config.mjs")).href);

export default (args) =>
  upstream(args).map((config) => ({
    ...config,
    output: { ...config.output, dir: process.env.ASRS_COVERAGE_BUNDLES, sourcemap: true }
  }));
