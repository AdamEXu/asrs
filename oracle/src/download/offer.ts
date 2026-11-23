import net from "node:net";
import { upstreamRequire } from "./bundle";

// What ssh2 offers a server: its ident line and the name-lists of its KEXINIT, recorded once by a
// server that only reads, with the options main.ts connects with.

const LISTS = ["kex", "hostKey", "cipherCs", "cipherSc", "macCs", "macSc", "compressionCs", "compressionSc", "languageCs", "languageSc"];

export async function runOffer(): Promise<void> {
  const { Client } = upstreamRequire()("ssh2");
  const record = await new Promise<object>((resolve, reject) => {
    const server = net.createServer((socket) => {
      let received = Buffer.alloc(0);
      socket.on("error", () => undefined);
      socket.write("SSH-2.0-fixture\r\n");
      socket.on("data", (data) => {
        received = Buffer.concat([received, data]);
        const ident = received.indexOf("\r\n") + 2;
        if (ident < 2 || received.length < ident + 4 || received.length < ident + 4 + received.readUInt32BE(ident)) return;
        let p = ident + 4 + 1 + 1 + 16;
        if (received[ident + 5] !== 20) reject(new Error("not a KEXINIT"));
        const offer: any = { ident: received.subarray(0, ident - 2).toString("latin1") };
        for (const list of LISTS) {
          const length = received.readUInt32BE(p);
          offer[list] = received.subarray(p + 4, p + 4 + length).toString("latin1");
          p += 4 + length;
        }
        offer.firstFollows = received[p];
        resolve(offer);
      });
    });
    server.listen(0, "127.0.0.1", () => {
      new Client()
        .on("error", () => undefined)
        .connect({
          host: "127.0.0.1",
          port: (server.address() as net.AddressInfo).port,
          forceIPv4: true,
          readyTimeout: 3000,
          username: "systemcore",
          password: "systemcore"
        });
    });
  });
  process.stdout.write(JSON.stringify(record) + "\n");
}
