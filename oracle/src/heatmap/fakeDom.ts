// A minimal DOM so the real h337 can run under ELECTRON_RUN_AS_NODE: canvases and
// contexts with the API surface heatmap.js touches, returning empty ImageData so the
// store and renderer logic still run. Only the genHeatmap entry uses it; canvasMain
// runs the same code in a real page.

class FakeContext {
  fillStyle: unknown;
  globalAlpha = 1;

  createLinearGradient() {
    return { addColorStop() {} };
  }
  createRadialGradient() {
    return { addColorStop() {} };
  }
  fillRect() {}
  beginPath() {}
  arc() {}
  fill() {}
  clearRect() {}
  drawImage() {}
  getImageData(_x: number, _y: number, w: number, h: number) {
    const width = Math.trunc(w);
    const height = Math.trunc(h);
    if (width === 0) {
      throw new DOMException(
        "Failed to execute 'getImageData' on 'CanvasRenderingContext2D': The source width is 0.",
        "IndexSizeError"
      );
    }
    if (height === 0) {
      throw new DOMException(
        "Failed to execute 'getImageData' on 'CanvasRenderingContext2D': The source height is 0.",
        "IndexSizeError"
      );
    }
    const size = Math.max(0, width) * Math.max(0, height) * 4;
    return { data: new Uint8ClampedArray(size), width: Math.abs(width), height: Math.abs(height) };
  }
  putImageData() {}
}

class FakeElement {
  readonly tag: string;
  style: Record<string, string> = { width: "300px", height: "150px" };
  className = "";
  private children: FakeElement[] = [];
  private context?: FakeContext;
  private bitmapWidth = 300;
  private bitmapHeight = 150;

  constructor(tag: string) {
    this.tag = tag;
  }

  get width() {
    return this.bitmapWidth;
  }
  set width(value: number) {
    this.bitmapWidth = value >>> 0;
  }
  get height() {
    return this.bitmapHeight;
  }
  set height(value: number) {
    this.bitmapHeight = value >>> 0;
  }

  getContext() {
    return (this.context ??= new FakeContext());
  }
  appendChild(child: FakeElement) {
    this.children.push(child);
    return child;
  }
  removeChild(child: FakeElement) {
    this.children = this.children.filter((c) => c !== child);
    return child;
  }
  get firstChild() {
    return this.children[0] ?? null;
  }
  getElementsByTagName(tag: string) {
    return this.children.filter((c) => c.tag === tag);
  }
}

export function installFakeDom() {
  (globalThis as Record<string, unknown>).document = {
    createElement: (tag: string) => new FakeElement(tag)
  };
  (globalThis as Record<string, unknown>).getComputedStyle = (element: FakeElement) => element.style;
}

export function fakeContainer(): FakeElement {
  return new FakeElement("div");
}
