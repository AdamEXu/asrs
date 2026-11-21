import { toTree } from "../cve1";
import { LiveLoop } from "./loop";
import { ClientTraffic, Outbox, ServerHost } from "./server";

/** The session trace: one JSON line per event, in the order events happen. */
export class Trace {
  readonly lines: string[] = [];

  constructor(private loop: LiveLoop) {}

  add(ev: string, id: number | undefined, ...v: unknown[]): void {
    const line: { [key: string]: unknown } = { t: this.loop.now, ev };
    if (id !== undefined) line.id = id;
    if (v.length > 0) line.v = toTree(v[0]);
    this.lines.push(JSON.stringify(line));
  }
}

type Listener = (event: any) => void;

class Emitter {
  private listeners: { [type: string]: Listener[] } = {};
  private handlers: { [type: string]: Listener | null } = {};

  addEventListener(type: string, listener: Listener): void {
    const list = (this.listeners[type] ??= []);
    if (!list.includes(listener)) list.push(listener);
  }

  removeEventListener(type: string, listener: Listener): void {
    const list = this.listeners[type];
    if (list) this.listeners[type] = list.filter((l) => l !== listener);
  }

  /** `on<type>`: the first assignment registers a listener slot that later assignments reuse. */
  protected setHandler(type: string, handler: Listener | null): void {
    if (!(type in this.handlers)) {
      this.addEventListener(type, (event) => this.handlers[type]?.call(this, event));
    }
    this.handlers[type] = handler;
  }

  protected getHandler(type: string): Listener | null {
    return this.handlers[type] ?? null;
  }

  protected dispatch(type: string, event: any): void {
    for (const listener of (this.listeners[type] ?? []).slice()) {
      try {
        listener.call(this, event);
      } catch {}
    }
  }
}

class FakeCloseEvent {
  readonly type: string;
  readonly code: number;
  readonly reason: string;
  readonly wasClean: boolean;

  constructor(type: string, init: { code?: number; reason?: string; wasClean?: boolean } = {}) {
    this.type = type;
    this.code = init.code ?? 0;
    this.reason = init.reason ?? "";
    this.wasClean = init.wasClean ?? false;
  }
}

function bytesOf(data: unknown): Uint8Array {
  if (data instanceof ArrayBuffer) return new Uint8Array(data.slice(0));
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength));
  throw new TypeError("unsupported websocket payload");
}

/**
 * The in-memory network between the session's client side (the upstream source, plus the
 * emulated main process) and its mock server. Each client action is recorded and reaches the
 * server `latencyMs` later; each server action is recorded and reaches the client `latencyMs`
 * later. Connection and request ids count up from 1 across websockets, requests and sockets.
 */
export class Net implements Outbox {
  private nextId = 1;
  private sockets = new Map<number, any>();
  private requests = new Map<number, { resolve: (r: unknown) => void; reject: (e: unknown) => void; url: string; done: boolean }>();

  constructor(
    private loop: LiveLoop,
    private host: ServerHost,
    private trace: Trace,
    private latencyMs: number
  ) {}

  private toServer(ev: string, id: number, v: unknown[], run: (s: ClientTraffic) => void): void {
    this.trace.add(ev, id, ...v);
    this.host.arrive(run);
  }

  private toClient(ev: string, id: number, v: unknown[], run: () => void): void {
    this.trace.add(ev, id, ...v);
    this.loop.deliver(this.latencyMs, run);
  }

  wsAccept(id: number, protocol: string): void {
    this.toClient("ws-accept", id, [protocol], () => this.sockets.get(id)?.accepted(protocol));
  }

  wsReject(id: number): void {
    this.toClient("ws-reject", id, [], () => this.sockets.get(id)?.failed());
  }

  wsPush(id: number, data: string | Uint8Array): void {
    this.toClient("ws-push", id, [data], () => this.sockets.get(id)?.received(data));
  }

  wsDrop(id: number): void {
    this.toClient("ws-drop", id, [], () => this.sockets.get(id)?.dropped());
  }

  httpReply(id: number, status: number, body: string): void {
    this.toClient("http-reply", id, [[status, body]], () => this.settle(id, status, body));
  }

  httpRefuse(id: number): void {
    this.toClient("http-refuse", id, [], () => this.settle(id, null, ""));
  }

  tcpAccept(id: number): void {
    this.toClient("tcp-accept", id, [], () => this.sockets.get(id)?.accepted());
  }

  tcpRefuse(id: number): void {
    this.toClient("tcp-refuse", id, [], () => this.sockets.get(id)?.refused());
  }

  tcpPush(id: number, data: Uint8Array): void {
    this.toClient("tcp-push", id, [data], () => this.sockets.get(id)?.received(data));
  }

  tcpDrop(id: number): void {
    this.toClient("tcp-drop", id, [], () => this.sockets.get(id)?.dropped());
  }

  private settle(id: number, status: number | null, body: string): void {
    const request = this.requests.get(id);
    if (request === undefined || request.done) return;
    request.done = true;
    if (status === null) {
      request.reject(new TypeError("Failed to fetch"));
      return;
    }
    request.resolve({
      ok: status >= 200 && status < 300,
      status,
      url: new URL(request.url).href,
      json: () => Promise.resolve().then(() => JSON.parse(body)),
      text: () => Promise.resolve(body)
    });
  }

  /**
   * `fetch` for GET requests. An abort reaches the server as `http-abort`. A URL that does not
   * parse rejects before any network activity, as Chromium's `Request` constructor throws, but
   * still takes an id.
   */
  fetch(url: string, init?: { signal?: AbortSignal }): Promise<unknown> {
    const signal = init?.signal;
    if (signal?.aborted) return Promise.reject(signal.reason);
    const id = this.nextId++;
    if (!URL.canParse(url)) return Promise.reject(new TypeError("Failed to fetch"));
    return new Promise((resolve, reject) => {
      const request = { resolve, reject, url, done: false };
      this.requests.set(id, request);
      signal?.addEventListener("abort", () => {
        if (request.done) return;
        request.done = true;
        this.toServer("http-abort", id, [], (s) => s.httpAbort(id));
        reject(signal.reason);
      });
      this.toServer("http-get", id, [url], (s) => s.httpGet(id, url));
    });
  }

  /** Installs the client-side globals: `WebSocket`, `CloseEvent`, `fetch`, `AbortSignal.timeout`. */
  install(): () => void {
    const g = globalThis as any;
    const saved = { WebSocket: g.WebSocket, CloseEvent: g.CloseEvent, fetch: g.fetch, timeout: AbortSignal.timeout };
    g.WebSocket = this.webSocketClass();
    g.CloseEvent = FakeCloseEvent;
    g.fetch = (url: string, init?: { signal?: AbortSignal }) => this.fetch(String(url), init);
    (AbortSignal as any).timeout = (ms: number) => {
      const controller = new AbortController();
      this.loop.postDelayedTask(() => controller.abort(new DOMException("signal timed out", "TimeoutError")), ms);
      return controller.signal;
    };
    return () => {
      g.WebSocket = saved.WebSocket;
      g.CloseEvent = saved.CloseEvent;
      g.fetch = saved.fetch;
      (AbortSignal as any).timeout = saved.timeout;
    };
  }

  /**
   * Browser `WebSocket` semantics, reduced to what the sources use: `send` while connecting
   * throws, after `close()` it is ignored; incoming messages are dropped unless open; a
   * refused handshake fires `error` then `close` (1006); a server drop fires `close` (1006);
   * `close()` fires `close` (1005, clean) after the closing handshake, a round trip later.
   */
  private webSocketClass(): unknown {
    const net = this;
    return class FakeWebSocket extends Emitter {
      static readonly CONNECTING = 0;
      static readonly OPEN = 1;
      static readonly CLOSING = 2;
      static readonly CLOSED = 3;
      readonly CONNECTING = 0;
      readonly OPEN = 1;
      readonly CLOSING = 2;
      readonly CLOSED = 3;

      readonly url: string;
      readyState = 0;
      protocol = "";
      binaryType = "blob";
      private id: number;

      constructor(url: string, protocols?: string | string[]) {
        super();
        this.url = String(url);
        const list = protocols === undefined ? [] : typeof protocols === "string" ? [protocols] : [...protocols];
        this.id = net.nextId++;
        net.sockets.set(this.id, this);
        const id = this.id;
        net.toServer("ws-open", id, [[this.url, list]], (s) => s.wsOpen(id, this.url, list));
      }

      set onopen(h: Listener | null) {
        this.setHandler("open", h);
      }
      get onopen() {
        return this.getHandler("open");
      }
      set onmessage(h: Listener | null) {
        this.setHandler("message", h);
      }
      get onmessage() {
        return this.getHandler("message");
      }
      set onclose(h: Listener | null) {
        this.setHandler("close", h);
      }
      get onclose() {
        return this.getHandler("close");
      }
      set onerror(h: Listener | null) {
        this.setHandler("error", h);
      }
      get onerror() {
        return this.getHandler("error");
      }

      send(data: unknown): void {
        if (this.readyState === 0) throw new DOMException("Still in CONNECTING state.", "InvalidStateError");
        if (this.readyState !== 1) return;
        const payload = typeof data === "string" ? data : bytesOf(data);
        const id = this.id;
        net.toServer("ws-send", id, [payload], (s) => s.wsSend(id, payload));
      }

      close(): void {
        if (this.readyState >= 2) return;
        const wasConnecting = this.readyState === 0;
        this.readyState = 2;
        const id = this.id;
        net.toServer("ws-close", id, [], (s) => s.wsClose(id));
        net.loop.deliver(2 * net.latencyMs, () => {
          if (this.readyState === 3) return;
          this.readyState = 3;
          if (wasConnecting) {
            this.dispatch("error", { type: "error" });
            this.dispatch("close", new FakeCloseEvent("close", { code: 1006 }));
          } else {
            this.dispatch("close", new FakeCloseEvent("close", { code: 1005, wasClean: true }));
          }
        });
      }

      accepted(protocol: string): void {
        if (this.readyState !== 0) return;
        this.readyState = 1;
        this.protocol = protocol;
        this.dispatch("open", { type: "open" });
      }

      failed(): void {
        if (this.readyState === 3) return;
        this.readyState = 3;
        this.dispatch("error", { type: "error" });
        this.dispatch("close", new FakeCloseEvent("close", { code: 1006 }));
      }

      received(data: string | Uint8Array): void {
        if (this.readyState !== 1) return;
        let payload: unknown = data;
        if (typeof data !== "string") {
          const copy = data.slice();
          payload = this.binaryType === "arraybuffer" ? copy.buffer : new Blob([copy]);
        }
        this.dispatch("message", { type: "message", data: payload });
      }

      dropped(): void {
        if (this.readyState === 3) return;
        this.readyState = 3;
        this.dispatch("close", new FakeCloseEvent("close", { code: 1006 }));
      }
    };
  }

  /**
   * `net.createConnection` for the emulated main process, with the Node behaviour main.ts
   * relies on: writes while connecting are held and go out on connect; an idle timeout
   * (`setTimeout`) fires once after that long without activity, where connecting, reads and
   * writes are activity once connected; a refused connection emits `error` then `close`; a server drop emits
   * `close`; `destroy()` emits `close` once; writes after that go nowhere. `error` is emitted
   * at once, as from `process.nextTick`, and `close` as a later task, as from the handle's close
   * callback.
   */
  createConnection(options: { host: string; port: number }): unknown {
    const net = this;
    const id = this.nextId++;
    const socket = new (class FakeSocket {
      private listeners: { [event: string]: { fn: Listener; once: boolean }[] } = {};
      private state: "connecting" | "open" | "closed" = "connecting";
      private pending: Uint8Array[] = [];
      private idleMs = 0;
      private idleTimer: any = null;
      private idleFired = false;

      on(event: string, fn: Listener) {
        (this.listeners[event] ??= []).push({ fn, once: false });
        return this;
      }

      once(event: string, fn: Listener) {
        (this.listeners[event] ??= []).push({ fn, once: true });
        return this;
      }

      private emit(event: string, arg?: unknown): void {
        const list = this.listeners[event] ?? [];
        this.listeners[event] = list.filter((l) => !l.once);
        for (const l of list) {
          try {
            l.fn(arg);
          } catch {}
        }
      }

      setTimeout(ms: number, callback?: Listener) {
        if (callback) this.once("timeout", callback);
        this.idleMs = ms;
        this.armIdle();
        return this;
      }

      private armIdle(): void {
        if (this.idleTimer !== null) clearTimeout(this.idleTimer);
        this.idleTimer = null;
        if (this.idleMs <= 0 || this.state === "closed") return;
        this.idleFired = false;
        this.idleTimer = setTimeout(() => {
          this.idleTimer = null;
          this.idleFired = true;
          this.emit("timeout");
        }, this.idleMs);
      }

      private touch(): void {
        if (this.idleMs > 0 && (this.idleTimer !== null || this.idleFired)) this.armIdle();
      }

      write(data: Uint8Array) {
        const bytes = new Uint8Array(data);
        if (this.state === "connecting") this.pending.push(bytes);
        else if (this.state === "open") this.send(bytes);
        return true;
      }

      private send(bytes: Uint8Array): void {
        net.toServer("tcp-send", id, [bytes], (s) => s.tcpSend(id, bytes));
        this.touch();
      }

      private close(): void {
        this.state = "closed";
        if (this.idleTimer !== null) clearTimeout(this.idleTimer);
        this.idleTimer = null;
        this.pending = [];
        net.loop.deliver(0, () => this.emit("close"));
      }

      destroy() {
        if (this.state === "closed") return this;
        net.toServer("tcp-close", id, [], (s) => s.tcpClose(id));
        this.close();
        return this;
      }

      accepted(): void {
        if (this.state !== "connecting") return;
        this.state = "open";
        this.touch();
        for (const bytes of this.pending.splice(0)) this.send(bytes);
      }

      refused(): void {
        if (this.state !== "connecting") return;
        this.emit("error", new Error("connect ECONNREFUSED"));
        this.close();
      }

      received(data: Uint8Array): void {
        if (this.state !== "open") return;
        this.touch();
        this.emit("data", Buffer.from(data));
      }

      dropped(): void {
        if (this.state === "closed") return;
        this.close();
      }
    })();
    this.sockets.set(id, socket);
    this.toServer("tcp-open", id, [[options.host, options.port]], (s) => s.tcpOpen(id, options.host, options.port));
    return socket;
  }
}
