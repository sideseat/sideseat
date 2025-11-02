#!/usr/bin/env node
const { spawn } = require('child_process');
const path = require('path');
const fs = require('fs');

// Detect platform and architecture
const platform = process.platform; // darwin, linux, win32
const arch = process.arch; // x64, arm64

// Construct binary name
const binaryName = platform === 'win32' ? `sideseat-${platform}-${arch}.exe` : `sideseat-${platform}-${arch}`;

const binaryPath = path.join(__dirname, 'bin', binaryName);

// Check if binary exists
if (!fs.existsSync(binaryPath)) {
  console.error(`Error: Binary not found for ${platform}-${arch}`);
  console.error(`Expected: ${binaryPath}`);
  console.error(`Supported platforms: darwin-arm64, darwin-x64, linux-x64, linux-arm64, win32-x64`);
  process.exit(1);
}

// Make binary executable on Unix
if (platform !== 'win32') {
  try {
    fs.chmodSync(binaryPath, 0o755);
  } catch (err) {
    console.error(`Warning: Could not make binary executable: ${err.message}`);
  }
}

// Launch binary with all arguments
const child = spawn(binaryPath, process.argv.slice(2), {
  stdio: 'inherit',
  shell: false,
});

child.on('exit', (code) => {
  process.exit(code || 0);
});

child.on('error', (err) => {
  console.error(`Failed to start SideSeat: ${err.message}`);
  process.exit(1);
});
