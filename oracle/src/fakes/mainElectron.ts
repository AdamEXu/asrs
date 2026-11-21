// The `electron` replacement main-process modules see in the `files` harness. Windows, displays,
// dialogs, menus, ports and `app` are all fake and deterministic; everything the scenario
// generator needs is reachable from `fakes/mainElectron`'s exported state.

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

const noop = () => undefined;

const allPorts: FakePort[] = [];

export function takeSent(): any[] {
  const out: any[] = [];
  allPorts.forEach((p) => out.push(...p.sent.splice(0, p.sent.length)));
  return out;
}

export class FakePort {
  handlers = new Map<string, ((event: any) => void)[]>();
  sent: any[] = [];

  constructor() {
    allPorts.push(this);
  }

  on(name: string, cb: (event: any) => void): this {
    const list = this.handlers.get(name) ?? [];
    list.push(cb);
    this.handlers.set(name, list);
    return this;
  }

  once(name: string, cb: (event: any) => void): this {
    return this.on(name, cb);
  }

  emit(name: string, event: any): void {
    (this.handlers.get(name) ?? []).forEach((cb) => cb(event));
  }

  postMessage(message: unknown): void {
    this.sent.push(message);
  }

  start(): void {}
  close(): void {}
}

export const channels: MessageChannelMain[] = [];

export class MessageChannelMain {
  port1 = new FakePort();
  port2 = new FakePort();

  constructor() {
    channels.push(this);
  }
}

export class FakeWebContents {
  handlers = new Map<string, ((...args: any[]) => void)[]>();
  posted: any[] = [];

  on(name: string, cb: (...args: any[]) => void): this {
    const list = this.handlers.get(name) ?? [];
    list.push(cb);
    this.handlers.set(name, list);
    return this;
  }

  once(name: string, cb: (...args: any[]) => void): this {
    return this.on(name, cb);
  }

  emit(name: string, ...args: any[]): void {
    (this.handlers.get(name) ?? []).forEach((cb) => cb(...args));
  }

  postMessage(channel: string, message: unknown, ports?: FakePort[]): void {
    this.posted.push({ channel, message, ports });
  }

  send(): void {}
  setWindowOpenHandler(): void {}
  openDevTools(): void {}
  setUserAgent(): void {}
  setZoomFactor(): void {}
}

let nextWindowId = 1;
const windows = new Map<number, FakeBrowserWindow>();
export const created: { id: number; bounds: Rect }[] = [];
export const closedIds: number[] = [];

export function takeCreated(): { id: number; bounds: Rect }[] {
  return created.splice(0, created.length);
}

export function takeClosed(): number[] {
  return closedIds.splice(0, closedIds.length);
}

export class FakeBrowserWindow {
  readonly id: number;
  readonly options: any;
  readonly webContents = new FakeWebContents();
  bounds: Rect;
  destroyed = false;
  closed = false;
  handlers = new Map<string, ((...args: any[]) => void)[]>();

  constructor(options: any) {
    this.id = nextWindowId++;
    this.options = options;
    this.bounds = {
      x: options?.x ?? 0,
      y: options?.y ?? 0,
      width: options?.width ?? 0,
      height: options?.height ?? 0
    };
    const proxy = new Proxy(this, {
      get(target, prop: string) {
        const value = (target as any)[prop];
        if (value !== undefined) return value;
        const stub = () => undefined;
        (target as any)[prop] = stub;
        return stub;
      }
    });
    windows.set(this.id, proxy);
    created.push({ id: this.id, bounds: { ...this.bounds } });
    return proxy;
  }

  static fromId(id: number): FakeBrowserWindow | null {
    return windows.get(id) ?? null;
  }

  static getAllWindows(): FakeBrowserWindow[] {
    return [...windows.values()];
  }

  static getFocusedWindow(): FakeBrowserWindow | null {
    return focused;
  }

  getBounds(): Rect {
    return { ...this.bounds };
  }

  getPosition(): [number, number] {
    return [this.bounds.x, this.bounds.y];
  }

  getSize(): [number, number] {
    return [this.bounds.width, this.bounds.height];
  }

  destroy(): void {
    this.closed = true;
    this.destroyed = true;
    windows.delete(this.id);
    if (focused === this) focused = null;
  }

  setBounds(bounds: Partial<Rect>): void {
    this.bounds = { ...this.bounds, ...bounds };
  }

  on(name: string, cb: (...args: any[]) => void): this {
    const list = this.handlers.get(name) ?? [];
    list.push(cb);
    this.handlers.set(name, list);
    return this;
  }

  once(name: string, cb: (...args: any[]) => void): this {
    const wrapper = (...args: any[]) => {
      const list = this.handlers.get(name) ?? [];
      this.handlers.set(
        name,
        list.filter((f) => f !== wrapper)
      );
      cb(...args);
    };
    return this.on(name, wrapper);
  }

  emit(name: string, ...args: any[]): void {
    (this.handlers.get(name) ?? []).forEach((cb) => cb(...args));
  }

  focus(): void {
    focused = this;
    this.emit("focus");
  }

  isDestroyed(): boolean {
    return this.destroyed;
  }

  isFullScreen(): boolean {
    return false;
  }

  isFocused(): boolean {
    return focused === this;
  }

  close(): void {
    if (this.destroyed) return;
    let prevented = false;
    this.emit("close", { preventDefault: () => (prevented = true) });
    if (prevented) return;
    this.closed = true;
    this.destroyed = true;
    windows.delete(this.id);
    closedIds.push(this.id);
    if (focused === this) focused = null;
    this.emit("closed");
  }

  show(): void {}
  hide(): void {}
  loadFile(): void {}
  loadURL(): void {}
}

const BrowserWindowStatics = {
  fromId: FakeBrowserWindow.fromId,
  getAllWindows: FakeBrowserWindow.getAllWindows,
  getFocusedWindow: FakeBrowserWindow.getFocusedWindow
};
export const BrowserWindow = Object.assign(FakeBrowserWindow, BrowserWindowStatics);

let focused: FakeBrowserWindow | null = null;

export function windowList(): FakeBrowserWindow[] {
  return [...windows.values()];
}

export function setWindowBounds(id: number, bounds: Rect | null): void {
  if (bounds === null) {
    windows.delete(id);
    return;
  }
  const window = windows.get(id);
  if (window !== undefined) {
    window.bounds = { ...bounds };
    return;
  }
  windows.set(id, {
    id,
    bounds: { ...bounds },
    destroyed: false,
    getBounds() {
      return { ...this.bounds };
    },
    isDestroyed() {
      return false;
    },
    close() {
      windows.delete(this.id);
    }
  } as any);
}

let displaysList: Rect[] = [{ x: 0, y: 0, width: 1920, height: 1080 }];

export function setDisplays(rects: Rect[]): void {
  displaysList = rects.map((r) => ({ ...r }));
}

export const screen = {
  getAllDisplays(): { bounds: Rect }[] {
    return displaysList.map((bounds) => ({ bounds: { ...bounds } }));
  },
  getPrimaryDisplay(): { bounds: Rect } {
    return displaysList.length === 0 ? (undefined as any) : { bounds: { ...displaysList[0] } };
  },
  on: noop,
  once: noop,
  removeListener: noop
};

const paths: Record<string, string> = {
  userData: "/fake/userData",
  temp: "/fake/temp",
  home: "/fake/home",
  documents: "/fake/documents",
  downloads: "/fake/downloads",
  appData: "/fake/appData",
  cache: "/fake/cache"
};

let readyResolve: (() => void) | null = null;
const readyPromise = new Promise<void>((resolve) => (readyResolve = resolve));
let readyFlag = false;

export function fireReady(): void {
  readyFlag = true;
  readyResolve?.();
  queueMicrotask(() => fireAppEvent("ready"));
}

const appEvents = new Map<string, ((...args: any[]) => void)[]>();

export const app = {
  isPackaged: false,
  runningUnderARM64Translation: false,
  commandLine: { appendSwitch: noop },
  getName: () => "AdvantageScope",
  setName: noop,
  getVersion: () => "27.0.0-alpha-6",
  getPath: (name: string): string => paths[name] ?? `/fake/${name}`,
  getAppPath: () => "/fake/app",
  getLocale: () => "en-US",
  isReady: () => readyFlag,
  whenReady: () => readyPromise,
  on(name: string, cb: (...args: any[]) => void): void {
    const list = appEvents.get(name) ?? [];
    list.push(cb);
    appEvents.set(name, list);
  },
  once(name: string, cb: (...args: any[]) => void): void {
    const wrapper = (...args: any[]) => {
      appEvents.set(
        name,
        (appEvents.get(name) ?? []).filter((f) => f !== wrapper)
      );
      cb(...args);
    };
    app.on(name, wrapper);
  },
  quit: noop,
  exit: noop,
  relaunch: noop,
  addRecentDocument: noop,
  clearRecentDocuments: noop,
  setAppUserModelId: noop,
  requestSingleInstanceLock: () => true,
  disableHardwareAcceleration: noop,
  setAboutPanelOptions: noop,
  getLoginItemSettings: () => ({ openAtLogin: false }),
  setLoginItemSettings: noop,
  releaseSingleInstanceLock: noop,
  isDefaultProtocolClient: () => false,
  setAsDefaultProtocolClient: () => true
};

export function fireAppEvent(name: string, ...args: any[]): void {
  (appEvents.get(name) ?? []).forEach((cb) => cb(...args));
}

export function setPackaged(value: boolean): void {
  app.isPackaged = value;
}

export class Menu {
  items: any[] = [];

  append(item: any): void {
    this.items.push(item);
  }

  insert(pos: number, item: any): void {
    this.items.splice(pos, 0, item);
  }

  popup(options?: any): void {
    popped.push({ menu: this, window: options?.window });
  }

  closePopup(): void {}

  static buildFromTemplate(template: any[]): Menu {
    const menu = new Menu();
    menu.items = template;
    return menu;
  }

  static setApplicationMenu(menu: Menu | null): void {
    applicationMenu = menu;
  }

  static getApplicationMenu(): Menu | null {
    return applicationMenu;
  }

  static sendActionToFirstResponder(): void {}
}

export class MenuItem {
  options: any;

  constructor(options: any) {
    this.options = options;
  }

  get label(): string {
    return this.options?.label;
  }

  get click(): ((...args: any[]) => void) | undefined {
    return this.options?.click;
  }

  get submenu(): any[] | Menu | undefined {
    return this.options?.submenu;
  }
}

export const popped: { menu: Menu; window: any }[] = [];
let applicationMenu: Menu | null = null;

export function menuTemplate(): any[] | null {
  return applicationMenu?.items ?? null;
}

export function menuItemAt(items: any[], path: number[]): any {
  let current: any = items;
  for (let i = 0; i < path.length; i++) {
    const item = current[path[i]];
    if (i === path.length - 1) return item;
    const submenu = item instanceof MenuItem ? item.options?.submenu : item?.submenu;
    current = submenu instanceof Menu ? submenu.items : submenu;
  }
  return current;
}

const dialogQueue: { save: any[]; open: any[]; box: any[]; boxSync: any[] } = {
  save: [],
  open: [],
  box: [],
  boxSync: []
};

export function queueDialog(kind: keyof typeof dialogQueue, value: any): void {
  dialogQueue[kind].push(value);
}

export const dialog = {
  showSaveDialog: async (): Promise<any> =>
    dialogQueue.save.shift() ?? { canceled: true, filePath: undefined },
  showOpenDialog: async (): Promise<any> =>
    dialogQueue.open.shift() ?? { canceled: true, filePaths: [] },
  showMessageBox: async (): Promise<any> => dialogQueue.box.shift() ?? { response: 0, checkboxChecked: false },
  showMessageBoxSync: (): number => dialogQueue.boxSync.shift() ?? 0,
  showCertificateTrustDialog: noop
};

export const shell = {
  openExternal: (url: string): Promise<void> => {
    externalUrls.push(url);
    return Promise.resolve();
  },
  showItemInFolder: noop,
  openPath: async (): Promise<string> => "",
  trashItem: async (): Promise<void> => {},
  beep: noop,
  writeShortcutLink: () => false,
  readShortcutLink: () => ({ target: "", cwd: "", args: "" })
};

export const externalUrls: string[] = [];

export const nativeTheme = {
  themeSource: "system" as string,
  shouldUseDarkColors: false,
  addListener: noop,
  on: noop,
  once: noop,
  removeListener: noop
};

export const powerMonitor = {
  on: noop,
  once: noop,
  isOnBatteryPower: () => false,
  getSystemIdleTime: () => 0
};

export const nativeImage = {
  createFromPath: (path: string) => ({ __fakeNativeImage: path }),
  createFromDataURL: (url: string) => ({ __fakeNativeImage: url }),
  createFromBuffer: () => ({}),
  createEmpty: () => ({})
};

export const clipboard = {
  writeText: noop,
  readText: () => "",
  writeBuffer: noop,
  readBuffer: () => Buffer.alloc(0),
  clear: noop
};

export const net = {
  isOnline: () => true,
  fetch: (...args: any[]) => fetch(...(args as [any]))
};

export const ipcMain = {
  on: noop,
  once: noop,
  handle: noop,
  handleOnce: noop,
  removeHandler: noop,
  removeListener: noop
};

export const globalShortcut = {
  register: () => true,
  unregister: noop,
  unregisterAll: noop,
  isRegistered: () => false
};

export const safeStorage = {
  isEncryptionAvailable: () => false,
  encryptString: () => Buffer.alloc(0),
  decryptString: () => ""
};

export const systemPreferences = {
  getColor: () => "#000000",
  getAccentColor: () => "00000000",
  isDarkMode: () => false,
  subscribeNotification: noop,
  on: noop
};

export const crashReporter = { start: noop };

export const contentTracing = { stopRecording: async (): Promise<string> => "" };

export const session = {
  defaultSession: {
    webRequest: { onBeforeSendHeaders: noop, onHeadersReceived: noop },
    setPermissionRequestHandler: noop,
    setPermissionCheckHandler: noop,
    clearCache: async (): Promise<void> => {},
    clearStorageData: async (): Promise<void> => {}
  },
  fromPartition: () => session.defaultSession
};

export const protocol = {
  registerSchemesAsPrivileged: noop,
  registerFileProtocol: noop,
  handle: noop
};

export const desktopCapturer = {
  getSources: async (): Promise<any[]> => []
};

export const webContents = {
  getAllWebContents: () => [] as FakeWebContents[]
};

const touchBarComponent = class {
  options: any;
  constructor(options?: any) {
    this.options = options;
  }
};

export class TouchBar {
  options: any;
  static TouchBarButton = touchBarComponent;
  static TouchBarColorPicker = touchBarComponent;
  static TouchBarGroup = touchBarComponent;
  static TouchBarLabel = touchBarComponent;
  static TouchBarOtherItemsProxy = touchBarComponent;
  static TouchBarPopover = touchBarComponent;
  static TouchBarScrubber = touchBarComponent;
  static TouchBarSegmentedControl = touchBarComponent;
  static TouchBarSlider = touchBarComponent;
  static TouchBarSpacer = touchBarComponent;
  constructor(options?: any) {
    this.options = options;
  }
}

export type TouchBarSlider = InstanceType<typeof touchBarComponent>;
export type TitleBarOverlay = { color?: string; symbolColor?: string; height?: number };
export type MessagePortMain = FakePort;
export type FileFilter = { name: string; extensions: string[] };
export type BrowserWindowConstructorOptions = any;
