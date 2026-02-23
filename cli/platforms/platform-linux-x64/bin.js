#!/usr/bin/env node
const { spawn } = require("child_process");
const { accessSync, chmodSync, constants } = require("fs");
const path = require("path");

const binName = process.platform === "win32" ? "sideseat.exe" : "sideseat";
const bin = path.join(__dirname, binName);

// Ensure binary is executable (Unix only)
if (process.platform !== "win32") {
  try {
    accessSync(bin, constants.X_OK);
  } catch {
    try {
      chmodSync(bin, 0o755);
    } catch {}
  }
}

spawn(bin, process.argv.slice(2), { stdio: "inherit" })
  .on("exit", (code) => process.exit(code || 0))
  .on("error", (err) => {
    console.error(`Failed to start SideSeat: ${err.message}`);
    process.exit(1);
  });
