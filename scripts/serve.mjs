// Serve generated browser assets locally with the same project-prefix behavior as Pages.
import { createReadStream } from "node:fs";
import { realpath, stat } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";

const [directory = "_site", portText = "4173", prefixText = "/"] = process.argv.slice(2);
const root = await realpath(directory);
const port = Number(portText);
const prefix = "/" + prefixText.replace(/^\/+|\/+$/g, "");
const basePath = prefix === "/" ? prefix : prefix + "/";
if (!Number.isInteger(port) || port < 1 || port > 65535) {
  throw new Error("The port must be an integer between 1 and 65535.");
}

const mimeTypes = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".gz": "application/gzip",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
};

const server = createServer(async (request, response) => {
  // Support only read-only requests; every missing file remains a real 404.
  if (!["GET", "HEAD"].includes(request.method)) {
    response.writeHead(405, { Allow: "GET, HEAD" });
    response.end();
    return;
  }
  try {
    const pathname = decodeURIComponent(new URL(request.url, "http://127.0.0.1").pathname);
    if (basePath !== "/" && pathname === prefix) {
      response.writeHead(308, { Location: basePath });
      response.end();
      return;
    }
    if (!pathname.startsWith(basePath) || pathname.includes("\0")) {
      response.writeHead(404);
      response.end();
      return;
    }
    const relative = pathname.slice(basePath.length) || "index.html";
    const filename = path.resolve(root, relative);
    if (filename !== root && !filename.startsWith(root + path.sep)) {
      response.writeHead(404);
      response.end();
      return;
    }

    // Check real paths as well, so a symlink cannot expose files outside the site.
    const resolved = await realpath(filename);
    if (resolved !== root && !resolved.startsWith(root + path.sep)) {
      response.writeHead(404);
      response.end();
      return;
    }
    const info = await stat(resolved);
    if (!info.isFile()) {
      response.writeHead(404);
      response.end();
      return;
    }
    response.writeHead(200, {
      "Content-Type": mimeTypes[path.extname(resolved)] || "application/octet-stream",
      "Content-Length": info.size,
      "Cache-Control": "no-store",
      "X-Content-Type-Options": "nosniff",
    });
    if (request.method === "HEAD") {
      response.end();
      return;
    }
    createReadStream(resolved).on("error", () => response.destroy()).pipe(response);
  } catch (error) {
    // Invalid request paths and missing assets do not fall back to the app shell.
    const status = error instanceof URIError ? 400 : error.code === "ENOENT" || error.code === "ENOTDIR" ? 404 : 500;
    response.writeHead(status);
    response.end();
  }
});

server.on("error", (error) => {
  console.error(error.message);
  process.exitCode = 1;
});
server.listen(port, "127.0.0.1", () => {
  console.log("Serving " + root + " at http://127.0.0.1:" + port + basePath);
});
for (const signal of ["SIGINT", "SIGTERM"]) {
  // Let Playwright and an interactive terminal stop the preview cleanly.
  process.on(signal, () => server.close());
}
