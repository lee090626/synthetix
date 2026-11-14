// Builds npm packages from release binaries: one per platform plus the
// synthetix-cli launcher. Usage: node scripts/npm-packages.cjs <binaries> <out> <version>
const fs = require('node:fs');
const path = require('node:path');

const [binaries, out, version] = process.argv.slice(2);
if (!binaries || !out || !/^\d+\.\d+\.\d+/.test(version ?? '')) {
  console.error('usage: node scripts/npm-packages.cjs <binaries> <out> <version>');
  process.exit(1);
}
const platforms = {
  'darwin-arm64': ['darwin', 'arm64'],
  'darwin-x64': ['darwin', 'x64'],
  'linux-x64': ['linux', 'x64'],
  'win32-x64': ['win32', 'x64'],
};
const root = path.join(__dirname, '..', 'npm', 'synthetix-cli');
const launcher = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));

for (const [platform, [os, cpu]] of Object.entries(platforms)) {
  const exe = os === 'win32' ? 'axiom.exe' : 'axiom';
  const source = path.join(binaries, `axiom-${platform}${os === 'win32' ? '.exe' : ''}`);
  const dir = path.join(out, `synthetix-cli-${platform}`);
  fs.mkdirSync(path.join(dir, 'bin'), { recursive: true });
  fs.copyFileSync(source, path.join(dir, 'bin', exe));
  fs.chmodSync(path.join(dir, 'bin', exe), 0o755);
  fs.writeFileSync(path.join(dir, 'package.json'), JSON.stringify({
    name: Object.keys(launcher.optionalDependencies).find((n) => n.endsWith(`synthetix-cli-${platform}`)),
    version,
    description: `Synthetix CLI binary for ${platform}.`,
    license: launcher.license,
    repository: launcher.repository,
    os: [os],
    cpu: [cpu],
    files: [`bin/${exe}`],
  }, null, 2) + '\n');
}

const dir = path.join(out, 'synthetix-cli');
fs.mkdirSync(path.join(dir, 'bin'), { recursive: true });
fs.copyFileSync(path.join(root, 'bin', 'axiom.js'), path.join(dir, 'bin', 'axiom.js'));
fs.chmodSync(path.join(dir, 'bin', 'axiom.js'), 0o755);
launcher.version = version;
for (const name of Object.keys(launcher.optionalDependencies)) {
  launcher.optionalDependencies[name] = version;
}
fs.writeFileSync(path.join(dir, 'package.json'), JSON.stringify(launcher, null, 2) + '\n');
console.log(`wrote npm packages ${version} to ${out}`);
