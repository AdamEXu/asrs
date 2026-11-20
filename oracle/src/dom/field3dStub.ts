// Stands in for upstream's Field3dRendererImpl, which needs WebGL. The DOM harness records the
// 3D command the controller produces; 3D pixels belong to the Electron capture (H09).
export default class Field3dRendererImpl {
  private state: unknown = null;

  constructor(
    readonly mode: string,
    readonly useAA: boolean,
    readonly canvas: HTMLCanvasElement,
    readonly canvasContainer: HTMLElement,
    readonly alert: HTMLElement,
    readonly spinner: HTMLElement
  ) {}

  saveState(): unknown {
    return this.state;
  }

  restoreState(state: unknown) {
    this.state = state;
  }

  set3DCamera(_index: number) {}

  setFov(_fov: number) {}

  stop() {}

  getAspectRatio(): number | null {
    return null;
  }

  render(_command: unknown) {}
}
