#!/usr/bin/env node
const { spawn } = require('child_process');
const { existsSync, accessSync, chmodSync, constants } = require('fs');
const path = require('path');

const platform = process.platform;
const arch = process.arch;
const pkgName = `@sideseat/platform-${platform}-${arch}`;
const binName = platform === 'win32' ? 'sideseat.exe' : 'sideseat';

// Locate the native binary: node_modules package → vendor fallback
let binaryPath;
try {
  const pkgDir = path.dirname(require.resolve(`${pkgName}/package.json`));
  binaryPath = path.join(pkgDir, binName);
} catch {
  const vendorPath = path.join(__dirname, 'vendor', `platform-${platform}-${arch}`, binName);
  if (existsSync(vendorPath)) {
    binaryPath = vendorPath;
  } else {
    const version = require('./package.json').version;
    console.error(`Error: Platform package not found: ${pkgName}`);
    console.error(`\nThis usually means your platform (${platform}-${arch}) is not supported,`);
    console.error(`or the platform package failed to install.`);
    console.error(`\nSupported platforms: darwin-arm64, darwin-x64, linux-x64, linux-arm64, win32-x64`);
    console.error(`\nTry: npx --yes sideseat@${version}`);
    console.error(`  or: npx --yes ${pkgName}@${version}`);
    console.error(`  or: npm install -g sideseat`);
    process.exit(1);
  }
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
