'use strict';

const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');

const root = path.join(__dirname, '..');
const tauriCli = path.join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js');
const targetDir = path.join(os.tmpdir(), 'squish-release-target');
process.env.CARGO_TARGET_DIR = targetDir;

const result = spawnSync(process.execPath, [tauriCli, 'build'], {
  cwd: root,
  env: process.env,
  stdio: 'inherit',
});

if (result.status !== 0) process.exit(result.status ?? 1);
require('./sync-release');
