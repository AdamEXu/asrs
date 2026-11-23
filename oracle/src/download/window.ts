import vm from "node:vm";
import { buildWindow } from "./bundle";

// download.ts in a document that only records what the P04 part of it produces: the "start" message
// its set-preferences handler sends (the robot address it connects to) and the alert text it shows
// for a show-error or show-alert message. Records: {preferences, platform, start} and
// {preferences, platform, message, name, hidden, text}.

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

const ADDRESSES = [
  "10.63.28.2", "10.00.028.02", "010.1.2.3", "0.0.0.0", "00.000.0000.0", "172.22.11.2", "roborio-6328-frc.local", "robot.local", "localhost",
  "", ".", "..", "0", "000", "0x1.2", "10.0.0", "1.02.3.4.5", "00abc.0.def0", " 10.1.2.3 ", "fe80::1", "a.00b", "é.0", "10.<b>.1", "0robot"
];
const PATHS = ["/U/logs", "/home/lvuser/logs/", "", "/", "/a b/<c>&\"d'", "/ü–"];
const MESSAGES = [
  "No such file", "No such file ", "no such file", "Timed out while waiting for handshake", "Fetch failed", "All configured authentication methods failed",
  "Not connected", "Error while looking up IPv4 address for 'robot.local': Error: getaddrinfo ENOTFOUND robot.local", "ENOTFOUND", "xENOTFOUNDx", "enotfound",
  "Error while looking up IPv4 address for 'robot.local': Error: getaddrinfo EAI_AGAIN robot.local", "Socket is closed", "Connection lost before handshake",
  "connect ECONNREFUSED 10.63.28.2:22", "connect EHOSTUNREACH 10.63.28.2:22", "read ECONNRESET", "Permission denied", "Failure", "Too many authentication failures",
  "Unable to start subsystem: sftp", "No response from server", "Handshake failed: no matching key exchange algorithm", "ENOENT: no such file or directory, open '/x'", "", "<b>&amp;</b>", "Unknown"
];

class Element {
  hidden = true;
  innerHTML = "";
  innerText = "";
  value = 0;
  style: any = {};
  classList = { add() {}, remove() {} };
  children: Element[] = [];
  childElementCount = 0;
  firstChild: Element | null = null;
  addEventListener() {}
  removeChild() {}
  getBoundingClientRect() {
    return { height: 0 };
  }
}

export async function runWindow(): Promise<void> {
  const code = await buildWindow();
  for (const platform of ["darwin", "win32", "linux"]) {
    for (const fixed of ["", "usb", "wifi"]) {
      for (const robotAddress of fixed === "" ? ADDRESSES : ["10.63.28.2"]) {
        for (const remotePath of robotAddress === ADDRESSES[0] ? PATHS : [PATHS[0]]) {
          const preferences = { systemcoreStaticAddress: fixed, robotAddress, remotePath };
          const alert = new Element();
          const listeners: { [name: string]: (event: any) => void } = {};
          const list = new Element();
          list.children.push(new Element());
          const button = new Element();
          button.children.push(new Element(), new Element());
          const byClass: { [name: string]: Element } = { "file-list": list, "alert-text": alert };
          const document = {
            getElementsByClassName: (name: string) => [byClass[name] ?? new Element()],
            getElementsByTagName: () => [new Element()],
            getElementById: () => button,
            createElement: () => new Element()
          };
          const sent: any[] = [];
          const port: any = { postMessage: (message: any) => sent.push(message) };
          const window = { addEventListener: (name: string, callback: any) => (listeners[name] = callback) };
          vm.runInNewContext(code, { window, document, console: { warn() {} } });
          listeners.message({ data: "port", ports: [port] });
          const main = (name: string, data: unknown) => port.onmessage({ data: { name, data } });
          main("set-platform", platform);
          main("set-preferences", preferences);
          out({ preferences, platform, start: sent.splice(0) });
          const interesting = robotAddress === ADDRESSES[0] || fixed !== "" || robotAddress === "roborio-6328-frc.local" || robotAddress === "10.<b>.1";
          if (platform !== "darwin" && fixed === "") continue;
          if (!interesting) continue;
          for (const name of ["show-error", "show-alert"]) {
            for (const message of MESSAGES) {
              alert.hidden = true;
              alert.innerHTML = "unset";
              main(name, message);
              out({ preferences, platform, message, name, hidden: alert.hidden, text: alert.innerHTML });
            }
          }
        }
      }
    }
  }
}
