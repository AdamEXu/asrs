// Runs in the page's main world before any page script (a session preload hands it to
// contextBridge.executeInMainWorld), so it must stay self-contained: it is serialized with
// Function.prototype.toString and may not reference anything outside its own body.
export function installTap(readGl: boolean) {
  type Entry = {
    canvas: HTMLCanvasElement;
    kind: string;
    context: any;
    drew: boolean;
    frame: number;
    pixels: Uint8Array | null;
    width: number;
    height: number;
  };
  const entries: Entry[] = [];
  let frame = 0;
  let glReads = 0;
  let saves = 0;
  let savedState: string | null = null;

  // The hub's HubState as main receives it: the data of the last "save-state" message the page
  // posted on its message port.
  const postMessage = MessagePort.prototype.postMessage;
  MessagePort.prototype.postMessage = function (this: MessagePort, message: any, ...rest: any[]) {
    if (message !== null && typeof message === "object" && message.name === "save-state") {
      savedState = JSON.stringify(message.data);
      saves++;
    }
    return (postMessage as any).call(this, message, ...rest);
  } as any;

  const DRAW_CALLS = ["clear", "drawArrays", "drawElements", "drawArraysInstanced", "drawElementsInstanced", "drawRangeElements", "blitFramebuffer"];

  const register = (canvas: HTMLCanvasElement, kind: string, context: any) => {
    const entry: Entry = { canvas, kind, context, drew: false, frame: -1, pixels: null, width: 0, height: 0 };
    entries.push(entry);
    if (kind === "webgl" || kind === "webgl2") {
      for (const name of DRAW_CALLS) {
        const original = context[name];
        if (typeof original !== "function") continue;
        context[name] = function (this: any, ...args: any[]) {
          entry.drew = true;
          return original.apply(this, args);
        };
      }
    }
  };

  const getContext = HTMLCanvasElement.prototype.getContext;
  HTMLCanvasElement.prototype.getContext = function (this: HTMLCanvasElement, kind: string, ...rest: any[]) {
    const context = (getContext as any).call(this, kind, ...rest);
    if (context !== null && !entries.some((e) => e.context === context)) {
      register(this, kind === "experimental-webgl" ? "webgl" : kind, context);
    }
    return context;
  } as any;

  const readBack = (entry: Entry) => {
    const gl = entry.context as WebGL2RenderingContext;
    const width = gl.drawingBufferWidth;
    const height = gl.drawingBufferHeight;
    const framebuffer = gl.getParameter(gl.FRAMEBUFFER_BINDING);
    const isWebgl2 = entry.kind === "webgl2";
    const readFramebuffer = isWebgl2 ? gl.getParameter(gl.READ_FRAMEBUFFER_BINDING) : null;
    const packBuffer = isWebgl2 ? gl.getParameter(gl.PIXEL_PACK_BUFFER_BINDING) : null;
    const alignment = gl.getParameter(gl.PACK_ALIGNMENT);
    if (isWebgl2) {
      gl.bindFramebuffer(gl.READ_FRAMEBUFFER, null);
      gl.bindBuffer(gl.PIXEL_PACK_BUFFER, null);
    } else {
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    }
    gl.pixelStorei(gl.PACK_ALIGNMENT, 4);
    const raw = new Uint8Array(width * height * 4);
    gl.readPixels(0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, raw);
    gl.pixelStorei(gl.PACK_ALIGNMENT, alignment);
    if (isWebgl2) {
      gl.bindFramebuffer(gl.READ_FRAMEBUFFER, readFramebuffer);
      gl.bindBuffer(gl.PIXEL_PACK_BUFFER, packBuffer);
    } else {
      gl.bindFramebuffer(gl.FRAMEBUFFER, framebuffer);
    }
    const row = width * 4;
    const pixels = new Uint8Array(raw.length);
    for (let y = 0; y < height; y++) pixels.set(raw.subarray((height - 1 - y) * row, (height - y) * row), y * row);
    entry.pixels = pixels;
    entry.width = width;
    entry.height = height;
    entry.frame = frame;
    glReads++;
  };

  // The drawing buffer is only defined until the compositor takes the frame, so it is read in
  // the same task as the draw: after each animation frame callback that drew into it.
  const requestAnimationFrame = window.requestAnimationFrame;
  window.requestAnimationFrame = function (callback: FrameRequestCallback) {
    return requestAnimationFrame.call(window, (time: number) => {
      for (const entry of entries) entry.drew = false;
      try {
        callback(time);
      } finally {
        frame++;
        if (readGl) for (const entry of entries) if (entry.drew) readBack(entry);
      }
    });
  };

  const describe = (canvas: HTMLCanvasElement) => {
    const rect = canvas.getBoundingClientRect();
    const path: string[] = [];
    for (let el: Element | null = canvas; el !== null && el !== document.documentElement; el = el.parentElement) {
      let part = el.tagName.toLowerCase();
      if (el.id) part += "#" + el.id;
      for (const name of Array.from(el.classList)) part += "." + name;
      path.unshift(part);
    }
    return {
      path: path.join(" > "),
      rect: [rect.x, rect.y, rect.width, rect.height],
      width: canvas.width,
      height: canvas.height,
      connected: canvas.isConnected,
      visible: canvas.isConnected && rect.width > 0 && rect.height > 0 && canvas.getClientRects().length > 0
    };
  };

  (window as any).__asrsParity = {
    frame: () => frame,
    glReads: () => glReads,
    saves: () => saves,
    savedState: () => savedState,
    contexts: () => entries.map((e, index) => ({ index, kind: e.kind, ...describe(e.canvas) })),
    canvasPixels(index: number) {
      const entry = entries[index];
      const { width, height } = entry.canvas;
      if (width === 0 || height === 0) return { width, height, rgba: new Uint8Array(0) };
      const data = (entry.context as CanvasRenderingContext2D).getImageData(0, 0, width, height);
      return { width, height, colorSpace: data.colorSpace, rgba: new Uint8Array(data.data.buffer) };
    },
    glPixels(index: number) {
      const entry = entries[index];
      const gl = entry.context as WebGL2RenderingContext;
      return {
        width: entry.width,
        height: entry.height,
        frame: entry.frame,
        rgba: entry.pixels,
        attributes: gl.getContextAttributes(),
        renderer: (() => {
          const ext = gl.getExtension("WEBGL_debug_renderer_info");
          return ext ? gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER);
        })()
      };
    }
  };
}
