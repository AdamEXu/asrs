import { checkJsonNumbers, hexToBytes, MockServer, Outbox, ServerContext, urlQuery } from "./server";

/**
 * RLOG server: a TCP listener (config `port`, default 5800) that only records what the client
 * writes (heartbeats). Steps: `["send", hex]` writes one frame (big-endian int32 length, then
 * the payload) to every open connection, `["sendraw", hex]` writes the bytes as given.
 */
export class RLOGServer extends MockServer {
  private port: number;

  constructor(out: Outbox, ctx: ServerContext, config: any) {
    super(out, ctx);
    this.port = config.port ?? 5800;
    if (typeof this.port !== "number") throw new Error("rlog: port must be a number");
  }

  protected ports(): number[] {
    return [this.port];
  }

  protected serverStep(op: any[]): void {
    let bytes: Uint8Array;
    switch (op[0]) {
      case "send": {
        const payload = hexToBytes(op[1]);
        bytes = new Uint8Array(4 + payload.length);
        new DataView(bytes.buffer).setInt32(0, payload.length);
        bytes.set(payload, 4);
        break;
      }
      case "sendraw":
        bytes = hexToBytes(op[1]);
        break;
      default:
        throw new Error(`rlog: unknown step ${JSON.stringify(op[0])}`);
    }
    for (const id of this.tcp) this.pushTcp(id, bytes);
  }
}

/** Seconds with exactly six decimals, from integer microseconds, without float formatting. */
export function secondsText(us: number): string {
  if (!Number.isSafeInteger(us) || us < 0) throw new Error(`phoenix: bad timestamp ${us}`);
  const frac = String(us % 1000000).padStart(6, "0");
  return `${Math.floor(us / 1000000)}.${frac}`;
}

interface PhoenixDevice {
  device: any;
  signals: any[];
  values: { [signalId: string]: number[] };
}

/**
 * Phoenix 6 diagnostics server on port 1250 (`getdevices`, `getsignals`, `plotpro`).
 *
 * Config: `devices` ([{device, signals, values}]: `device` is the DeviceArray entry as sent,
 * `signals` the getsignals list, `values` maps a signal id to the values its points cycle
 * through), `busUtil` (BusUtilPerc), `startUs` (server time at t = 0, default 0), `points`
 * (points per plot, default 2) and `stepUs` (time between points, default 1000). A plot reply
 * has `points` points ending at the current server time; point k has `Ordinal` equal to its time
 * divided by `stepUs`, and each requested signal the device knows takes
 * `values[id][Ordinal % length]` (0 when none are given), keyed in request order. Steps:
 * `["devices", [...]]` replaces the device list, `["error", action, code]` makes `action`
 * answer with that `GeneralReturn.Error` (0 restores it).
 */
export class PhoenixServer extends MockServer {
  private devices: PhoenixDevice[];
  private busUtil: number;
  private startUs: number;
  private points: number;
  private stepUs: number;
  private errors = new Map<string, number>();

  constructor(out: Outbox, ctx: ServerContext, config: any) {
    super(out, ctx);
    this.devices = this.checkDevices(config.devices ?? []);
    this.busUtil = config.busUtil ?? 0;
    this.startUs = config.startUs ?? 0;
    this.points = config.points ?? 2;
    this.stepUs = config.stepUs ?? 1000;
    checkJsonNumbers(this.busUtil, "phoenix busUtil");
    for (const n of [this.startUs, this.points, this.stepUs]) {
      if (!Number.isSafeInteger(n) || n < 0) throw new Error("phoenix: startUs, points and stepUs must be safe non-negative integers");
    }
    if (this.stepUs === 0) throw new Error("phoenix: stepUs must be positive");
  }

  private checkDevices(devices: PhoenixDevice[]): PhoenixDevice[] {
    if (!Array.isArray(devices)) throw new Error("phoenix: devices must be a list");
    checkJsonNumbers(devices, "phoenix devices");
    const scalar = (v: unknown) => typeof v === "number" || typeof v === "string";
    return devices.map((d) => {
      const plain = (v: unknown) => v !== null && typeof v === "object" && !Array.isArray(v);
      if (!plain(d.device) || !scalar(d.device.ID)) throw new Error("phoenix: device needs an object with a numeric or string ID");
      const signals = d.signals ?? [];
      if (!Array.isArray(signals) || !signals.every((s) => plain(s) && scalar(s.Id))) {
        throw new Error("phoenix: signals must be objects with a numeric or string Id");
      }
      const values = d.values ?? {};
      if (!plain(values) || Object.values(values).some((list) => !Array.isArray(list))) {
        throw new Error("phoenix: values must map ids to arrays");
      }
      return { device: d.device, signals, values };
    });
  }

  protected ports(): number[] {
    return [1250];
  }

  private general(action: string): string {
    return `{"GeneralReturn":{"Error":${this.errors.get(action) ?? 0},"ErrorMessage":""}`;
  }

  private find(query: Map<string, string>): PhoenixDevice | undefined {
    return this.devices.find(
      (d) =>
        d.device.Model === query.get("model") &&
        String(d.device.ID) === query.get("id") &&
        d.device.CANbus === query.get("canbus")
    );
  }

  protected http(url: string): [number, string] | null {
    const query = urlQuery(url);
    const action = query.get("action") ?? "";
    switch (action) {
      case "getdevices":
        return [
          200,
          `${this.general(action)},"BusUtilPerc":${JSON.stringify(this.busUtil)},"DeviceArray":${JSON.stringify(this.devices.map((d) => d.device))}}`
        ];
      case "getsignals": {
        const device = this.find(query);
        if (device === undefined) return [200, `{"GeneralReturn":{"Error":1,"ErrorMessage":"unknown device"}}`];
        return [200, `${this.general(action)},"Signals":${JSON.stringify(device.signals)}}`];
      }
      case "plotpro": {
        const device = this.find(query);
        if (device === undefined) return [200, `{"GeneralReturn":{"Error":1,"ErrorMessage":"unknown device"}}`];
        const ids = (query.get("signals") ?? "").split(",");
        const known = ids.filter((id, i) => ids.indexOf(id) === i && device.signals.some((s) => String(s.Id) === id));
        const nowUs = this.startUs + this.ctx.now() * 1000;
        const points: string[] = [];
        for (let k = this.points - 1; k >= 0; k--) {
          const us = nowUs - k * this.stepUs;
          if (us < 0) continue;
          const ordinal = Math.floor(us / this.stepUs);
          const signals = known.map((id) => {
            const list = Object.hasOwn(device.values, id) ? device.values[id] : [];
            const value = list.length === 0 ? 0 : list[ordinal % list.length];
            return `${JSON.stringify(id)}:${JSON.stringify(value)}`;
          });
          points.push(`{"Ordinal":${ordinal},"Timestamp":${secondsText(us)},"Signals":{${signals.join(",")}}}`);
        }
        return [200, `${this.general(action)},"Count":${points.length},"Points":[${points.join(",")}]}`];
      }
      default:
        return [404, ""];
    }
  }

  protected serverStep(op: any[]): void {
    switch (op[0]) {
      case "devices":
        this.devices = this.checkDevices(op[1]);
        break;
      case "error":
        if (!Number.isSafeInteger(op[2])) throw new Error("phoenix: error code must be an integer");
        this.errors.set(op[1], op[2]);
        break;
      default:
        throw new Error(`phoenix: unknown step ${JSON.stringify(op[0])}`);
    }
  }
}

/**
 * FTC Dashboard server: a websocket on port 8000 that accepts any request. It answers
 * `GET_ROBOT_STATUS` with `{"type":"RECEIVE_ROBOT_STATUS","status":<status>}` and records
 * everything else. Config: `status` (the status object). Steps: `["status", object]`,
 * `["send", json]` (written compactly to every open connection), `["sendtext", text]`.
 */
export class FTCServer extends MockServer {
  private status: unknown;

  constructor(out: Outbox, ctx: ServerContext, config: any) {
    super(out, ctx);
    this.status = config.status ?? {};
    checkJsonNumbers(this.status, "ftc status");
  }

  protected ports(): number[] {
    return [8000];
  }

  protected acceptWs(_url: string, _protocols: string[]): string {
    return "";
  }

  protected wsMessage(id: number, data: string | Uint8Array): void {
    if (typeof data !== "string") return;
    let message: any;
    try {
      message = JSON.parse(data);
    } catch {
      return;
    }
    if (message !== null && typeof message === "object" && message.type === "GET_ROBOT_STATUS") {
      this.pushWs(id, JSON.stringify({ type: "RECEIVE_ROBOT_STATUS", status: this.status }));
    }
  }

  protected serverStep(op: any[]): void {
    let text: string;
    switch (op[0]) {
      case "status":
        if (op[1] === undefined) throw new Error("ftc: status needs a value");
        checkJsonNumbers(op[1], "ftc status");
        this.status = op[1];
        return;
      case "send":
        if (op[1] === undefined) throw new Error("ftc: send needs a value");
        checkJsonNumbers(op[1], "ftc send");
        text = JSON.stringify(op[1]);
        break;
      case "sendtext":
        if (typeof op[1] !== "string") throw new Error("ftc: sendtext needs a string");
        text = op[1];
        break;
      default:
        throw new Error(`ftc: unknown step ${JSON.stringify(op[0])}`);
    }
    for (const id of this.ws.keys()) this.pushWs(id, text);
  }
}
