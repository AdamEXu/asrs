const out = (v: unknown) => process.stdout.write(JSON.stringify(v) + "\n");
function deepArr(d: number): unknown {
  let v: unknown = [];
  for (let i = 0; i < d; i++) v = [v];
  return v;
}
function attempt(f: () => unknown): string {
  try { f(); return "ok"; } catch (e) { return `${(e as Error).name}`; }
}
const d = 20000;
out({ u2: attempt(() => JSON.stringify(deepArr(d), undefined, 2)) });
out({ u0: attempt(() => JSON.stringify(deepArr(d), undefined, "")) });
out({ ux: attempt(() => JSON.stringify(deepArr(d), undefined, "x")) });
out({ uu: attempt(() => JSON.stringify(deepArr(d), undefined, undefined)) });
// replacer arrays (property list) - spec-valid replacer
out({ rarr: attempt(() => JSON.stringify(deepArr(d), ["a"])) });
// exact threshold for the recursive path, objects too
function deepObj(n: number): unknown { let v: unknown = {}; for (let i = 0; i < n; i++) v = { a: v }; return v; }
function threshold(f: (d: number) => unknown, hi: number): string {
  let lo = 500;
  if (attempt(() => f(hi)) === "ok") return `>${hi}`;
  while (hi - lo > 2) {
    const mid = (lo + hi) >> 1;
    if (attempt(() => f(mid)) === "ok") lo = mid; else hi = mid;
  }
  return `${lo}`;
}
out({ "thr-arr": threshold((n) => JSON.stringify(deepArr(n), null), 200000) });
out({ "thr-obj": threshold((n) => JSON.stringify(deepObj(n), null), 200000) });
