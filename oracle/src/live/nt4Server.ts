import { decodeAll, encode, number, Packed } from "./msgpack";
import { checkJsonNumbers, hexToBytes, MockServer, Outbox, ServerContext } from "./server";

const V41 = "v4.1.networktables.first.wpi.edu";
const V40 = "networktables.first.wpi.edu";
const RTT = "rtt.networktables.first.wpi.edu";

const TYPE_INDEX: { [type: string]: number } = {
  boolean: 0,
  double: 1,
  int: 2,
  float: 3,
  string: 4,
  json: 4,
  raw: 5,
  rpc: 5,
  msgpack: 5,
  protobuf: 5,
  "boolean[]": 16,
  "double[]": 17,
  "int[]": 18,
  "float[]": 19,
  "string[]": 20
};

interface Topic {
  id: number;
  name: string;
  type: string;
  properties: { [key: string]: unknown };
  value: { ts: number; packed: Packed } | null;
}

interface Subscription {
  topics: string[];
  prefix: boolean;
  topicsOnly: boolean;
}

interface Connection {
  rtt: boolean;
  subs: Map<number, Subscription>;
  announced: Set<string>;
}

function isObject(value: unknown): value is { [key: string]: any } {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function packValue(type: string, value: any, where: string): Packed {
  const check = (ok: boolean) => {
    if (!ok) throw new Error(`${where}: ${JSON.stringify(value)} is not a ${type} value`);
  };
  const list = (item: (v: any) => Packed) => {
    check(Array.isArray(value));
    return { arr: (value as any[]).map(item) };
  };
  switch (type) {
    case "boolean":
      check(typeof value === "boolean");
      return { bool: value };
    case "double":
      check(typeof value === "number");
      return { f64: value };
    case "float":
      check(typeof value === "number");
      return { f32: value };
    case "int":
      check(Number.isSafeInteger(value));
      return { int: value };
    case "string":
    case "json":
      check(typeof value === "string");
      return { str: value };
    case "boolean[]":
      return list((v) => packValue("boolean", v, where));
    case "double[]":
      return list((v) => packValue("double", v, where));
    case "float[]":
      return list((v) => packValue("float", v, where));
    case "int[]":
      return list((v) => packValue("int", v, where));
    case "string[]":
      return list((v) => packValue("string", v, where));
    default:
      check(typeof value === "string");
      return { bin: hexToBytes(value) };
  }
}

/**
 * NT4 server: the HTTP alive check and the websocket protocol for 4.1 (with its RTT socket) or
 * 4.0. Topics are announced to every connection whose subscriptions match them; values go to
 * matching subscriptions that are not topics-only, and a new subscription gets the current
 * value of each topic it matches. `periodic` and `all` are not modelled: every update is sent
 * once, when it happens. Client values and unpublish are only recorded.
 *
 * Config: `version` ("4.1" | "4.0"), `ports` ([5810]), `offsetUs` (server time is
 * `(epochMs + t) * 1000 + offsetUs`), `topics` ([{name, type, properties?, value?}], announced
 * before the session starts). Steps: `["announce", name, type, properties?]`,
 * `["unannounce", name]`, `["properties", name, update]`, `["set", name, value, tsUs?]`,
 * `["batch", [[name, value, tsUs?], ...]]` (one frame), `["setraw", name, hex, tsUs?]` (value
 * bytes as given), `["text", json]` and `["binary", hex]` (sent verbatim to every main
 * connection).
 */
export class NT4Server extends MockServer {
  private version: string;
  private serverPorts: number[];
  private offsetUs: number;
  private topics: Topic[] = [];
  private nextTopicId = 1;
  private conns = new Map<number, Connection>();

  constructor(out: Outbox, ctx: ServerContext, config: any) {
    super(out, ctx);
    this.version = config.version ?? "4.1";
    if (this.version !== "4.1" && this.version !== "4.0") throw new Error(`nt4: unknown version ${this.version}`);
    this.serverPorts = config.ports ?? [5810];
    if (!Array.isArray(this.serverPorts) || !this.serverPorts.every((p) => typeof p === "number")) {
      throw new Error("nt4: ports must be a list of numbers");
    }
    this.offsetUs = config.offsetUs ?? 0;
    if (!Number.isSafeInteger(this.offsetUs)) throw new Error("nt4: offsetUs must be a safe integer");
    for (const t of config.topics ?? []) {
      const topic = this.addTopic(t.name, t.type, t.properties ?? {});
      if (t.value !== undefined) topic.value = { ts: this.serverTimeUs(), packed: packValue(t.type, t.value, t.name) };
    }
  }

  private serverTimeUs(): number {
    return (this.ctx.epochMs + this.ctx.now()) * 1000 + this.offsetUs;
  }

  protected ports(): number[] {
    return this.serverPorts;
  }

  protected http(_url: string): [number, string] {
    return [200, ""];
  }

  protected acceptWs(_url: string, protocols: string[]): string | null {
    if (this.version === "4.1") {
      for (const p of [V41, V40, RTT]) if (protocols.includes(p)) return p;
    } else if (protocols.includes(V40)) {
      return V40;
    }
    return null;
  }

  protected wsOpened(id: number): void {
    this.conns.set(id, { rtt: this.ws.get(id) === RTT, subs: new Map(), announced: new Set() });
  }

  private addTopic(name: string, type: string, properties: { [key: string]: unknown }): Topic {
    if (typeof name !== "string" || typeof type !== "string") throw new Error("nt4: topic needs a name and a type");
    if (this.topic(name) !== undefined) throw new Error(`nt4: topic ${name} exists`);
    if (!isObject(properties)) throw new Error(`nt4: topic ${name} properties must be an object`);
    checkJsonNumbers(properties, `nt4 topic ${name} properties`);
    const topic: Topic = { id: this.nextTopicId++, name, type, properties: { ...properties }, value: null };
    this.topics.push(topic);
    return topic;
  }

  private topic(name: string): Topic | undefined {
    return this.topics.find((t) => t.name === name);
  }

  private mainConns(): [number, Connection][] {
    return [...this.conns.entries()].filter(([id, c]) => !c.rtt && this.ws.has(id));
  }

  private matches(sub: Subscription, name: string): boolean {
    return sub.topics.some((p) => (sub.prefix ? name.startsWith(p) : name === p));
  }

  private wantsTopic(conn: Connection, name: string): boolean {
    return [...conn.subs.values()].some((s) => this.matches(s, name));
  }

  private wantsValue(conn: Connection, name: string): boolean {
    return [...conn.subs.values()].some((s) => !s.topicsOnly && this.matches(s, name));
  }

  private announceMessage(topic: Topic): unknown {
    return { method: "announce", params: { name: topic.name, id: topic.id, type: topic.type, properties: topic.properties } };
  }

  private sendJson(id: number, messages: unknown[]): void {
    if (messages.length > 0) this.pushWs(id, JSON.stringify(messages));
  }

  private valueArray(topic: Topic, ts: number, packed: Packed): Packed {
    const index = Object.hasOwn(TYPE_INDEX, topic.type) ? TYPE_INDEX[topic.type] : 5;
    return { arr: [{ int: topic.id }, number(ts), { int: index }, packed] };
  }

  private announceToAll(topic: Topic): void {
    for (const [id, conn] of this.mainConns()) {
      if (!this.wantsTopic(conn, topic.name)) continue;
      conn.announced.add(topic.name);
      this.sendJson(id, [this.announceMessage(topic)]);
    }
  }

  private sendValues(updates: [Topic, number, Packed][]): void {
    for (const [id, conn] of this.mainConns()) {
      const arrays = updates.filter(([t]) => this.wantsValue(conn, t.name)).map(([t, ts, p]) => this.valueArray(t, ts, p));
      if (arrays.length > 0) this.pushWs(id, encode(arrays));
    }
  }

  private updateProperties(topic: Topic, update: { [key: string]: unknown }): void {
    checkJsonNumbers(update, `nt4 topic ${topic.name} properties`);
    for (const key of Object.keys(update)) {
      if (update[key] === null) delete topic.properties[key];
      else topic.properties[key] = update[key];
    }
    for (const [id, conn] of this.mainConns()) {
      if (conn.announced.has(topic.name)) {
        this.sendJson(id, [{ method: "properties", params: { name: topic.name, update } }]);
      }
    }
  }

  protected wsMessage(id: number, data: string | Uint8Array): void {
    const conn = this.conns.get(id)!;
    if (typeof data !== "string") {
      for (const item of decodeAll(data)) {
        if (!Array.isArray(item) || item.length < 4 || item[0] !== -1 || typeof item[3] !== "number") continue;
        this.pushWs(id, encode([{ arr: [{ int: -1 }, number(this.serverTimeUs()), { int: 2 }, number(item[3])] }]));
      }
      return;
    }
    if (conn.rtt) return;
    let messages: any;
    try {
      messages = JSON.parse(data);
    } catch {
      return;
    }
    if (!Array.isArray(messages)) return;
    for (const message of messages) {
      if (!isObject(message)) continue;
      const params = message.params;
      if (!isObject(params)) continue;
      switch (message.method) {
        case "subscribe":
          this.subscribe(id, conn, params);
          break;
        case "unsubscribe":
          conn.subs.delete(params.subuid);
          break;
        case "publish":
          if (typeof params.name === "string" && typeof params.type === "string" && this.topic(params.name) === undefined) {
            const properties = isObject(params.properties) ? params.properties : {};
            this.announceToAll(this.addTopic(params.name, params.type, properties));
          }
          break;
        case "setproperties": {
          const topic = typeof params.name === "string" ? this.topic(params.name) : undefined;
          if (topic !== undefined && isObject(params.update)) {
            this.updateProperties(topic, params.update);
          }
          break;
        }
      }
    }
  }

  private subscribe(id: number, conn: Connection, params: any): void {
    if (typeof params.subuid !== "number" || !Array.isArray(params.topics)) return;
    const options = isObject(params.options) ? params.options : {};
    const sub: Subscription = {
      topics: params.topics.filter((t: unknown) => typeof t === "string"),
      prefix: options.prefix === true,
      topicsOnly: options.topicsonly === true
    };
    conn.subs.set(params.subuid, sub);
    const matched = this.topics.filter((t) => this.matches(sub, t.name));
    const fresh = matched.filter((t) => !conn.announced.has(t.name));
    fresh.forEach((t) => conn.announced.add(t.name));
    this.sendJson(
      id,
      fresh.map((t) => this.announceMessage(t))
    );
    if (!sub.topicsOnly) {
      const arrays = matched.filter((t) => t.value !== null).map((t) => this.valueArray(t, t.value!.ts, t.value!.packed));
      if (arrays.length > 0) this.pushWs(id, encode(arrays));
    }
  }

  private known(name: unknown): Topic {
    const topic = typeof name === "string" ? this.topic(name) : undefined;
    if (topic === undefined) throw new Error(`nt4: unknown topic ${JSON.stringify(name)}`);
    return topic;
  }

  private update(name: unknown, value: unknown, tsUs: unknown, raw: boolean): [Topic, number, Packed] {
    const topic = this.known(name);
    if (tsUs !== undefined && !Number.isSafeInteger(tsUs)) throw new Error("nt4: tsUs must be a safe integer");
    const ts = (tsUs as number | undefined) ?? this.serverTimeUs();
    const packed: Packed = raw ? { raw: hexToBytes(value as string) } : packValue(topic.type, value, topic.name);
    topic.value = { ts, packed };
    return [topic, ts, packed];
  }

  protected serverStep(op: any[]): void {
    switch (op[0]) {
      case "announce":
        this.announceToAll(this.addTopic(op[1], op[2], op[3] ?? {}));
        break;
      case "unannounce": {
        const topic = this.known(op[1]);
        this.topics.splice(this.topics.indexOf(topic), 1);
        for (const [id, conn] of this.mainConns()) {
          if (!conn.announced.delete(topic.name)) continue;
          this.sendJson(id, [{ method: "unannounce", params: { name: topic.name, id: topic.id } }]);
        }
        break;
      }
      case "properties":
        if (!isObject(op[2])) throw new Error("nt4: properties needs an update object");
        this.updateProperties(this.known(op[1]), op[2]);
        break;
      case "set":
        this.sendValues([this.update(op[1], op[2], op[3], false)]);
        break;
      case "setraw":
        this.sendValues([this.update(op[1], op[2], op[3], true)]);
        break;
      case "batch":
        this.sendValues((op[1] as any[][]).map(([name, value, ts]) => this.update(name, value, ts, false)));
        break;
      case "text":
        if (op[1] === undefined) throw new Error("nt4: text needs a value");
        checkJsonNumbers(op[1], "nt4 text");
        for (const [id] of this.mainConns()) this.pushWs(id, JSON.stringify(op[1]));
        break;
      case "binary":
        for (const [id] of this.mainConns()) this.pushWs(id, hexToBytes(op[1]));
        break;
      default:
        throw new Error(`nt4: unknown step ${JSON.stringify(op[0])}`);
    }
  }
}
