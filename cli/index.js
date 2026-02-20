#!/usr/bin/env node
const { spawn } = require('child_process');
const fs = require('fs');
const path = require('path');

const platform = process.platform;
const arch = process.arch;
const pkgName = `@sideseat/platform-${platform}-${arch}`;
const binName = platform === 'win32' ? 'sideseat.exe' : 'sideseat';

function resolveBinFromPkgJson(pkgJsonPath) {
  try {
    const pkg = JSON.parse(fs.readFileSync(pkgJsonPath, 'utf8'));
    const bin = pkg.bin;
    if (!bin) return null;
    const rel = typeof bin === 'string' ? bin : (bin.sideseat || bin[Object.keys(bin)[0]]);
    if (!rel) return null;
    return path.resolve(path.dirname(pkgJsonPath), rel);
  } catch {
    return null;
  }
}

let binaryPath;
try {
  const pkgJsonPath = require.resolve(`${pkgName}/package.json`);
  binaryPath = resolveBinFromPkgJson(pkgJsonPath) ||
    path.join(path.dirname(pkgJsonPath), binName);
} catch {
  // Fallback: check if postinstall placed the binary in vendor/
  const vendorPkg = path.join(__dirname, 'vendor', `platform-${platform}-${arch}`, 'package.json');
  const vendorBin = fs.existsSync(vendorPkg) && resolveBinFromPkgJson(vendorPkg);
  const vendorDirect = path.join(__dirname, 'vendor', `platform-${platform}-${arch}`, binName);
  if (vendorBin && fs.existsSync(vendorBin)) {
    binaryPath = vendorBin;
  } else if (fs.existsSync(vendorDirect)) {
    binaryPath = vendorDirect;
  } else {
    console.error(`Error: Platform package not found: ${pkgName}`);
    console.error(`\nThis usually means your platform (${platform}-${arch}) is not supported,`);
    console.error(`or the platform package failed to install.`);
    console.error(`\nSupported platforms: darwin-arm64, darwin-x64, linux-x64, linux-arm64, win32-x64`);
    const version = require('./package.json').version;
    console.error(`\nIf using npx, try specifying the version directly:`);
    console.error(`  npx --yes sideseat@${version}`);
    console.error(`\nOr run the platform package directly:`);
    console.error(`  npx --yes ${pkgName}@${version}`);
    console.error(`\nOr install globally: npm install -g sideseat`);
    try {
      const npmCache = require('child_process')
        .execSync('npm config get cache', { encoding: 'utf8', stdio: 'pipe' }).trim();
      const npxCache = path.join(npmCache, '_npx');
      console.error(`\nIf npx keeps reusing a broken install, delete its cache:`);
      if (platform === 'win32') {
        console.error(`  rmdir /s /q "${npxCache}"`);
      } else {
        console.error(`  rm -rf "${npxCache}"`);
      }
    } catch {}
    process.exit(1);
  }
}

// Ensure binary is executable (Unix only)
if (platform !== 'win32') {
  try {
    fs.accessSync(binaryPath, fs.constants.X_OK);
  } catch {
    try {
      fs.chmodSync(binaryPath, 0o755);
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
