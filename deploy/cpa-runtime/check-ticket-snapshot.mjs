// Real IPC v3 master/worker probe using only local upstreams and fake credentials.
// Run with CODEX_TEST_BINARY=/absolute/path/to/codex-app-server node this-file.mjs.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { createServer as createTcpServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";

const binary = process.env.CODEX_TEST_BINARY;
assert(binary, "CODEX_TEST_BINARY must select a real v3 worker");
const fixture = await mkdtemp(join(tmpdir(), "codex-ticket-v3-"));
const calls = [];
let logs = "";
let child;
let socket;
const upstream = createServer(async (request, response) => {
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  if (request.method !== "POST" || request.url !== "/v1/responses") {
    response.writeHead(404).end("{}");
    return;
  }
  const body = JSON.parse(Buffer.concat(chunks));
  calls.push({ body, headers: request.headers });
  const side = body.fixture_side;
  const mode = body.fixture_mode;
  const state = `ticket-${side}-${mode}`;
  const headers = {
    "content-type": "text/event-stream",
    "set-cookie":
      mode === "delete"
        ? "__oailb=; Max-Age=0"
        : `__oailb=response-${side}; Max-Age=120`,
    "x-fixture-api-key": `response-key-${side}`,
  };
  if (mode !== "metadata") headers["x-codex-turn-state"] = state;
  response.writeHead(200, headers);
  response.write(": heartbeat\n\n");
  if (mode === "metadata") {
    const metadata = `data: ${JSON.stringify({ type: "codex.response.metadata", headers: { "x-codex-turn-state": state } })}\r\n\r\n`;
    response.write(metadata.slice(0, 17));
    await delay(5);
    response.write(metadata.slice(17));
  }
  response.end("data: [DONE]\n\n");
});

try {
  upstream.listen(0, "127.0.0.1");
  await once(upstream, "listening");
  const url = `http://127.0.0.1:${upstream.address().port}`;
  const reservation = createTcpServer().listen(0, "127.0.0.1");
  await once(reservation, "listening");
  const port = reservation.address().port;
  await new Promise((resolve) => reservation.close(resolve));
  await mkdir(join(fixture, "auths"));
  const encode = (value) =>
    Buffer.from(JSON.stringify(value)).toString("base64url");
  for (const side of ["left", "right"]) {
    const account = `${side}.json`;
    const token = `${encode({ alg: "none" })}.${encode({ "email": `${side}@example.com`, "https://api.openai.com/auth": { chatgpt_account_id: side, chatgpt_user_id: side, chatgpt_plan_type: "plus" } })}.sig`;
    await writeFile(
      join(fixture, "auths", account),
      JSON.stringify({
        type: "codex",
        id_token: token,
        access_token: `fake-access-${side}`,
        refresh_token: "fake-refresh",
        account_id: side,
        email: `${side}@example.com`,
        last_refresh: new Date().toISOString(),
        expired: "2099-01-01T00:00:00Z",
        codex_cli: { enabled: true },
      }),
    );
    const home = join(fixture, "state", account);
    await mkdir(home, { recursive: true });
    await writeFile(
      join(home, "config.toml"),
      `
model = "gpt-5.2"
model_provider = "fixture"
approval_policy = "never"
sandbox_mode = "read-only"
cli_auth_credentials_store = "file"
chatgpt_base_url = "${url}"
[model_providers.fixture]
name = "OpenAI"
base_url = "${url}/v1"
wire_api = "responses"
requires_openai_auth = true
supports_websockets = false
[model_providers.fixture.http_headers]
Cookie = "__cflb=request-${side}; __oailb=old-${side}"
`,
    );
  }
  const env = {
    ...process.env,
    CODEX_CPA_AUTH_DIR: join(fixture, "auths"),
    CODEX_HOME: join(fixture, "state"),
    CODEX_CPA_PORT: String(port),
    CODEX_APP_SERVER_LOGIN_ISSUER: url,
    CODEX_REFRESH_TOKEN_URL_OVERRIDE: `${url}/oauth/token`,
    RUST_LOG: "codex_app_server=trace",
    NO_PROXY: "localhost,127.0.0.1",
  };
  for (const key of [
    "CODEX_CPA_AUTH_FILE",
    "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
  ])
    delete env[key];
  child = spawn(binary, [], { env, stdio: ["ignore", "pipe", "pipe"] });
  for (const stream of [child.stdout, child.stderr])
    stream.on("data", (chunk) => {
      logs += chunk.toString();
    });
  let ready = false;
  for (let i = 0; i < 120; i++) {
    assert.equal(child.exitCode, null, "worker exited");
    try {
      ready = (
        await fetch(`http://127.0.0.1:${port}/readyz`, {
          signal: AbortSignal.timeout(1000),
        })
      ).ok;
    } catch {
      /* Startup. */
    }
    if (ready) break;
    await delay(250);
  }
  assert(ready);
  socket = new WebSocket(`ws://127.0.0.1:${port}/cpa/v1/ws`);
  const messages = [];
  socket.addEventListener("message", (event) =>
    messages.push(JSON.parse(event.data)),
  );
  await once(socket, "open");
  const send = (value) => socket.send(JSON.stringify(value));
  async function waitFor(predicate) {
    for (let i = 0; i < 600; i++) {
      const found = messages.find(predicate);
      if (found) return found;
      await delay(50);
    }
    throw new Error("Timed out waiting for actual worker IPC");
  }
  send({
    id: 1,
    method: "initialize",
    params: {
      clientInfo: { name: "ticket-probe", version: "1" },
      capabilities: { experimentalApi: true },
    },
  });
  assert((await waitFor((m) => m.id === 1)).result);
  send({ method: "initialized" });
  send({ id: 2, method: "cpa/capabilities/read", params: {} });
  const capabilities = (await waitFor((m) => m.id === 2)).result;
  assert.equal(capabilities.protocolVersion, 3);
  assert.equal(capabilities.ticketSnapshots, true);
  const cases = [
    ["left", "header", undefined],
    ["left", "header", true],
    ["right", "header", true],
    ["left", "metadata", true],
    ["right", "delete", true],
  ];
  for (const [index, [side, mode, capture]] of cases.entries()) {
    send({
      id: index + 10,
      method: "cpa/inference/start",
      params: {
        requestId: `probe-${index}`,
        credentialId: `${side}.json`,
        sourceFormat: "openai-response",
        operation: "responses",
        sessionId: `probe-${index}`,
        captureTicketSnapshot: capture,
        request: {
          model: "gpt-5.2",
          input: [],
          stream: true,
          fixture_side: side,
          fixture_mode: mode,
        },
      },
    });
  }
  for (const [index, [side, mode, capture]] of cases.entries()) {
    await waitFor(
      (m) =>
        (m.params?.requestId === `probe-${index}` &&
          ["cpa/inference/completed", "cpa/inference/error"].includes(
            m.method,
          )) ||
        (m.id === index + 10 && m.error),
    );
    const reply = messages.find((m) => m.id === index + 10);
    assert(reply?.result, `inference ${index} rejected`);
    assert.equal(
      reply.result.headers["x-fixture-api-key"][0],
      `response-key-${side}`,
    );
    const sequence = messages.filter(
      (m) => m.params?.requestId === `probe-${index}`,
    );
    assert.equal(sequence.at(-1).method, "cpa/inference/completed");
    const snapshots = sequence.filter(
      (m) => m.method === "cpa/inference/ticket",
    );
    assert.equal(snapshots.length, capture && mode !== "delete" ? 1 : 0);
    if (snapshots.length) {
      const ticket = snapshots[0].params;
      assert.equal(ticket.state, `ticket-${side}-${mode}`);
      assert.equal(ticket.credentialId, `${side}.json`);
      assert.equal(ticket.model, "gpt-5.2");
      assert.equal(ticket.route.__cflb.value, `request-${side}`);
      assert.equal(ticket.route.__oailb.value, `response-${side}`);
      assert(
        Date.parse(ticket.route.__oailb.expires_at) >
          Date.parse(ticket.observedAt),
      );
    }
    const request = sequence.find(
      (m) =>
        m.method === "cpa/inference/upstream" && m.params.kind === "request",
    ).params;
    assert.equal(
      request.headers.authorization[0],
      `Bearer fake-access-${side}`,
    );
    assert.equal(
      request.headers.cookie[0],
      `__cflb=request-${side}; __oailb=old-${side}`,
    );
    assert(!JSON.parse(request.body).captureTicketSnapshot);
    const body = sequence
      .filter((m) => m.method === "cpa/inference/body")
      .map((m) => Buffer.from(m.params.bodyBase64, "base64"))
      .map((b) => b.toString())
      .join("");
    assert(
      body.startsWith(": heartbeat\n\n") && body.endsWith("data: [DONE]\n\n"),
    );
  }
  assert.equal(calls.length, cases.length);
  await delay(100);
  for (const side of ["left", "right"]) {
    for (const secret of [
      `fake-access-${side}`,
      `response-key-${side}`,
      `__cflb=request-${side}`,
      `__oailb=response-${side}`,
      `ticket-${side}-header`,
      `ticket-${side}-metadata`,
    ])
      assert(!logs.includes(secret), "ordinary logs exposed private material");
  }
  console.log(
    JSON.stringify({
      protocolVersion: 3,
      ticketSnapshots: true,
      realWorkerRequests: calls.length,
      snapshots: 3,
      rawIpcHeaders: true,
      ordinaryLogsPrivate: true,
    }),
  );
} finally {
  socket?.close();
  child?.kill();
  upstream.closeAllConnections();
  await new Promise((resolve) => upstream.close(resolve));
  await rm(fixture, { recursive: true, force: true });
}
