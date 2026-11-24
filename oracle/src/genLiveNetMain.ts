import { app, BrowserWindow } from "electron";
import { createHash } from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import type { AddressInfo } from "node:net";

// Electron main process for `genLiveNet` (not run under ELECTRON_RUN_AS_NODE). Every host and port
// resolves to one loopback server, so nothing leaves the machine whatever the URL says. One record
// per fixture item:
//   "addresses" with "urls": {"url", "api", "protocols", "result", "requests"}: the URL built from an
//     address, what the page's call settled with and what reached the server.
//   "ports": {"ports", "blocked"}: the ports of the range a request never reaches.
//   "bodies": {"body", "text", "json"}: what `Response.text()` and `Response.json()` read from bytes.
//   "timers": {"timers", "order"}: the order the timers' tasks ran in.

const request = JSON.parse(fs.readFileSync(process.argv[process.argv.length - 1], "utf8"));
const { fixture } = request;

function fail(message: string): never {
  process.stderr.write(`genLiveNetMain: ${message}\n`);
  app.exit(1);
  throw new Error(message);
}

const PAGE_SCRIPT = `(() => {
  const thrown = (error) => ({ name: error.name, message: error.message });
  window.__fetch = async (url) => {
    try {
      const response = await fetch(url);
      return { status: response.status, url: response.url };
    } catch (error) {
      return { rejected: thrown(error) };
    }
  };
  window.__ws = (url, protocols) => {
    let ws;
    try {
      ws = protocols.length === 0 ? new WebSocket(url) : new WebSocket(url, protocols);
    } catch (error) {
      return { threw: thrown(error) };
    }
    return new Promise((resolve) => {
      const events = [];
      ws.onopen = () => {
        events.push("open");
        ws.close();
      };
      ws.onerror = () => events.push("error");
      ws.onclose = () => resolve({ url: ws.url, protocol: ws.protocol, events: [...events, "close"] });
    });
  };
  window.__reach = async (from, to) => {
    const blocked = [];
    const request = (port) => fetch("http://robot.local:" + port + "/").catch(() => blocked.push(port));
    await Promise.all(Array.from({ length: to - from }, (_, i) => request(from + i)));
    return blocked;
  };
  window.__body = async (url) => {
    const read = async (how) => {
      const response = await fetch(url);
      try {
        return { value: await response[how]() };
      } catch (error) {
        return { rejected: error.name };
      }
    };
    return { text: await read("text"), json: await read("json") };
  };
  const channel = new MessageChannel();
  const queue = [];
  channel.port1.onmessage = () => queue.shift()();
  const inTask = (run) => {
    queue.push(run);
    channel.port2.postMessage(0);
  };
  const quiet = () => new Promise((resolve) => {
    const start = performance.now();
    const spin = () => (performance.now() - start > 5 ? resolve() : inTask(spin));
    inTask(spin);
  });
  const trial = (timers) => new Promise((resolve) => inTask(() => {
    const order = [];
    const counts = {};
    let left = timers.reduce((sum, timer) => sum + (timer.repeats ?? 1), 0);
    const fired = (timer, id) => {
      order.push(timer.name);
      counts[timer.name] = (counts[timer.name] ?? 0) + 1;
      if (timer.repeats !== undefined && counts[timer.name] === timer.repeats) clearInterval(id.current);
      left--;
      if (left === 0) resolve(order);
      create(timer.name, counts[timer.name]);
    };
    const create = (name, count) => {
      for (const timer of timers) {
        if ((timer.after?.[0] ?? null) !== name || (timer.after?.[1] ?? 0) !== count) continue;
        const id = {};
        const run = () => fired(timer, id);
        if (timer.kind === "interval") id.current = setInterval(run, timer.delay);
        else if (timer.kind === "timeout") setTimeout(run, timer.delay);
        else if (timer.kind === "abort") AbortSignal.timeout(timer.delay).addEventListener("abort", run);
        else throw new Error(timer.kind);
      }
    };
    create(null, 0);
  }));
  window.__timers = async (timers, trials) => {
    const counts = {};
    for (let i = 0; i < trials; i++) {
      const order = JSON.stringify(await trial(timers));
      counts[order] = (counts[order] ?? 0) + 1;
      await quiet();
    }
    return counts;
  };
  return true;
})()`;

const observed: object[] = [];
let body: Buffer = Buffer.alloc(0);

const server = http.createServer((incoming, response) => {
  observed.push({ method: incoming.method, target: incoming.url, host: incoming.headers.host, authorization: incoming.headers.authorization ?? null });
  response.writeHead(200, { "Access-Control-Allow-Origin": "*", "Content-Length": body.length, Connection: "close" });
  response.end(body);
});
server.on("upgrade", (incoming, socket) => {
  const protocols = incoming.headers["sec-websocket-protocol"];
  observed.push({ method: "WS", target: incoming.url, host: incoming.headers.host, authorization: incoming.headers.authorization ?? null, protocols: protocols ?? null });
  const accept = createHash("sha1")
    .update(incoming.headers["sec-websocket-key"] + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11")
    .digest("base64");
  const selected = protocols === undefined ? "" : `Sec-WebSocket-Protocol: ${protocols.split(",")[0].trim()}\r\n`;
  socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n${selected}\r\n`);
  socket.on("data", () => socket.end(Buffer.from([0x88, 0x00])));
  socket.on("error", () => {});
});

const settled = <T>(what: string, promise: Promise<T>) =>
  Promise.race([promise, new Promise<never>((_resolve, reject) => setTimeout(() => reject(new Error(`${what} did not settle`)), 120000).unref())]);

async function record(): Promise<string[]> {
  const port = (server.address() as AddressInfo).port;
  const win = new BrowserWindow({ width: 400, height: 300, show: false, webPreferences: { offscreen: true, backgroundThrottling: false } });
  win.webContents.on("render-process-gone", (_event, details) => fail(`renderer gone: ${details.reason}`));
  await win.loadFile(request.page);
  await win.webContents.executeJavaScript(PAGE_SCRIPT);
  const call = (name: string, ...args: unknown[]): Promise<any> =>
    settled(`${name} ${JSON.stringify(args)}`, win.webContents.executeJavaScript(`window.${name}(${args.map((arg) => JSON.stringify(arg)).join(", ")})`));
  const lines: string[] = [];
  for (const address of (fixture.addresses ?? []) as string[]) {
    for (const { api, prefix, suffix, protocols } of fixture.urls) {
      const url = prefix + address + suffix;
      observed.length = 0;
      const result = api === "fetch" ? await call("__fetch", url) : await call("__ws", url, protocols);
      lines.push(JSON.stringify({ url, api, protocols, result, requests: observed }));
    }
  }
  if (fixture.ports !== undefined) {
    const [first, last] = fixture.ports as [number, number];
    const blocked: number[] = [];
    for (let from = first; from <= last; from += 64) {
      const to = Math.min(from + 64, last + 1);
      observed.length = 0;
      const failed: number[] = await call("__reach", from, to);
      if (observed.length !== to - from - failed.length) fail(`ports ${from} to ${to}: ${observed.length} requests, ${failed.length} failures`);
      blocked.push(...failed);
    }
    lines.push(JSON.stringify({ ports: fixture.ports, blocked: blocked.sort((a, b) => a - b) }));
  }
  for (const hex of (fixture.bodies ?? []) as string[]) {
    body = Buffer.from(hex, "hex");
    lines.push(JSON.stringify({ body: hex, ...(await call("__body", `http://127.0.0.1:${port}/`)) }));
  }
  for (const timers of (fixture.timers ?? []) as object[]) {
    const counts: { [order: string]: number } = await call("__timers", timers, fixture.trials);
    const [order, count] = Object.entries(counts).sort((a, b) => b[1] - a[1])[0];
    if (count < fixture.trials * fixture.share) fail(`${JSON.stringify(timers)}: no settled order: ${JSON.stringify(counts)}`);
    lines.push(JSON.stringify({ timers, order: JSON.parse(order) }));
  }
  return lines;
}

app.setPath("userData", request.userData);
app.dock?.hide();
app.disableHardwareAcceleration();
// The switch has to be set before Electron is ready, so listening happens first.
await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
app.commandLine.appendSwitch("host-resolver-rules", `MAP * 127.0.0.1:${(server.address() as AddressInfo).port}`);
app
  .whenReady()
  .then(async () => {
    fs.writeFileSync(request.output, (await record()).map((line) => line + "\n").join(""));
    app.exit(0);
  })
  .catch((error) => fail(String(error?.stack ?? error)));
