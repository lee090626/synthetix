#!/usr/bin/env node
// Runs the native axiom binary from the platform package npm installed.
const { spawnSync } = require('node:child_process');

const platform = `${process.platform}-${process.arch}`;
const exe = process.platform === 'win32' ? 'axiom.exe' : 'axiom';
// Platform package names come from optionalDependencies (win32 is scoped).
const { optionalDependencies } = require('../package.json');
const name = Object.keys(optionalDependencies).find((n) => n.endsWith(`synthetix-cli-${platform}`));
let binary;
try {
  binary = require.resolve(`${name}/bin/${exe}`);
} catch {
  console.error(`synthetix-cli: no binary for ${platform}; supported: darwin-arm64, darwin-x64, linux-x64, win32-x64`);
  process.exit(1);
}
const result = spawnSync(binary, process.argv.slice(2), { stdio: 'inherit' });
if (result.error) {
  console.error(`synthetix-cli: ${result.error.message}`);
  process.exit(1);
}
process.exit(result.status ?? 1);
