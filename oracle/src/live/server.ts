import { ServerSide } from "./loop";

/** What a mock server can do to the client. The network records each call and delivers it. */
export interface Outbox {
  wsAccept(id: number, protocol: string): void;
  wsReject(id: number): void;
  wsPush(id: number, data: string | Uint8Array): void;
  wsDrop(id: number): void;
  httpReply(id: number, status: number, body: string): void;
  httpRefuse(id: number): void;
  tcpAccept(id: number): void;
  tcpRefuse(id: number): void;
  tcpPush(id: number, data: Uint8Array): void;
  tcpDrop(id: number): void;
}

/** Client traffic as it reaches the server. */
export interface ClientTraffic {
  wsOpen(id: number, url: string, protocols: string[]): void;
  wsSend(id: number, data: string | Uint8Array): void;
  wsClose(id: number): void;
  httpGet(id: number, url: string): void;
  httpAbort(id: number): void;
  tcpOpen(id: number, host: string, port: number): void;
  tcpSend(id: number, data: Uint8Array): void;
  tcpClose(id: number): void;
}

export interface ServerContext {
  epochMs: number;
  now: () => number;
}

/** The port in an absolute `ws:`/`http:` URL, or 80 when it names none. */
export function urlPort(url: string): number {
  const start = url.indexOf("://");
  let rest = start < 0 ? url : url.slice(start + 3);
  const end = rest.search(/[/?#]/);
  if (end >= 0) rest = rest.slice(0, end);
  const colon = rest.lastIndexOf(":");
  if (colon < 0) return 80;
  const digits = rest.slice(colon + 1);
  return /^[0-9]+$/.test(digits) ? Number(digits) : -1;
}

/** Query parameters in order; a malformed escape leaves that parameter out. */
export function urlQuery(url: string): Map<string, string> {
  const out = new Map<string, string>();
  const q = url.indexOf("?");
  if (q < 0) return out;
  for (const part of url.slice(q + 1).split("&")) {
    const eq = part.indexOf("=");
    const [k, v] = eq < 0 ? [part, ""] : [part.slice(0, eq), part.slice(eq + 1)];
    try {
      const key = decodeURIComponent(k);
      if (!out.has(key)) out.set(key, decodeURIComponent(v));
    } catch {}
  }
  return out;
}

/**
 * Rejects JSON numbers that JS and Rust would print differently, so every JSON text a mock
 * server writes is the same on both sides: safe integers, and other finite numbers with
 * magnitude in [1e-6, 1e21). `-0` is out.
 */
export function checkJsonNumbers(value: unknown, where: string): void {
  if (typeof value === "number") {
    const ok =
      !Object.is(value, -0) &&
      (Number.isSafeInteger(value) || (Number.isFinite(value) && Math.abs(value) >= 1e-6 && Math.abs(value) < 1e21));
    if (!ok) throw new Error(`${where}: number ${value} cannot be written identically by both mocks`);
  } else if (Array.isArray(value)) {
    value.forEach((item, i) => checkJsonNumbers(item, `${where}[${i}]`));
  } else if (value !== null && typeof value === "object") {
    for (const [k, v] of Object.entries(value)) checkJsonNumbers(v, `${where}.${k}`);
  }
}

export function hexToBytes(hex: string): Uint8Array {
  if (!/^([0-9a-f]{2})*$/.test(hex)) throw new Error(`not lowercase hex: ${JSON.stringify(hex)}`);
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.slice(2 * i, 2 * i + 2), 16);
  return out;
}

/**
 * Behaviour every mock shares. Steps: `["up"]`, `["down", "refuse" | "hang"]` (new connections
 * and requests are refused or never answered; open ones stay), `["mute", bool]` (pushes on open
 * connections are dropped), `["drop"]` (the server closes every open connection, websockets
 * first, each in opening order).
 */
export abstract class MockServer implements ClientTraffic {
  protected up = true;
  protected downMode = "refuse";
  protected muted = false;
  protected ws = new Map<number, string>();
  protected tcp = new Set<number>();

  constructor(
    protected out: Outbox,
    protected ctx: ServerContext
  ) {}

  protected abstract ports(): number[];
  protected acceptWs(_url: string, _protocols: string[]): string | null {
    return null;
  }
  protected wsOpened(_id: number): void {}
  protected wsMessage(_id: number, _data: string | Uint8Array): void {}
  protected http(_url: string): [number, string] | null {
    return null;
  }
  protected tcpData(_id: number, _data: Uint8Array): void {}
  protected abstract serverStep(op: any[]): void;

  /** Whether a new connection to `port` gets an answer, after refusing it when it must. */
  private answers(port: number, refuse: () => void): boolean {
    if (!this.ports().includes(port)) {
      refuse();
      return false;
    }
    if (!this.up) {
      if (this.downMode === "refuse") refuse();
      return false;
    }
    return true;
  }

  wsOpen(id: number, url: string, protocols: string[]): void {
    if (!this.answers(urlPort(url), () => this.out.wsReject(id))) return;
    const protocol = this.acceptWs(url, protocols);
    if (protocol === null) {
      this.out.wsReject(id);
      return;
    }
    this.ws.set(id, protocol);
    this.out.wsAccept(id, protocol);
    this.wsOpened(id);
  }

  wsSend(id: number, data: string | Uint8Array): void {
    if (this.ws.has(id)) this.wsMessage(id, data);
  }

  wsClose(id: number): void {
    this.ws.delete(id);
  }

  httpGet(id: number, url: string): void {
    if (!this.answers(urlPort(url), () => this.out.httpRefuse(id))) return;
    const reply = this.http(url);
    if (reply === null) this.out.httpRefuse(id);
    else this.out.httpReply(id, reply[0], reply[1]);
  }

  httpAbort(_id: number): void {}

  tcpOpen(id: number, _host: string, port: number): void {
    if (!this.answers(port, () => this.out.tcpRefuse(id))) return;
    this.tcp.add(id);
    this.out.tcpAccept(id);
  }

  tcpSend(id: number, data: Uint8Array): void {
    if (this.tcp.has(id)) this.tcpData(id, data);
  }

  tcpClose(id: number): void {
    this.tcp.delete(id);
  }

  protected pushWs(id: number, data: string | Uint8Array): void {
    if (!this.muted && this.ws.has(id)) this.out.wsPush(id, data);
  }

  protected pushTcp(id: number, data: Uint8Array): void {
    if (!this.muted && this.tcp.has(id)) this.out.tcpPush(id, data);
  }

  step(op: any[]): void {
    switch (op[0]) {
      case "up":
        this.up = true;
        break;
      case "down":
        if (op[1] !== "refuse" && op[1] !== "hang") throw new Error(`down: mode must be "refuse" or "hang"`);
        this.up = false;
        this.downMode = op[1];
        break;
      case "mute":
        this.muted = op[1] === true;
        break;
      case "drop":
        for (const id of this.ws.keys()) this.out.wsDrop(id);
        for (const id of this.tcp) this.out.tcpDrop(id);
        this.ws.clear();
        this.tcp.clear();
        break;
      default:
        this.serverStep(op);
    }
  }
}

/**
 * Runs a mock server on the session clock: client traffic arrives `latencyMs` after it is sent,
 * and at each time the server takes every arrival (in send order) before its steps for that
 * time (in file order).
 */
export class ServerHost implements ServerSide {
  private queue: { t: number; kind: number; seq: number; run: () => void }[] = [];
  private seq = 0;

  constructor(
    readonly server: MockServer,
    steps: { t: number; op: any[] }[],
    private latencyMs: number,
    private now: () => number
  ) {
    for (const step of steps) this.queue.push({ t: step.t, kind: 1, seq: this.seq++, run: () => server.step(step.op) });
  }

  arrive(run: (server: MockServer) => void): void {
    this.queue.push({ t: this.now() + this.latencyMs, kind: 0, seq: this.seq++, run: () => run(this.server) });
  }

  nextTime(): number | null {
    return this.queue.length === 0 ? null : Math.min(...this.queue.map((q) => q.t));
  }

  process(t: number): void {
    const due = this.queue.filter((q) => q.t === t).sort((a, b) => a.kind - b.kind || a.seq - b.seq);
    this.queue = this.queue.filter((q) => q.t !== t);
    for (const item of due) item.run();
  }
}
