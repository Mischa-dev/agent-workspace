import { get } from "node:http";

// Use node:http so the configured public Host is preserved while probing the
// container's loopback address. Fetch can discard a supplied Host header.
const port = Number(process.env.PORT || 4310);
const host = new URL(process.env.PUBLIC_ORIGIN || `http://127.0.0.1:${port}`)
  .host;
const expectWorker = process.argv.includes("--worker");
const request = get(
  {
    hostname: "127.0.0.1",
    port,
    path: "/health",
    headers: { Host: host },
    timeout: 2000,
    agent: false,
  },
  (response) => {
    let body = "";
    response.setEncoding("utf8");
    response.on("data", (chunk) => {
      body += chunk;
    });
    response.on("end", () => {
      try {
        const value = JSON.parse(body);
        process.exit(
          response.statusCode === 200 &&
            value.ok &&
            (!expectWorker || value.workerOnline)
            ? 0
            : 1,
        );
      } catch {
        process.exit(1);
      }
    });
    response.on("error", () => process.exit(1));
  },
);
request.on("timeout", () =>
  request.destroy(new Error("Health probe timed out")),
);
request.on("error", () => process.exit(1));
