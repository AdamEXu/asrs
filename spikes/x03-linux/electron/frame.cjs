const path = require("path");
const fs = require("fs");
const { app } = require("electron");

const upstream = process.argv[process.argv.length - 1];
const userData = "/tmp/x03-frame-userdata";
fs.rmSync(userData, { recursive: true, force: true });
fs.mkdirSync(userData, { recursive: true });
app.setPath("userData", userData);
require(path.join(upstream, "bundles", "main.js"));
