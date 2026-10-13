#!/usr/bin/env node
// Codex Server IPC v5: one request per connection, with no external dependencies.
import { randomUUID } from "node:crypto";
import { once } from "node:events";

const capabilitiesOnly = process.argv.includes("--capabilities");
const credential = process.env.CODEX_SERVER_CREDENTIAL;
const model = process.env.CODEX_SERVER_MODEL;
if (!capabilitiesOnly && (!credential || !model)) {
  console.error(
    "Set CODEX_SERVER_CREDENTIAL and CODEX_SERVER_MODEL, or use --capabilities.",
  );
  process.exit(1);
}

const socket = new WebSocket(
  process.env.CODEX_SERVER_URL ?? "ws://127.0.0.1:38317/cpa/v1/ws",
);
const queue = [];
let waiter;
let closed;
const stop = (error) => {
  closed = error;
  if (waiter) {
    waiter.reject(error);
    waiter = undefined;
  }
};
socket.addEventListener("message", ({ data }) => {
  try {
    const message = JSON.parse(data);
    if (waiter) {
      waiter.resolve(message);
      waiter = undefined;
    } else {
      queue.push(message);
    }
  } catch (error) {
    stop(error);
  }
});
socket.addEventListener("close", () =>
  stop(new Error("Connection closed before completion")),
);
socket.addEventListener("error", () =>
  stop(new Error("WebSocket connection failed")),
);
function nextMessage() {
  if (queue.length) return Promise.resolve(queue.shift());
  if (closed) return Promise.reject(closed);
  return new Promise((resolve, reject) => {
    waiter = { resolve, reject };
  });
}
function send(message) {
  socket.send(JSON.stringify(message));
}
async function rpc(id, method, params) {
  send({ id, method, params });
  for (;;) {
    const message = await nextMessage();
    if (message.id !== id) continue;
    if (message.error) throw new Error(JSON.stringify(message.error));
    return message.result;
  }
}

let cancel;
try {
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener(
      "error",
      () => reject(new Error("WebSocket connection failed")),
      { once: true },
    );
    socket.addEventListener(
      "close",
      () => reject(new Error("Connection closed before initialization")),
      { once: true },
    );
  });
  await rpc(1, "initialize", {
    clientInfo: { name: "codex-server-example", version: "1.0.0" },
    capabilities: { experimentalApi: true },
  });
  send({ method: "initialized" });
  const capabilities = await rpc(2, "cpa/capabilities/read", {});
  if (
    capabilities.protocolVersion !== 5 ||
    capabilities.executionMode !== "inference-only" ||
    !capabilities.rawBody ||
    !capabilities.operations?.includes("responses")
  ) {
    throw new Error("Server does not support the required v5 message contract");
  }
  if (capabilitiesOnly) {
    console.log(JSON.stringify(capabilities, null, 2));
  } else {
    const requestId = randomUUID();
    const session = process.env.CODEX_SERVER_SESSION ?? randomUUID();
    let cancelled = false;
    cancel = () => {
      if (cancelled) {
        socket.close();
        return;
      }
      cancelled = true;
      send({ id: 4, method: "cpa/inference/cancel", params: { requestId } });
    };
    process.on("SIGINT", cancel);
    send({
      id: 3,
      method: "cpa/inference/start",
      params: {
        requestId,
        credentialId: credential,
        operation: "responses",
        sourceFormat: "openai-response",
        sessionId: session,
        request: {
          model,
          stream: true,
          store: false,
          input:
            process.argv.slice(2).join(" ") || "Reply with one short sentence.",
          session_id: session,
          thread_id: session,
          prompt_cache_key: session,
          turn_id: randomUUID(),
        },
      },
    });
    let businessDone = false;
    let diagnosticsDone = false;
    let failure;
    while (!businessDone || !diagnosticsDone) {
      const message = await nextMessage();
      if (message.error && message.id === 3) {
        failure = JSON.stringify(message.error);
        businessDone = true;
      }
      if (message.error && message.id === 4) {
        console.error("Cancellation failed:", JSON.stringify(message.error));
      }
      if (message.params?.requestId !== requestId) continue;
      if (message.method === "cpa/inference/diagnostics/completed") {
        diagnosticsDone = true;
        continue;
      }
      if (message.method === "cpa/inference/upstream") continue;
      let output = message;
      if (message.method === "cpa/inference/event") {
        const { bodyBase64, ...params } = message.params;
        output = {
          method: message.method,
          params: {
            ...params,
            body: Buffer.from(bodyBase64, "base64").toString("utf8"),
          },
        };
      } else if (message.method === "cpa/inference/completed") {
        businessDone = true;
      } else if (message.method === "cpa/inference/error") {
        failure = JSON.stringify(message.params);
        businessDone = true;
      } else {
        continue;
      }
      // Each request streams one native attempt. Errors end that request; this
      // example never silently resends or joins another response to its output.
      if (!process.stdout.write(JSON.stringify(output) + "\n")) {
        await once(process.stdout, "drain");
      }
    }
    if (failure) throw new Error(failure);
  }
} catch (error) {
  console.error(error.message ?? String(error));
  process.exitCode = 1;
} finally {
  if (cancel) process.off("SIGINT", cancel);
  socket.close();
}
