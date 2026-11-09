// Select the published snapshot or its isolated avatar bootstrap for the Pages build.
import { appendFileSync, readFileSync } from "node:fs";

const [filename = "snapshot/snapshot.json"] = process.argv.slice(2);
const snapshot = JSON.parse(readFileSync(filename, "utf8"));
const legacy = snapshot.animated_avatars == null;
appendFileSync(process.env.GITHUB_OUTPUT,
  `legacy=${legacy}\nsnapshot_directory=${legacy ? "avatar-snapshot" : "snapshot"}\n`);
