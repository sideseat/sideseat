#!/usr/bin/env node
const { spawn } = require('child_process');
const { chmodSync, accessSync, constants } = require('fs');
const path = require('path');

const platform = process.platform;
const arch = process.arch;
const pkgName = `@sideseat/platform-${platform}-${arch}`;
const binName = platform === 'win32' ? 'sideseat.exe' : 'sideseat';

let binaryPath;
try {
  binaryPath = path.join(require.resolve(`${pkgName}/package.json`), '..', binName);
} catch {
  console.error(`Error: Platform package not found: ${pkgName}`);
  console.error(`\nThis usually means your platform (${platform}-${arch}) is not supported,`);
  console.error(`or the platform package failed to install.`);
  console.error(`\nSupported platforms: darwin-arm64, darwin-x64, linux-x64, linux-arm64, win32-x64`);
  console.error(`\nTry reinstalling: npm install sideseat`);
  console.error(`Or install the platform package directly: npm install ${pkgName}`);
  process.exit(1);
}

// Ensure binary is executable (Unix only)
if (platform !== 'win32') {
  try {
    accessSync(binaryPath, constants.X_OK);
  } catch {
    try {
      chmodSync(binaryPath, 0o755);
    } catch (err) {
      console.error(`Warning: Could not make binary executable: ${err.message}`);
    }
  }
}

spawn(binaryPath, process.argv.slice(2), { stdio: 'inherit' })
  .on('exit', (code) => process.exit(code || 0))
  .on('error', (err) => {
    console.error(`Failed to start SideSeat: ${err.message}`);
    process.exit(1);
  });
