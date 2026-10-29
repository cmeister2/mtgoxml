/** Release versions are injected into an ephemeral CI checkout. */
module.exports = {
  branches: ["main"],
  repositoryUrl: "https://github.com/cmeister2/mtgoxml.git",
  tagFormat: "${version}",
  plugins: [
    "@semantic-release/commit-analyzer",
    [
      "@semantic-release/release-notes-generator",
      { preset: "conventionalcommits" },
    ],
    "@semantic-release-cargo/semantic-release-cargo",
    [
      "@semantic-release/github",
      { successComment: false, failComment: false, releasedLabels: false },
    ],
  ],
};
