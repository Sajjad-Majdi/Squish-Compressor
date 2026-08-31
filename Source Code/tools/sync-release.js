'use strict';

const fs = require('fs');
const path = require('path');

const root = path.join(__dirname, '..');
const projectRoot = path.join(root, '..');
const pkg = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
const tauri = JSON.parse(fs.readFileSync(path.join(root, 'src-tauri', 'tauri.conf.json'), 'utf8'));
const cargo = fs.readFileSync(path.join(root, 'src-tauri', 'Cargo.toml'), 'utf8');
const cargoVersion = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1];

if (!cargoVersion || pkg.version !== tauri.version || pkg.version !== cargoVersion) {
  throw new Error(
    `Version mismatch: package=${pkg.version}, tauri=${tauri.version}, cargo=${cargoVersion || 'missing'}`,
  );
}

const executableDir = path.join(projectRoot, 'Executable');
const installerDir = path.join(projectRoot, 'Installer');
fs.mkdirSync(executableDir, { recursive: true });
fs.mkdirSync(installerDir, { recursive: true });

const releaseDir = process.env.CARGO_TARGET_DIR
  ? path.join(process.env.CARGO_TARGET_DIR, 'release')
  : path.join(root, 'src-tauri', 'target', 'release');
const installerName = `Squish_${pkg.version}_x64-setup.exe`;
const copies = [
  [path.join(releaseDir, 'squish.exe'), path.join(executableDir, 'squish.exe')],
  [path.join(releaseDir, 'ffmpeg.exe'), path.join(executableDir, 'ffmpeg.exe')],
  [path.join(releaseDir, 'WebView2Loader.dll'), path.join(executableDir, 'WebView2Loader.dll')],
  [path.join(releaseDir, 'bundle', 'nsis', installerName), path.join(installerDir, installerName)],
];

for (const [source] of copies) {
  if (!fs.existsSync(source)) throw new Error(`Missing release artifact: ${source}`);
}

for (const name of fs.readdirSync(installerDir)) {
  if (/^Squish_.*_x64-setup\.exe$/i.test(name) && name !== installerName) {
    fs.rmSync(path.join(installerDir, name), { force: true });
  }
}

for (const [source, destination] of copies) fs.copyFileSync(source, destination);
console.log(`Synced Squish ${pkg.version} to Installer and Executable.`);
