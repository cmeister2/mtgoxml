const { execFileSync } = require("node:child_process");

/** Repair the shared workspace lock after semantic-release-cargo prepares versions. */
function prepare(_options, context) {
  // Version 2.4.84 updates member manifests but looks for a separate lock beside each one.
  // Cargo repairs the shared lock while retaining existing registry package versions offline.
  execFileSync(process.env.CARGO || "cargo", ["update", "--workspace", "--offline"], {
    cwd: context.cwd || process.cwd(),
    stdio: "inherit",
  });
}

module.exports = { prepare };
