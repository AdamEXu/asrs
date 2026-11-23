import * as fakeFs from "../fakes/mainFs";
import * as electronFake from "../fakes/mainElectron";
import { toTree } from "../cve1";

// The application around main.ts's download section: the fake electron of the `files` harness, one
// hub window and the download window opened through the real "Download Logs..." menu item. Every
// call the section makes on its surroundings is appended to `effects` in call order:
//   {k: "send", name, data}        posted on the download window's port (dropped, like Electron
//                                  does, once the window is destroyed)
//   {k: "openFiles", data}         the hub's "open-files" message
//   {k: "open" | "save", token, options}, {k: "box", token, options}
//                                  dialogs; the promise stays pending until `answer(token)`
//   {k: "destroyWindow"}, {k: "focusHub"}

const flush = async (times = 10) => {
  for (let i = 0; i < times; i++) await new Promise((resolve) => setImmediate(resolve));
};

const label = (item: any): string | undefined => (item instanceof electronFake.MenuItem ? item.label : item?.label);
function findItem(items: any[], text: string): any {
  for (const item of items) {
    if (label(item) === text) return item;
    const submenu = item instanceof electronFake.MenuItem ? item.options?.submenu : item?.submenu;
    const list = submenu instanceof electronFake.Menu ? submenu.items : submenu;
    if (Array.isArray(list)) {
      const found = findItem(list, text);
      if (found !== undefined) return found;
    }
  }
  return undefined;
}

export class World {
  effects: any[] = [];
  downloadWindow: any = null;
  private hub: any = null;
  private port: electronFake.FakePort | null = null;
  private dialogs = new Map<number, (value: unknown) => void>();
  private nextDialog = 0;
  private windowIcon: unknown;

  constructor(private bundleUrl: string) {}

  async start(): Promise<void> {
    const bundle = await import(this.bundleUrl);
    this.windowIcon = bundle.Constants.WINDOW_ICON;
    await bundle.load();
    const dialog = electronFake.dialog as any;
    const pending = (k: string) => (window: any, options: any) => {
      if (window !== this.downloadWindow) throw new Error(`${k} dialog without the download window as its parent`);
      const described: any = {};
      for (const key of Object.keys(options)) {
        described[key] = key === "icon" && options.icon === this.windowIcon ? "WINDOW_ICON" : options[key];
      }
      const token = this.nextDialog++;
      this.effects.push({ k, token, options: toTree(described) });
      return new Promise((resolve) => this.dialogs.set(token, resolve));
    };
    dialog.showOpenDialog = pending("open");
    dialog.showSaveDialog = pending("save");
    electronFake.fireReady();
    await flush(20);
    this.hub = electronFake.windowList()[0];
    this.hub.webContents.emit("dom-ready");
    await flush();
    dialog.showMessageBox = pending("box");
    const hub = this.hub;
    const focus = hub.focus.bind(hub);
    hub.focus = () => {
      this.effects.push({ k: "focusHub" });
      focus();
    };
    const hubPort = this.portOf(hub);
    const post = hubPort.postMessage.bind(hubPort);
    hubPort.postMessage = (message: any) => {
      if (message?.name === "open-files") this.effects.push({ k: "openFiles", data: toTree(message.data) });
      post(message);
    };
  }

  private portOf(window: any): electronFake.FakePort {
    const posted = window.webContents.posted.filter((p: any) => p.channel === "port");
    const port1 = posted[posted.length - 1].ports[0];
    return electronFake.channels.find((c) => c.port1 === port1)!.port2;
  }

  private menu(text: string) {
    const item = findItem(electronFake.menuTemplate() ?? [], text);
    if (item === undefined) throw new Error(`no menu item ${text}`);
    const before = new Set(electronFake.windowList());
    (item instanceof electronFake.MenuItem ? item.options.click : item.click).call(item, item, this.hub);
    return electronFake.windowList().find((w) => !before.has(w)) ?? null;
  }

  async openDownload(): Promise<void> {
    const window: any = this.menu("Download Logs...");
    if (window === null) return;
    this.downloadWindow = window;
    const destroy = window.destroy.bind(window);
    const lose = () => Object.defineProperty(window, "id", { get: () => { throw new TypeError("Object has been destroyed"); } });
    window.destroy = () => {
      if (window.destroyed) return;
      this.effects.push({ k: "destroyWindow" });
      destroy();
      lose();
    };
    window.on("closed", lose);
    window.webContents.emit("dom-ready");
    this.port = this.portOf(window);
    const post = this.port.postMessage.bind(this.port);
    this.port.postMessage = (message: any) => {
      this.effects.push({ k: "send", name: message.name, data: toTree(message.data) });
      post(message);
    };
    await flush();
  }

  /** The user closing the download window from its frame. */
  closeDownload(): void {
    this.downloadWindow.close();
  }

  /** The user opening and closing the licenses window, whose `close` also runs downloadStop. */
  async licenses(): Promise<void> {
    const window: any = this.menu("Show Licenses...");
    await flush();
    window.close();
  }

  fromWindow(name: string, data?: unknown): void {
    this.port!.emit("message", { data: { name, data } });
  }

  answer(token: number, value: unknown): void {
    const resolve = this.dialogs.get(token);
    if (resolve === undefined) throw new Error(`no pending dialog ${token}`);
    this.dialogs.delete(token);
    resolve(value);
  }

  pendingDialogs(): number[] {
    return [...this.dialogs.keys()];
  }

  async settle(): Promise<void> {
    await flush();
  }

  take(): any[] {
    return this.effects.splice(0, this.effects.length);
  }
}

export { fakeFs, electronFake };
