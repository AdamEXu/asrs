import Field3dRendererImpl from "@upstream/shared/renderers/Field3dRendererImpl";
import { Field3dRendererCommand } from "@upstream/shared/renderers/Field3dRenderer";

declare const SCENE: { robotPath: string; cameras: Record<string, { position: number[]; target: number[] }>; pose: { translation: [number, number, number]; rotation: [number, number, number, number] } };

const w = window as any;
w.platform = "darwin";
w.isBattery = false;
w.sendMainMessage = () => {};
w.preferences = { field3dModeAc: "standard", field3dModeBattery: "", field3dAntialiasing: true };
w.assets = {
  field2ds: [],
  field3ds: [],
  robots: [
    {
      name: "KitBot",
      path: SCENE.robotPath,
      isFTC: false,
      disableSimplification: false,
      rotations: [{ axis: "z", degrees: 90 }],
      position: [0.12, 3.17, 0],
      cameras: [],
      components: []
    }
  ],
  joysticks: [],
  loadFailures: []
};

const container = document.getElementById("container")!;
const canvas = document.getElementById("canvas") as HTMLCanvasElement;
const alert = document.getElementById("alert")!;
const spinner = document.getElementById("spinner")!;

const impl = new Field3dRendererImpl("standard", true, canvas, container, alert, spinner);
const cam = SCENE.cameras[new URLSearchParams(location.search).get("cam") ?? "field"];
impl.restoreState({ cameraIndex: -1, orbitFov: 50, cameraPosition: cam.position, cameraTarget: cam.target });

const command: Field3dRendererCommand = {
  field: "FRC:Evergreen",
  isRedAlliance: false,
  coordinateSystem: "center-red",
  objects: [
    {
      type: "robot",
      model: "KitBot",
      color: "#000000",
      poses: [{ pose: SCENE.pose, annotation: { is2DSource: false } }],
      components: [],
      mechanisms: { xz: null, yz: null },
      visionTargets: [],
      swerveModuleVelocities: []
    } as any
  ],
  cameraOverride: null,
  autoDriverStation: -1,
  allRobotModels: ["KitBot"]
};

let frames = 0;
function loop() {
  impl.render(command);
  frames++;
  const loading = (impl as any).robotLoadingCount > 0;
  w.s3Ready = !loading && frames > 30;
  requestAnimationFrame(loop);
}
requestAnimationFrame(loop);
