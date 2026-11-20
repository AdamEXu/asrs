// Transcribed from src/main/electron/main.ts at the pinned commit (the live RLOG variables, the
// `live-rlog-start` and `live-rlog-stop` cases of `handleHubMessage`, and the heartbeat
// interval) because main.ts cannot be loaded outside Electron's main process. The statements
// are upstream's, with `net` and `sendMessage` injected and one hub window (id 1).

const RLOG_CONNECT_TIMEOUT_MS = 3000;
const RLOG_DATA_TIMEOUT_MS = 3000;
const RLOG_HEARTBEAT_DELAY_MS = 500;
const RLOG_HEARTBEAT_DATA = new Uint8Array([6, 3, 2, 8]);

export interface NetModule {
  createConnection(options: { host: string; port: number }): any;
}

export class RLOGMain {
  private rlogSockets: { [id: number]: any } = {};
  private rlogSocketTimeouts: { [id: number]: any } = {};
  private rlogDataArrays: { [id: number]: Uint8Array } = {};

  constructor(
    private net: NetModule,
    private sendMessage: (window: number, name: string, data?: any) => boolean
  ) {
    setInterval(() => {
      Object.values(this.rlogSockets).forEach((socket) => {
        socket.write(RLOG_HEARTBEAT_DATA);
      });
    }, RLOG_HEARTBEAT_DELAY_MS);
  }

  /** Returns false for messages this emulation does not handle. */
  handle(name: string, data: any): boolean {
    const window = 1;
    const windowId = 1;
    const message = { name, data };
    const { rlogSockets, rlogSocketTimeouts, rlogDataArrays, net } = this;
    const sendMessage = this.sendMessage;
    switch (message.name) {
      case "live-rlog-start":
        rlogSockets[windowId]?.destroy();
        rlogSockets[windowId] = net.createConnection({
          host: message.data.address,
          port: message.data.port
        });

        rlogSockets[windowId].setTimeout(RLOG_CONNECT_TIMEOUT_MS, () => {
          sendMessage(window, "live-data", { uuid: message.data.uuid, status: false });
        });

        let appendArray = (newArray: Uint8Array) => {
          let fullArray = new Uint8Array(rlogDataArrays[windowId].length + newArray.length);
          fullArray.set(rlogDataArrays[windowId]);
          fullArray.set(newArray, rlogDataArrays[windowId].length);
          rlogDataArrays[windowId] = fullArray;
        };

        rlogDataArrays[windowId] = new Uint8Array();
        rlogSockets[windowId].on("data", (data: Uint8Array) => {
          appendArray(data);
          if (rlogSocketTimeouts[windowId] !== null) clearTimeout(rlogSocketTimeouts[windowId]);
          rlogSocketTimeouts[windowId] = setTimeout(() => {
            rlogSockets[windowId]?.destroy();
          }, RLOG_DATA_TIMEOUT_MS);

          while (true) {
            let expectedLength;
            if (rlogDataArrays[windowId].length < 4) {
              break;
            } else {
              expectedLength = new DataView(rlogDataArrays[windowId].buffer).getInt32(0) + 4;
              if (rlogDataArrays[windowId].length < expectedLength) {
                break;
              }
            }

            let singleArray = rlogDataArrays[windowId].slice(4, expectedLength);
            rlogDataArrays[windowId] = rlogDataArrays[windowId].slice(expectedLength);

            if (singleArray.length > 0) {
              let success = sendMessage(window, "live-data", {
                uuid: message.data.uuid,
                success: true,
                raw: new Uint8Array(singleArray)
              });
              if (!success) {
                rlogSockets[windowId]?.destroy();
              }
            }
          }
        });

        rlogSockets[windowId].on("error", () => {
          sendMessage(window, "live-data", { uuid: message.data.uuid, success: false });
        });

        rlogSockets[windowId].on("close", () => {
          sendMessage(window, "live-data", { uuid: message.data.uuid, success: false });
        });
        return true;

      case "live-rlog-stop":
        rlogSockets[windowId]?.destroy();
        return true;

      default:
        return false;
    }
  }
}
