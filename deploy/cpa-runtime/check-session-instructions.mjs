// Real master/worker contract checks against a local HTTP/WebSocket model stub.
// No OpenAI account, network model call, or nonstandard Node package is needed.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { createServer as tcpServer } from 'node:net';
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { zstdDecompressSync } from 'node:zlib';

const binary = process.argv[2];
assert(binary, 'usage: node check-session-instructions.mjs BINARY [--serve] [--port PORT] [--keep] [--http] [--http-only] [--upload-recovery]');
const args = process.argv.slice(3);
const serve = args.includes('--serve');
const keep = serve || args.includes('--keep');
const httpOnly = args.includes('--http');
const controlHttpOnly = args.includes('--http-only');
const uploadRecovery = args.includes('--upload-recovery');
assert(!uploadRecovery || (!httpOnly && !controlHttpOnly && !serve), 'upload recovery tests require native WebSocket');

const portArg = args.indexOf('--port');
const home = await mkdtemp(join(tmpdir(), 'codex-session-contract-'));
const baseline = 'PRIVATE_BASELINE_SESSION_CONTRACT_1264';
const digest = value => createHash('sha256').update(value).digest('hex');
const requests = [];
const sockets = new Set();
const gates = new Map();
let connectionSequence = 0;
let responseSequence = 0;
let child;
let stderr = '';
let rpcSequence = 100;

function frame(text, opcode = 1) {
  const data = Buffer.isBuffer(text) ? text : Buffer.from(text);
  const header = Buffer.alloc(data.length < 126 ? 2 : data.length < 65536 ? 4 : 10);
  header[0] = 0x80 | opcode;
  if (header.length === 2) header[1] = data.length;
  else if (header.length === 4) { header[1] = 126; header.writeUInt16BE(data.length, 2); }
  else { header[1] = 127; header.writeBigUInt64BE(BigInt(data.length), 2); }
  return Buffer.concat([header, data]);
}

function textOf(body) {
  return typeof body.input === 'string' ? body.input : (body.input ?? [])
    .filter(item => item.role === 'user').flatMap(item => typeof item.content === 'string'
      ? [item.content] : (item.content ?? []).map(part => part.text ?? '')).join('\n');
}

async function model(body, transport, connection, send, close) {
  const text = textOf(body);
  const id = text.includes('same-id') ? 'same-literal-response-id' : `resp-${++responseSequence}`;
  const record = { transport, connection, body, id };
  requests.push(record);
  await writeFile(join(home, 'mock-requests.json'), JSON.stringify(requests, null, 2), { mode: 0o600 });
  assert.equal(JSON.stringify(body).includes('cpa_session_instructions'), false);
  if (text.includes('close1009')) { close(1009); return; }
  const parent = body.previous_response_id ?? null;
  if (!text.includes('missing-created')) {
    send({ type: 'response.created', response: { id, status: 'in_progress', previous_response_id: parent } });
  }
  if (text.includes('hold-')) await new Promise(resolve => gates.set(text, resolve));
  const echoedParent = text.includes('wrong-parent') ? 'unrelated-parent' : parent;
  send({ type: 'response.completed', response: {
    id, status: 'completed', previous_response_id: echoedParent, output: [],
    usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 },
  } });
  if (text.includes('close-after')) close(1000);
}

const upstream = createServer(async (req, res) => {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  if (req.method === 'POST' && /\/responses(?:\/lite)?(?:\?|$)/.test(req.url)) {
    const wire = Buffer.concat(chunks);
    const json = req.headers['content-encoding'] === 'zstd' ? zstdDecompressSync(wire) : wire;
    const body = JSON.parse(json.toString());
    if (textOf(body).includes('redirect-full') && req.url === '/v1/responses') {
      res.writeHead(307, { location: '/v1/redirected/responses' }); res.end(); return;
    }
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    await model(body, 'http', null, value => res.write(`data: ${JSON.stringify(value)}\n\n`), () => res.end());
    res.end();
  } else if (/\/models(?:\?|$)/.test(req.url)) {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(await readFile(new URL('../../codex-rs/models-manager/models.json', import.meta.url)));
  } else {
    res.writeHead(404, { 'content-type': 'application/json' }); res.end('{}');
  }
});

upstream.on('upgrade', (req, socket, head) => {
  const connection = ++connectionSequence;
  const accept = createHash('sha1').update(req.headers['sec-websocket-key'] + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64');
  socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\nx-codex-websocket-max-message-bytes: ${uploadRecovery ? 16 * 1024 * 1024 : 65536}\r\n\r\n`);
  sockets.add(socket); socket.on('close', () => sockets.delete(socket)); socket.on('error', () => {});
  let pending = head;
  let fragments = [];
  const close = code => { const bytes = Buffer.alloc(2); bytes.writeUInt16BE(code); socket.end(frame(bytes, 8)); };
  function consume() {
    while (pending.length >= 2) {
      const opcode = pending[0] & 15;
      const final = Boolean(pending[0] & 128);
      const masked = Boolean(pending[1] & 128);
      let length = pending[1] & 127;
      let offset = 2;
      if (length === 126) { if (pending.length < 4) return; length = pending.readUInt16BE(2); offset = 4; }
      else if (length === 127) { if (pending.length < 10) return; length = Number(pending.readBigUInt64BE(2)); offset = 10; }
      const maskSize = masked ? 4 : 0;
      if (pending.length < offset + maskSize + length) return;
      const mask = pending.subarray(offset, offset + maskSize);
      const bytes = Buffer.from(pending.subarray(offset + maskSize, offset + maskSize + length));
      pending = pending.subarray(offset + maskSize + length);
      if (masked) for (let i = 0; i < bytes.length; i++) bytes[i] ^= mask[i % 4];
      if (opcode === 8) { socket.end(frame(bytes, 8)); return; }
      if (opcode === 9) { socket.write(frame(bytes, 10)); continue; }
      if (opcode !== 1 && opcode !== 0) continue;
      if (uploadRecovery && opcode === 1 && !final) {
        const prefix = bytes.subarray(0, 8192).toString();
        const parent = prefix.match(/"previous_response_id":"(resp-\d+)"/)?.[1];
        assert(parent, 'first fragment carries the proven parent');
        const matchesParent = prefix.includes('recover-upload-current');
        assert(matchesParent || prefix.includes('recover-upload-wrong-parent'));
        const id = `resp-${++responseSequence}`;
        const response = { id, previous_response_id: matchesParent ? parent : 'wrong-parent' };
        requests.push({ transport: 'websocket', connection, id, parent, partial: true });
        // Reply after the first fragment. Closing without reading the rest
        // forces the worker to use the replies already buffered during upload.
        const created = frame(JSON.stringify({ type: 'response.created', response }));
        const completed = frame(JSON.stringify({
          type: 'response.completed', response: { ...response, status: 'completed', output: [] },
        }));
        socket.write(Buffer.concat([created, completed]));
        socket.pause();
        close(1000);
        return;
      }
      fragments.push(bytes);
      if (!final) continue;
      const body = JSON.parse(Buffer.concat(fragments).toString()); fragments = [];
      void model(body, 'websocket', connection, value => socket.write(frame(JSON.stringify(value))), close)
        .catch(error => { stderr += String(error); socket.destroy(); });
    }
  }
  socket.on('data', chunk => { pending = Buffer.concat([pending, chunk]); consume(); });
  consume();
});

async function openClient(port) {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/`);
  await new Promise((resolve, reject) => {
    ws.addEventListener('open', resolve, { once: true });
    ws.addEventListener('error', reject, { once: true });
  });
  const pending = new Map();
  const exchanges = new Map();
  ws.addEventListener('message', event => {
    const message = JSON.parse(event.data);
    if (message.id !== undefined) pending.get(message.id)?.(message);
    const state = exchanges.get(message.params?.requestId);
    if (state) {
      if (message.method === 'cpa/inference/upstream') state.logs.push(message.params);
      if (message.method === 'cpa/inference/event' || message.method === 'cpa/inference/body')
        state.body += Buffer.from(message.params.bodyBase64, 'base64').toString();
      if (message.method === 'cpa/inference/error') { state.error = message.params; state.done = true; }
      if (message.method === 'cpa/inference/completed') state.done = true;
      if (message.method === 'cpa/inference/diagnostics/completed') state.diagnostics = true;
      state.finish();
    }
  });
  async function rpc(method, params) {
    const id = ++rpcSequence;
    return await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`RPC timeout ${method}\n${stderr}`)), 15000);
      pending.set(id, value => { clearTimeout(timer); pending.delete(id); resolve(value); });
      ws.send(JSON.stringify({ id, method, params }));
    });
  }
  await rpc('initialize', { clientInfo: { name: 'session-contract-tests', version: '1' }, capabilities: { experimentalApi: true } });
  ws.send(JSON.stringify({ method: 'initialized' }));
  const caps = (await rpc('cpa/capabilities/read', {})).result;
  async function exchange(body, { credential = 'a.json', scope = 'caller/session', baselineDigest = digest(baseline), append = false, parent, managed = true, httpOnly: requestHttpOnly = controlHttpOnly, requestId: explicitRequestId } = {}) {
    const requestId = explicitRequestId ?? `session-${++rpcSequence}`;
    const params = { requestId, credentialId: credential, operation: 'responses', sourceFormat: 'openai-response', sessionId: 'transport-session', request: body };
    if (managed) params.sessionInstructions = { mode: append || parent ? 'reuse' : 'start', scope, baselineDigest,
      ...(requestHttpOnly ? { httpOnly: true } : {}), ...(append ? { frameType: 'response.append' } : {}), ...(parent ? { previousResponseId: parent } : {}) };
    if (append) body.type = 'response.append';
    if (parent) body.previous_response_id = parent;
    return await new Promise((resolve, reject) => {
      const state = { logs: [], body: '', done: false, diagnostics: !caps.diagnosticsCompleted, reply: null, error: null, modelSendsBefore: requests.length, handshakesBefore: connectionSequence };
      const timer = setTimeout(() => reject(new Error(`Inference timeout ${requestId}\n${stderr}`)), 20000);
      state.finish = () => {
        if (!state.reply || !state.done || !state.diagnostics) return;
        if ((state.error ?? state.reply.error?.data)?.httpStatus === 409) {
          assert.equal(requests.length, state.modelSendsBefore, '409 rejects before any model send');
          assert.equal(connectionSequence, state.handshakesBefore, '409 rejects before any new WS handshake');
        }
        clearTimeout(timer); exchanges.delete(requestId); resolve(state);
      };
      exchanges.set(requestId, state);
      void rpc('cpa/inference/start', params).then(reply => {
        state.reply = reply;
        if (reply.error) state.done = true;
        state.finish();
      }, reject);
    });
  }
  return { rpc, exchange, caps, close: () => ws.close() };
}

function input(text, full = false, model = 'gpt-5.2') {
  const message = (role, text) => ({ type: 'message', role, content: [{ type: 'input_text', text }] });
  return { model, instructions: '', input: [...(full ? [message('developer', baseline)] : []), message('user', text)],
    session_id: 'source-session', thread_id: 'source-thread', turn_id: `turn-${++rpcSequence}`, stream: true };
}
function failure(result, code, status) {
  const data = result.error ?? result.reply.error?.data;
  assert(data, JSON.stringify(result));
  assert.equal(data.httpStatus, status);
  assert.equal(JSON.parse(data.body).error.code, code);
  assert.equal(data.nativeError, undefined);
}
function success(result) {
  assert.equal(result.reply.result?.statusCode, 200, JSON.stringify(result));
  assert.equal(result.error, null, JSON.stringify(result));
  const events = result.body.split('\n').filter(line => line.startsWith('data: ')).map(line => JSON.parse(line.slice(6)));
  return events.find(event => event.type === 'response.completed')?.response.id;
}
async function until(predicate) {
  for (let n = 0; n < 300; n++) { if (predicate()) return; await delay(10); }
  throw new Error('mock gate timeout');
}

try {
  await new Promise(resolve => upstream.listen(0, '127.0.0.1', resolve));
  const url = `http://127.0.0.1:${upstream.address().port}`;
  const port = portArg >= 0 ? Number(args[portArg + 1]) : await new Promise(resolve => {
    const server = tcpServer().listen(0, '127.0.0.1', () => { const value = server.address().port; server.close(() => resolve(value)); });
  });
  await mkdir(join(home, 'auths'), { mode: 0o700 });
  await mkdir(join(home, 'state'), { mode: 0o700 });
  const encode = value => Buffer.from(JSON.stringify(value)).toString('base64url');
  for (const credential of ['a.json', 'b.json']) {
    const token = `${encode({ alg: 'none', typ: 'JWT' })}.${encode({ exp: 4102444800, email: 'session-fixture@example.com',
      'https://api.openai.com/auth': { chatgpt_account_id: 'fixture-account', chatgpt_user_id: 'fixture', chatgpt_plan_type: 'plus' } })}.c2lnbmF0dXJl`;
    await writeFile(join(home, 'auths', credential), JSON.stringify({ type: 'codex', id_token: token,
      access_token: `fixture-${credential}`, refresh_token: 'fixture', account_id: 'fixture-account', email: 'session-fixture@example.com',
      last_refresh: new Date().toISOString(), expired: '2099-01-01T00:00:00Z', codex_cli: { enabled: true } }), { mode: 0o600 });
    const state = join(home, 'state', credential); await mkdir(state, { mode: 0o700 });
    await writeFile(join(state, 'config.toml'), `model = "gpt-5.2"\nmodel_provider = "fixture"\napproval_policy = "never"\nsandbox_mode = "read-only"\ncli_auth_credentials_store = "file"\nchatgpt_base_url = "${url}"\n[model_providers.fixture]\nname = "OpenAI"\nbase_url = "${url}/v1"\nwire_api = "responses"\nrequires_openai_auth = true\nsupports_websockets = ${!httpOnly}\nstream_idle_timeout_ms = 5000\n`, { mode: 0o600 });
  }
  child = spawn(binary, [], { env: { ...process.env, CODEX_CPA_AUTH_DIR: join(home, 'auths'), CODEX_HOME: join(home, 'state'), CODEX_CPA_PORT: String(port) }, stdio: ['ignore', 'ignore', 'pipe'] });
  child.stderr.on('data', chunk => { stderr = (stderr + chunk).slice(-200000); });
  let client;
  for (let n = 0; n < 300; n++) {
    try { if ((await fetch(`http://127.0.0.1:${port}/readyz`)).ok) { client = await openClient(port); break; } } catch {}
    if (child.exitCode !== null) throw new Error(`Runtime exited: ${stderr}`);
    await delay(50);
  }
  assert(client, `runtime startup timeout\n${stderr}`);
  assert(['full', 'websocket'].includes(client.caps.sessionInstructions));
  console.log(JSON.stringify({ binary, home, endpoint: `ws://127.0.0.1:${port}/`, upstream: url,
    credentialIds: ['a.json', 'b.json'], capability: client.caps.sessionInstructions, httpOnly, controlHttpOnly }));
  if (serve) {
    client.close();
    await new Promise(resolve => { process.once('SIGINT', resolve); process.once('SIGTERM', resolve); child.once('exit', resolve); });
  } else if (uploadRecovery) {
    assert.equal(client.caps.sessionInstructions, 'websocket');
    for (const matchesParent of [true, false]) {
      const parent = success(await client.exchange(input('seed-upload', true)));
      const before = requests.length;
      const handshakesBefore = connectionSequence;
      const label = matchesParent ? 'recover-upload-current' : 'recover-upload-wrong-parent';
      const recovered = await client.exchange(input(label + 'R'.repeat(12 * 1024 * 1024)), {
        append: true, parent,
      });
      const sent = requests.at(-1);
      assert.equal(requests.length, before + 1, 'partial create is the sole model send');
      assert.equal(connectionSequence, handshakesBefore, 'delta stays on its proven socket');
      assert.equal(sent.partial, true);
      assert.equal(sent.parent, parent);
      if (matchesParent) {
        assert.equal(success(recovered), sent.id, 'correlated completion survives the failed upload');
      } else {
        failure(recovered, 'upstream_websocket_lost', 502);
        assert.equal(recovered.body, '', 'unrelated buffered replies never reach business delivery');
      }
      const requestLogs = recovered.logs.filter(log => log.kind === 'request');
      assert.equal(requestLogs.length, 1);
      assert.equal(requestLogs[0].body, '');
      assert.equal(requestLogs[0].bodyBase64, undefined);
      failure(await client.exchange(input('after-retired-socket'), { append: true, parent: sent.id }),
        'previous_response_not_found', 409);
    }
    await writeFile(join(home, 'mock-requests.json'), JSON.stringify(requests, null, 2), { mode: 0o600 });
    client.close();
    console.log(JSON.stringify({ result: 'passed', capability: client.caps.sessionInstructions,
      cases: 'partial-upload/current-completion/wrong-parent/no-replay/retired-socket-409/privacy',
      modelSends: requests.length, home }));
  } else {
    const native = client.caps.sessionInstructions === 'websocket' && !httpOnly && !controlHttpOnly;
    success(await client.exchange(input('ordinary-route-probe'), { managed: false }));
    const ordinaryTransport = requests.at(-1).transport;
    assert.equal(ordinaryTransport, client.caps.sessionInstructions === 'websocket' && !httpOnly ? 'websocket' : 'http');
    let id = success(await client.exchange(input('first-full', true)));
    assert.equal(id, requests.at(-1).id, 'response ID stays native');
    const first = requests.at(-1);
    assert.equal(first.transport, controlHttpOnly ? 'http' : ordinaryTransport, 'control preserves the route or explicitly requires HTTP');
    assert.equal(first.body.previous_response_id, undefined, 'managed full input is never derived as an implicit delta');
    assert.equal(first.body.input.filter(item => item.role === 'developer' && item.content?.some(part => part.text === baseline)).length, 1, 'full wire input retains the caller baseline exactly once');
    const before = requests.length;
    if (!native) {
      const handshakesBefore = connectionSequence;
      failure(await client.exchange(input('delta'), { append: true }), 'previous_response_not_found', 409);
      assert.equal(requests.length, before);
      assert.equal(connectionSequence, handshakesBefore, 'HTTP delta never causes a WS handshake');
      failure(await client.exchange(input('explicit-parent'), { parent: id }), 'previous_response_not_found', 409);
      if (controlHttpOnly) failure(await client.exchange(input('http-has-no-ws-proof'), { append: true, parent: id, httpOnly: false }), 'previous_response_not_found', 409);
    } else {
      id = success(await client.exchange(input('delta'), { append: true, parent: id }));
      assert.equal(requests.at(-1).body.previous_response_id, first.id);
      assert.equal(requests.at(-1).body.input.length, 1, 'no reconstructed history or baseline in delta');
      assert.equal(requests.at(-1).connection, first.connection);
      failure(await client.exchange(input('other-CPA'), { append: true, scope: 'other-CPA/session' }), 'previous_response_not_found', 409);
      id = success(await client.exchange(input('same-id', true)));
      failure(await client.exchange(input('cold-worker'), { append: true, credential: 'b.json', parent: id }), 'previous_response_not_found', 409);
      assert.equal(success(await client.exchange(input('same-id', true), { credential: 'b.json', scope: 'caller-B/session' })), id);
      failure(await client.exchange(input('wrong-CPA'), { append: true, credential: 'b.json' }), 'previous_response_not_found', 409);
      success(await client.exchange(input('missing-created', true)));
      failure(await client.exchange(input('unproven'), { append: true }), 'previous_response_not_found', 409);
      success(await client.exchange(input('new-full', true)));
      failure(await client.exchange(input('model-change', false, 'gpt-5.1'), { append: true }), 'previous_response_not_found', 409);
      success(await client.exchange(input('new-full', true)));
      failure(await client.exchange(input('digest-change'), { append: true, baselineDigest: digest('other-baseline') }), 'previous_response_not_found', 409);
      success(await client.exchange(input('without-prefix'), { baselineDigest: '' }));
      failure(await client.exchange(input('empty-digest'), { append: true }), 'previous_response_not_found', 409);
      success(await client.exchange(input('wrong-parent', true)));
      failure(await client.exchange(input('after-parent-mismatch'), { append: true }), 'previous_response_not_found', 409);
      success(await client.exchange(input('new-full', true)));
      const beforeLocalLimit = requests.length;
      failure(await client.exchange(input('L'.repeat(131072)), { append: true }), 'upstream_websocket_not_sent', 502);
      assert.equal(requests.length, beforeLocalLimit, 'local frame limit fails before a model send');
      success(await client.exchange(input('new-full', true)));
      const httpBefore = requests.filter(request => request.transport === 'http').length;
      failure(await client.exchange(input('close1009'), { append: true }), 'upstream_websocket_lost', 502);
      assert.equal(requests.filter(request => request.transport === 'http').length, httpBefore, 'delta never falls back to HTTP');
      failure(await client.exchange(input('after-loss'), { append: true }), 'previous_response_not_found', 409);
      const held = client.exchange(input('hold-concurrency', true));
      await until(() => gates.has('hold-concurrency'));
      const second = await openClient(port);
      failure(await second.exchange(input('competing-delta'), { append: true }), 'previous_response_not_found', 409);
      gates.get('hold-concurrency')(); gates.delete('hold-concurrency');
      success(await held);
      failure(await client.exchange(input('after-concurrency'), { append: true }), 'previous_response_not_found', 409);
      second.close();
      const cancelled = client.exchange(input('hold-cancel', true), { requestId: 'cancel-controlled' });
      await until(() => gates.has('hold-cancel'));
      await client.rpc('cpa/inference/cancel', { requestId: 'cancel-controlled' });
      failure(await cancelled, 'upstream_websocket_lost', 502);
      gates.get('hold-cancel')(); gates.delete('hold-cancel');
      failure(await client.exchange(input('after-cancel'), { append: true }), 'previous_response_not_found', 409);
      success(await client.exchange(input('close-after', true)));
      await delay(100);
      failure(await client.exchange(input('after-disconnect'), { append: true }), 'previous_response_not_found', 409);
    }
    if (first.transport === 'http') {
      if (client.caps.sessionInstructions === 'full') {
        const beforeRedirect = requests.length;
        const ordinaryRedirect = await client.exchange(input('redirect-full'), { managed: false });
        const managedRedirect = await client.exchange(input('redirect-full', true));
        assert.equal(ordinaryRedirect.reply.error?.data.httpStatus, 307, 'main exposes redirects on its original HTTP sender');
        assert.equal(managedRedirect.reply.error?.data.httpStatus, 307, 'managed log privacy preserves main redirect behavior');
        assert.equal(requests.length, beforeRedirect, 'main does not add an automatic redirect model send');
      } else {
        success(await client.exchange(input('redirect-full', true)));
        assert.equal(requests.at(-1).transport, 'http', 'managed HTTP preserves native same-origin 307 redirects');
      }
    }
    const protectedResult = await client.exchange(input('private-full', true));
    success(protectedResult);
    assert(!JSON.stringify(protectedResult.logs).includes(baseline), 'managed baseline absent from diagnostics');
    assert(!stderr.includes(baseline), 'managed baseline absent from runtime logs');
    assert(protectedResult.logs.some(log => log.kind === 'request'), 'managed requests retain body-free transport diagnostics');
    for (const log of protectedResult.logs.filter(log => log.kind === 'request')) {
      assert.equal(log.body, ''); assert.equal(log.bodyBase64, undefined);
    }
    success(await client.exchange(input('ordinary'), { managed: false }));
    assert.equal(requests.at(-1).transport, ordinaryTransport, 'ordinary routing remains unchanged after managed HTTP requests');
    client.close();
    console.log(JSON.stringify({ result: 'passed', capability: client.caps.sessionInstructions, modelSends: requests.length,
      ordinaryTransport,
      cases: native ? 'full/delta/IDs/caller-scope/credential/cold-worker/created/model/digest/local-not-sent/1009/concurrency/cancellation/disconnect/privacy/ordinary' : 'full/delta-rejected-before-send/managed-http-redirect/privacy/ordinary', home }));
  }
} finally {
  for (const release of gates.values()) release();
  if (child && child.exitCode === null) { child.kill('SIGTERM'); await Promise.race([new Promise(resolve => child.once('exit', resolve)), delay(2000)]); }
  for (const socket of sockets) socket.destroy();
  await new Promise(resolve => upstream.close(resolve));
  await writeFile(join(home, 'runtime.stderr.log'), stderr, { mode: 0o600 });
  if (!keep) await rm(home, { recursive: true, force: true });
}
