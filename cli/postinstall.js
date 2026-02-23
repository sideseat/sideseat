const { execSync } = require("child_process");
const {
  existsSync,
  mkdirSync,
  copyFileSync,
  writeFileSync,
  renameSync,
  chmodSync,
  rmSync,
} = require("fs");
const path = require("path");
const https = require("https");
const http = require("http");
const zlib = require("zlib");
const crypto = require("crypto");

const platform = process.platform;
const arch = process.arch;
const pkgName = `@sideseat/platform-${platform}-${arch}`;
const pkgDir = `platform-${platform}-${arch}`;
const binName = platform === "win32" ? "sideseat.exe" : "sideseat";
const version = require("./package.json").version;

const MAX_BINARY_SIZE = 200 * 1024 * 1024;
const MAX_TARBALL_SIZE = 250 * 1024 * 1024;
const MAX_TAR_FILES = 20;
const MAX_REDIRECTS = 5;

// Target: cli/vendor/platform-{os}-{arch}/
const vendorDir = path.join(__dirname, "vendor", pkgDir);

const debug = !!process.env.SIDESEAT_DEBUG_INSTALL;

function log(msg) {
  console.error(`[sideseat postinstall] ${msg}`);
}

function isNpm() {
  const ua = process.env.npm_config_user_agent || "";
  return ua.startsWith("npm/");
}

function getRegistryUrl() {
  // Check scoped registry first (@sideseat:registry), then default registry
  if (isNpm()) {
    try {
      const scoped = execSync(
        "npm config get @sideseat:registry",
        { encoding: "utf8", stdio: "pipe", timeout: 5000 }
      ).trim();
      if (scoped && scoped !== "undefined") return scoped.replace(/\/$/, "");
    } catch {}
  }
  const envRegistry = process.env.npm_config_registry;
  if (envRegistry) return envRegistry.replace(/\/$/, "");
  return "https://registry.npmjs.org";
}

// Tier 1: Check if binary already exists
function tier1() {
  try {
    const pkgDir = path.dirname(require.resolve(`${pkgName}/package.json`));
    if (existsSync(path.join(pkgDir, binName))) return true;
  } catch {
    // Not found via require.resolve
  }

  if (existsSync(path.join(vendorDir, binName))) return true;

  return false;
}

// Tier 2: npm install in a temp dir, copy binary to vendor/
// Only runs under npm (not pnpm/yarn) to avoid package manager conflicts
function tier2() {
  if (!isNpm()) return false;

  const tmpDir = path.join(
    require("os").tmpdir(),
    `sideseat-postinstall-${Date.now()}`
  );

  try {
    mkdirSync(tmpDir, { recursive: true });
    writeFileSync(
      path.join(tmpDir, "package.json"),
      JSON.stringify({ private: true })
    );

    // Unset npm_config_global to prevent deadlock when running inside
    // a global install (postinstall → npm install → global → postinstall...)
    const env = { ...process.env, npm_config_global: undefined };

    execSync(
      `npm install --no-save --prefer-online --no-audit --no-fund --ignore-scripts ${pkgName}@${version}`,
      { cwd: tmpDir, stdio: "pipe", timeout: 120000, env }
    );

    const installedPkg = path.join(
      tmpDir,
      "node_modules",
      "@sideseat",
      pkgDir,
      "package.json"
    );

    if (!existsSync(installedPkg)) return false;

    const installedBin = path.join(path.dirname(installedPkg), binName);

    if (!existsSync(installedBin)) return false;

    // Atomic: copy to temp location in vendor, then rename
    mkdirSync(vendorDir, { recursive: true });
    const tmpBin = path.join(vendorDir, `${binName}.tmp`);
    copyFileSync(installedBin, tmpBin);
    renameSync(tmpBin, path.join(vendorDir, binName));

    if (platform !== "win32") {
      try {
        chmodSync(path.join(vendorDir, binName), 0o755);
      } catch {}
    }

    return true;
  } catch (err) {
    if (debug) log(`Tier 2 detail: ${err.message || err}`);
    return false;
  } finally {
    try {
      rmSync(tmpDir, { recursive: true, force: true });
    } catch {}
  }
}

// Tier 3: Fetch packument from registry, download tarball, verify integrity
function tier3() {
  const registryUrl = getRegistryUrl();
  const packumentUrl = `${registryUrl}/${encodeURIComponent(pkgName)}`;

  return fetchJson(packumentUrl)
    .then((packument) => {
      const versionData = packument.versions && packument.versions[version];
      if (!versionData || !versionData.dist) {
        throw new Error(`Version ${version} not found in packument`);
      }

      const tarballUrl = versionData.dist.tarball;
      const integrity = versionData.dist.integrity;

      if (!tarballUrl) {
        throw new Error("No tarball URL in packument");
      }

      return fetchBuffer(tarballUrl).then((gzipped) => {
        // Verify integrity if available (SRI format: algorithm-base64digest)
        if (integrity) {
          verifyIntegrity(gzipped, integrity);
        }

        const tarData = zlib.gunzipSync(gzipped);
        const files = parseTar(tarData);

        const binEntry = files.find(
          (f) => f.name === `package/${binName}` || f.name === binName
        );

        if (!binEntry) {
          throw new Error(`Binary ${binName} not found in tarball`);
        }

        if (binEntry.data.length > MAX_BINARY_SIZE) {
          throw new Error(
            `Binary exceeds max size (${binEntry.data.length} > ${MAX_BINARY_SIZE})`
          );
        }

        // Atomic: write to temp files then rename
        mkdirSync(vendorDir, { recursive: true });

        const tmpBin = path.join(vendorDir, `${binName}.tmp`);
        writeFileSync(tmpBin, binEntry.data, {
          mode: platform !== "win32" ? 0o755 : undefined,
        });
        renameSync(tmpBin, path.join(vendorDir, binName));

        return true;
      });
    })
    .catch((err) => {
      if (debug) log(`Tier 3 detail: ${err.message || err}`);
      return false;
    });
}

function verifyIntegrity(data, sri) {
  const parts = String(sri).trim().split(/\s+/);
  let checked = false;
  for (const p of parts) {
    const m = p.match(/^(sha\d+)-(.+)$/);
    if (!m) continue;
    const [, algo, expected] = m;
    try {
      const actual = crypto.createHash(algo).update(data).digest("base64");
      if (actual === expected) return;
      checked = true;
    } catch {
      // Unsupported algorithm, skip
    }
  }
  if (checked) throw new Error("Integrity check failed");
  // No verifiable hashes found — treat as unverifiable, not as failure
}

function fetchUrl(url, encoding, redirects) {
  if (redirects === undefined) redirects = 0;
  const isHttps = url.startsWith("https:");
  return new Promise((resolve, reject) => {
    const get = isHttps ? https.get : http.get;
    const request = get(url, (res) => {
      const code = res.statusCode;
      if (code === 301 || code === 302 || code === 303 || code === 307 || code === 308) {
        const location = res.headers.location;
        if (!location) return reject(new Error("Redirect without location"));
        if (redirects >= MAX_REDIRECTS)
          return reject(new Error("Too many redirects"));
        res.resume();
        const next = new URL(location, url).toString();
        if (isHttps && next.startsWith("http:"))
          return reject(new Error("HTTPS to HTTP downgrade blocked"));
        return fetchUrl(next, encoding, redirects + 1).then(resolve, reject);
      }
      if (code !== 200) {
        res.resume();
        return reject(new Error(`HTTP ${code} for ${url}`));
      }
      const chunks = [];
      let total = 0;
      res.on("data", (chunk) => {
        total += chunk.length;
        if (total > MAX_TARBALL_SIZE) {
          request.destroy();
          return reject(new Error("Response too large"));
        }
        chunks.push(chunk);
      });
      res.on("error", reject);
      res.on("end", () => {
        const buf = Buffer.concat(chunks);
        resolve(encoding === "json" ? JSON.parse(buf.toString()) : buf);
      });
    });
    request.on("error", reject);
    request.setTimeout(30000, () => {
      request.destroy();
      reject(new Error(`Timeout fetching ${url}`));
    });
  });
}

function fetchJson(url) {
  return fetchUrl(url, "json");
}

function fetchBuffer(url) {
  return fetchUrl(url, "buffer");
}

// Minimal tar parser: 512-byte header blocks followed by data blocks
// Only extracts regular files, ignores symlinks/hardlinks/devices
function parseTar(buf) {
  const files = [];
  let offset = 0;

  while (offset + 512 <= buf.length) {
    const header = buf.subarray(offset, offset + 512);

    // End-of-archive: zero block
    if (header.every((b) => b === 0)) break;

    // File name: bytes 0-99
    let name = "";
    for (let i = 0; i < 100 && header[i] !== 0; i++) {
      name += String.fromCharCode(header[i]);
    }

    // File size: bytes 124-135 (octal ASCII)
    let sizeStr = "";
    for (let i = 124; i < 136 && header[i] !== 0; i++) {
      sizeStr += String.fromCharCode(header[i]);
    }
    const size = parseInt(sizeStr.trim(), 8) || 0;

    // Type flag: byte 156 ('0' or '\0' = regular file)
    const type = header[156];

    // USTAR prefix: bytes 345-499
    let prefix = "";
    if (
      header[257] === 0x75 &&
      header[258] === 0x73 &&
      header[259] === 0x74 &&
      header[260] === 0x61 &&
      header[261] === 0x72
    ) {
      for (let i = 345; i < 500 && header[i] !== 0; i++) {
        prefix += String.fromCharCode(header[i]);
      }
    }

    const fullName = prefix ? `${prefix}/${name}` : name;

    offset += 512;

    if ((type === 48 || type === 0) && size >= 0 && size <= MAX_BINARY_SIZE) {
      const data = Buffer.from(buf.subarray(offset, offset + size));
      files.push({ name: fullName, data });
      if (files.length >= MAX_TAR_FILES) break;
    }

    // Advance past data blocks (rounded up to 512)
    offset += Math.ceil(size / 512) * 512;
  }

  return files;
}

async function main() {
  if (process.env.SIDESEAT_SKIP_INSTALL_CHECK) {
    return;
  }

  // Tier 1: Already installed
  if (tier1()) {
    return;
  }

  log("Platform package not found, attempting fallback install...");

  // Tier 2: npm install in temp dir (npm only, skipped under pnpm/yarn)
  if (tier2()) {
    log("Installed via npm fallback.");
    return;
  }

  log(isNpm() ? "npm fallback failed, trying direct download..." :
    "Skipped npm fallback (not running under npm), trying direct download...");

  // Tier 3: Direct download from registry with integrity verification
  if (await tier3()) {
    log("Installed via direct download.");
    return;
  }

  log(`Error: Could not install platform package ${pkgName}@${version}.`);
  log("Try: npx --yes sideseat@" + version);
  process.exit(1);
}

main().catch((err) => {
  log(`Error: ${err.message || err}`);
  process.exit(1);
});
