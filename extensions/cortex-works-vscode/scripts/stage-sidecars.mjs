import { chmodSync, copyFileSync, existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const extensionRoot = path.resolve(__dirname, '..');
const repoRoot = path.resolve(extensionRoot, '..', '..');

const SUPPORTED_PLATFORMS = {
  'darwin-arm64': { rustTargets: ['aarch64-apple-darwin'] },
  'darwin-x64': { rustTargets: ['x86_64-apple-darwin'] },
  'linux-arm64': { rustTargets: ['aarch64-unknown-linux-gnu', 'aarch64-unknown-linux-musl'] },
  'linux-x64': { rustTargets: ['x86_64-unknown-linux-gnu', 'x86_64-unknown-linux-musl'] },
  'win32-x64': { rustTargets: ['x86_64-pc-windows-gnullvm', 'x86_64-pc-windows-gnu'] },
};

const BINARIES = ['cortex-extension-bridge', 'cortex-mcp'];

function hostPlatformKey() {
  return `${process.platform}-${process.arch}`;
}

function binaryName(name, platformKey) {
  return platformKey.startsWith('win32-') ? `${name}.exe` : name;
}

function envKeyForPlatform(platformKey) {
  return `CORTEX_STAGE_SOURCE_${platformKey.toUpperCase().replace(/[^A-Z0-9]/g, '_')}`;
}

function requestedPlatforms() {
  if (process.argv.includes('--all')) {
    return Object.keys(SUPPORTED_PLATFORMS);
  }

  const fromEnv = process.env.CORTEX_VSCODE_PLATFORMS
    ?.split(',')
    .map((value) => value.trim())
    .filter(Boolean);

  return fromEnv?.length ? fromEnv : [hostPlatformKey()];
}

function isBestEffortAllPlatforms() {
  return process.argv.includes('--all');
}

function candidateSourceDirs(platformKey) {
  const platform = SUPPORTED_PLATFORMS[platformKey];
  const dirs = [];

  const override = process.env[envKeyForPlatform(platformKey)];
  if (override) {
    dirs.push(override);
  }

  if (platformKey === hostPlatformKey()) {
    dirs.push(path.join(repoRoot, 'target', 'release'));
  }

  for (const rustTarget of platform?.rustTargets ?? []) {
    dirs.push(path.join(repoRoot, 'target', rustTarget, 'release'));
  }

  return dirs;
}

function findSourceDir(platformKey) {
  return candidateSourceDirs(platformKey).find((dir) =>
    BINARIES.every((binary) => existsSync(path.join(dir, binaryName(binary, platformKey)))),
  );
}

for (const platformKey of requestedPlatforms()) {
  const sourceDir = findSourceDir(platformKey);
  if (!sourceDir) {
    if (isBestEffortAllPlatforms()) {
      console.warn(
        `skipped ${platformKey}: build the Rust targets first or set ${envKeyForPlatform(platformKey)}`,
      );
      continue;
    }

    throw new Error(
      `Missing built binaries for ${platformKey}. Build the Rust targets first or set ${envKeyForPlatform(platformKey)}.`,
    );
  }

  const targetDir = path.join(extensionRoot, 'resources', 'sidecars', platformKey);
  mkdirSync(targetDir, { recursive: true });

  for (const existing of readdirSync(targetDir)) {
    rmSync(path.join(targetDir, existing), { force: true, recursive: true });
  }

  for (const logicalName of BINARIES) {
    const filename = binaryName(logicalName, platformKey);
    const source = path.join(sourceDir, filename);
    const destination = path.join(targetDir, filename);

    copyFileSync(source, destination);
    if (!platformKey.startsWith('win32-')) {
      chmodSync(destination, 0o755);
    }
    console.log(`staged ${logicalName} (${platformKey}) -> ${destination}`);
  }
}