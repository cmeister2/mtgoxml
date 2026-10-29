import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
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

test("the Cargo plugin prepares manifest and lockfile versions without publishing", async () => {
  const require = createRequire(import.meta.url);
  const pluginName = "@semantic-release-cargo/semantic-release-cargo";
  assert.ok(config.plugins.includes(pluginName));
  const pluginUrl = pathToFileURL(require.resolve(pluginName)).href;
  const directory = await mkdtemp(join(tmpdir(), "mtgoxml-release-"));
  try {
    await mkdir(join(directory, "src"));
    await writeFile(join(directory, "src", "lib.rs"), "//! Release preparation fixture.\n");
    await writeFile(
      join(directory, "Cargo.toml"),
      '[package]\nname = "mtgoxml-release-fixture"\nversion = "0.0.0"\nedition = "2024"\n',
    );
    await writeFile(
      join(directory, "Cargo.lock"),
      'version = 4\n\n[[package]]\nname = "mtgoxml-release-fixture"\nversion = "0.0.0"\n',
    );
    const result = spawnSync(
      process.execPath,
      [
        "--input-type=module",
        "--eval",
        `import plugin from ${JSON.stringify(pluginUrl)};
         await plugin.prepare({}, {
           nextRelease: { version: "1.2.3" },
           logger: { info() {}, warn() {}, error() {} },
         });`,
      ],
      { cwd: directory, encoding: "utf8" },
    );
    assert.equal(result.status, 0, result.stderr || result.error?.message);
    const manifest = await readFile(join(directory, "Cargo.toml"), "utf8");
    const lock = await readFile(join(directory, "Cargo.lock"), "utf8");
    assert.match(manifest, /^version = "1\.2\.3"$/m);
    assert.match(lock, /name = "mtgoxml-release-fixture"\nversion = "1\.2\.3"/);
    assert.doesNotMatch(manifest + lock, /version = "0\.0\.0"/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
