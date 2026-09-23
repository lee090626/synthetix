const { execFileSync, spawnSync } = require("node:child_process");
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

const extensionRoot = __dirname;
const packageJson = JSON.parse(
  fs.readFileSync(path.join(extensionRoot, "package.json"), "utf8")
);
const vsixPath = path.resolve(
  process.argv[2] ??
    path.join(extensionRoot, `synthetix-vscode-${packageJson.version}.vsix`)
);
const source = fs.readFileSync(path.join(extensionRoot, "extension.js"));
const packaged = execFileSync("unzip", ["-p", vsixPath, "extension/extension.js"]);
const sourceHash = crypto.createHash("sha256").update(source).digest("hex");
const packageHash = crypto.createHash("sha256").update(packaged).digest("hex");
const sourceCli = fs.readFileSync(path.join(extensionRoot, "bin/darwin-arm64/axiom"));
const packagedCli = execFileSync("unzip", [
  "-p",
  vsixPath,
  "extension/bin/darwin-arm64/axiom",
], { maxBuffer: 64 * 1024 * 1024 });
const sourceCliHash = crypto.createHash("sha256").update(sourceCli).digest("hex");
const packageCliHash = crypto.createHash("sha256").update(packagedCli).digest("hex");

if (sourceHash !== packageHash) {
  throw new Error(
    `VSIX extension.js hash mismatch: source=${sourceHash} package=${packageHash}`
  );
}
if (sourceCliHash !== packageCliHash) {
  throw new Error(
    `VSIX bundled CLI hash mismatch: source=${sourceCliHash} package=${packageCliHash}`
  );
}
if (packageJson.main !== "./extension.js") {
  throw new Error(`unexpected VS Code extension entry point: ${packageJson.main}`);
}

const syntax = spawnSync(process.execPath, ["--check", path.join(extensionRoot, "extension.js")], {
  encoding: "utf8",
});
if (syntax.status !== 0) throw new Error(syntax.stderr || "extension.js syntax check failed");

const smoke = spawnSync(process.execPath, [path.join(extensionRoot, "smoke-test.js")], {
  encoding: "utf8",
});
if (smoke.status !== 0) throw new Error(smoke.stderr || "VS Code activation smoke test failed");

process.stdout.write(`extension.js sha256 ${sourceHash}\n`);
process.stdout.write(`bundled CLI sha256 ${sourceCliHash}\n`);
process.stdout.write("VSIX source hash and extension activation smoke test passed\n");
