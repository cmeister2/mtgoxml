import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import config from "./release.config.js";

test("the actual semantic-release generator renders the configured preset", async () => {
  const require = createRequire(import.meta.url);
  const releaseRequire = createRequire(require.resolve("semantic-release"));
  const generatorPath = releaseRequire.resolve(
    "@semantic-release/release-notes-generator",
  );
  const { generateNotes } = await import(pathToFileURL(generatorPath));
  const [, options] = config.plugins.find(
    (plugin) =>
      Array.isArray(plugin) &&
      plugin[0] === "@semantic-release/release-notes-generator",
  );
  const hash = "1234567890abcdef1234567890abcdef12345678";
  const notes = await generateNotes(options, {
    cwd: process.cwd(),
    options: { repositoryUrl: config.repositoryUrl },
    branch: { name: "main" },
    lastRelease: { version: "1.0.0", gitTag: "1.0.0" },
    nextRelease: { version: "2.0.0", gitTag: "2.0.0" },
    commits: [
      { hash, message: "feat: parse application manifests" },
      { hash, message: "fix: recognize XML namespaces\n\nCloses #42" },
      {
        hash,
        message:
          "feat: distinguish manifest kinds\n\nBREAKING CHANGE: Manifest now exposes its kind.",
      },
    ],
    logger: { log() {}, error() {} },
  });
  for (const text of [
    "Features",
    "Bug Fixes",
    "BREAKING CHANGES",
    "parse application manifests",
    "recognize XML namespaces",
    "distinguish manifest kinds",
    "Manifest now exposes its kind.",
    "https://github.com/cmeister2/mtgoxml/compare/1.0.0...2.0.0",
    "https://github.com/cmeister2/mtgoxml/issues/42",
    `https://github.com/cmeister2/mtgoxml/commit/${hash}`,
  ]) {
    assert.ok(notes.includes(text), `release notes should contain ${text}`);
  }
});

test("the Cargo plugin prepares a public crate and preserves its private workspace member", async () => {
  const require = createRequire(import.meta.url);
  const pluginName = "@semantic-release-cargo/semantic-release-cargo";
  assert.ok(config.plugins.includes(pluginName));
  const lockRepairName = "./scripts/prepare-workspace-lock.cjs";
  assert.equal(config.plugins[config.plugins.indexOf(pluginName) + 1], lockRepairName);
  const pluginUrl = pathToFileURL(require.resolve(pluginName)).href;
  const lockRepairUrl = pathToFileURL(require.resolve(lockRepairName)).href;
  const directory = await mkdtemp(join(tmpdir(), "mtgoxml-release-"));
  try {
    const cargo = (...arguments_) => {
      const result = spawnSync("cargo", arguments_, {
        cwd: directory,
        encoding: "utf8",
        timeout: 60_000,
      });
      assert.equal(result.status, 0, result.stderr || result.error?.message);
      return result.stdout;
    };
    const exporter = join(directory, "crates", "mtgoxml-export");
    await mkdir(join(directory, "src"), { recursive: true });
    await mkdir(join(exporter, "src"), { recursive: true });
    await writeFile(join(directory, "src", "lib.rs"),
      "//! Public release preparation fixture.\n/// Value consumed by the private exporter.\npub fn marker() -> u8 { 1 }\n");
    await writeFile(join(exporter, "src", "main.rs"),
      "//! Private workspace exporter fixture.\nfn main() { let _ = mtgoxml_release_fixture::marker(); }\n");
    await writeFile(
      join(directory, "Cargo.toml"),
      `[workspace]
members = [".", "crates/mtgoxml-export"]
default-members = [".", "crates/mtgoxml-export"]
resolver = "3"

[workspace.package]
edition = "2024"
rust-version = "1.85"
license = "MIT"

[workspace.lints.rust]
missing_docs = "deny"

[workspace.lints.clippy]
missing_docs_in_private_items = "deny"
missing_errors_doc = "deny"
missing_panics_doc = "deny"

[package]
name = "mtgoxml-release-fixture"
version = "0.0.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
description = "Public crate release preparation fixture"
repository = "https://github.com/cmeister2/mtgoxml"
include = ["/src/**", "/Cargo.toml", "/Cargo.lock"]

[lints]
workspace = true
`,
    );
    await writeFile(
      join(exporter, "Cargo.toml"),
      `[package]
name = "mtgoxml-export-release-fixture"
version = "0.0.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish = false

[lints]
workspace = true

[dependencies]
mtgoxml-release-fixture = { path = "../.." }
`,
    );
    cargo("generate-lockfile", "--offline");
    const result = spawnSync(
      process.execPath,
      [
        "--input-type=module",
        "--eval",
        `for (const url of ${JSON.stringify([pluginUrl, lockRepairUrl])}) {
           const { default: plugin } = await import(url);
           await plugin.prepare({}, {
             cwd: process.cwd(),
             nextRelease: { version: "1.2.3" },
             logger: { info() {}, warn() {}, error() {} },
           });
         }`,
      ],
      { cwd: directory, encoding: "utf8", timeout: 60_000 },
    );
    assert.equal(result.status, 0, result.stderr || result.error?.message);
    const manifest = await readFile(join(directory, "Cargo.toml"), "utf8");
    const privateManifest = await readFile(join(exporter, "Cargo.toml"), "utf8");
    const lock = await readFile(join(directory, "Cargo.lock"), "utf8");
    assert.match(manifest, /^version = "1\.2\.3"$/m);
    assert.match(lock, /name = "mtgoxml-release-fixture"\nversion = "1\.2\.3"/);
    assert.match(privateManifest, /^publish = false$/m);
    assert.match(manifest, /^rust-version = "1\.85"$/m);
    assert.match(manifest, /^missing_docs = "deny"$/m);
    assert.match(privateManifest, /^workspace = true$/m);

    const metadata = JSON.parse(cargo("metadata", "--locked", "--offline", "--no-deps", "--format-version=1"));
    assert.equal(metadata.workspace_members.length, 2);
    const publicCrate = metadata.packages.find((item) => item.name === "mtgoxml-release-fixture");
    const privateCrate = metadata.packages.find((item) => item.name === "mtgoxml-export-release-fixture");
    assert.equal(publicCrate.version, "1.2.3");
    assert.equal(publicCrate.rust_version, "1.85");
    assert.equal(privateCrate.rust_version, "1.85");
    assert.equal(publicCrate.publish, null);
    assert.deepEqual(privateCrate.publish, []);
    assert.equal(privateCrate.dependencies[0].name, publicCrate.name);
    assert.equal(privateCrate.dependencies[0].path, dirname(publicCrate.manifest_path));
    cargo("check", "--locked", "--offline", "--workspace", "--all-targets");
    for (const item of [publicCrate, privateCrate]) {
      assert.match(lock, new RegExp(`name = "${item.name}"\\nversion = "${item.version.replaceAll(".", "\\.")}"`));
    }
    // Exercise Cargo's actual package verification, without publishing.
    cargo("package", "--locked", "--offline", "--allow-dirty", "-p", publicCrate.name);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
