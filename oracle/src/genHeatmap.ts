import "./shims";
import { toTree } from "./cve1";
import { fakeContainer, installFakeDom } from "./heatmap/fakeDom";
import { heatmapFixtures } from "./heatmap/fixtures";
import { installRecorder, runUpdates } from "./heatmap/record";
import Heatmap from "@upstream/shared/renderers/Heatmap";

// Emits upstream Heatmap.update ground truth for the asrs_tabs::heatmap vectors:
//   {"fixture":<name>,"update":<i>,"args":<CVE-1 update>,"ok":<CVE-1 record>[,"throw":<CVE-1>]}
// "ok" records {created, setData, store}: whether update built a new h337 instance, the
// argument it passed setData (or null) and getData()'s output afterwards. Runs under a
// fake DOM; canvasMain's "heatmap" mode produces the same records in a real page.

installFakeDom();
const recorder = installRecorder();

for (const fixture of heatmapFixtures()) {
  const container = fakeContainer();
  const heatmap = new Heatmap(container as unknown as HTMLElement);
  const records = runUpdates(heatmap, recorder, fixture.updates);
  records.forEach((record, i) => {
    const update = fixture.updates[i];
    const line: Record<string, unknown> = {
      fixture: fixture.name,
      update: i,
      args: toTree({
        translations: update.translations,
        pixelDimensions: update.pixelDimensions,
        fieldDimensions: update.fieldDimensions
      }),
      ok: toTree({ created: record.created, setData: record.setData, store: record.store })
    };
    if (record.error) line.throw = toTree(record.error);
    process.stdout.write(JSON.stringify(line) + "\n");
  });
}
