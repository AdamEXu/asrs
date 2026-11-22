import { app, Menu } from "electron";
import fs from "node:fs";
import { toTree } from "./cve1";

// Electron main process for `genMenus electron` (not run under ELECTRON_RUN_AS_NODE). Builds the
// application menu from upstream's template with Electron's own Menu.buildFromTemplate and
// records every item as Electron fills it in, including the role defaults Electron supplies for
// labels, accelerators and submenus.

const request = JSON.parse(fs.readFileSync(process.argv[process.argv.length - 1], "utf8"));
app.setName(request.appName);
app.setPath("userData", request.userData);

const revive = (value: any): any => {
  if (Array.isArray(value)) return value.map(revive);
  if (value !== null && typeof value === "object") {
    if (typeof value[request.click] === "string") {
      const name = value[request.click];
      return { [name]() {} }[name];
    }
    const copy: any = {};
    for (const key of Object.keys(value)) copy[key] = revive(value[key]);
    return copy;
  }
  return value;
};

const FIELDS = [
  "label",
  "role",
  "type",
  "accelerator",
  "checked",
  "enabled",
  "visible",
  "sublabel",
  "toolTip",
  "registerAccelerator",
  "acceleratorWorksWhenHidden"
];

function items(menu: Electron.Menu): unknown[] {
  return menu.items.map((item: any) => {
    const record: any = {};
    for (const field of FIELDS) record[field] = item[field];
    record.defaultRoleAccelerator =
      typeof item.getDefaultRoleAccelerator === "function" ? item.getDefaultRoleAccelerator() : undefined;
    if (item.submenu) record.submenu = items(item.submenu);
    return record;
  });
}

app.whenReady().then(() => {
  const menu = Menu.buildFromTemplate(revive(request.template));
  Menu.setApplicationMenu(menu);
  const top = Menu.getApplicationMenu();
  if (top === null) {
    process.stderr.write("genMenusMain: no application menu\n");
    app.exit(1);
    return;
  }
  process.stdout.write(JSON.stringify({ op: "electron", appName: app.getName(), menu: toTree(items(top)) }) + "\n", () => app.exit(0));
});
