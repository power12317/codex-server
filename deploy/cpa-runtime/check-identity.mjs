// Exercise the published master and worker against a local upstream with fake
// credentials. No OpenAI request or local user credential is needed.
import assert from 'node:assert/strict';
import { spawn, execFile } from 'node:child_process';
import { once } from 'node:events';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { createServer as createTcpServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { promisify } from 'node:util';

const execute = promisify(execFile);
const image = process.argv[2];
const binary = process.env.CODEX_TEST_BINARY;
assert(image || binary, 'usage: node check-identity.mjs IMAGE');
const version = (await readFile(new URL('./release-version', import.meta.url), 'utf8')).trim();
const desktopVersion = (await readFile(new URL('./desktop-version', import.meta.url), 'utf8')).trim();
const fixture = await mkdtemp(join(tmpdir(), 'codex-identity-'));
const name = `codex-identity-${process.pid}`;
const requests = [];
const upstream = createServer((request, response) => {
  request.resume();
  if (request.method === 'POST' && request.url === '/v1/responses') {
    requests.push(request.headers);
    response.writeHead(200, { 'content-type': 'text/event-stream' });
    response.end('data: {"type":"response.completed","response":{"id":"identity-check","output":[],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}\n\n');
  } else {
    response.writeHead(404, { 'content-type': 'application/json' });
    response.end('{}');
  }
});
// The built-in OpenAI provider uses WebSockets. The identity fixture supports
// that native transport directly; it must not rely on error-driven HTTP fallback.
const upstreamSockets = new Set();
function serverFrame(payload, opcode = 1) {
  const body = Buffer.from(payload);
  const header = Buffer.alloc(body.length < 126 ? 2 : 4);
  header[0] = 0x80 | opcode;
  if (body.length < 126) header[1] = body.length;
  else { header[1] = 126; header.writeUInt16BE(body.length, 2); }
  return Buffer.concat([header, body]);
}
upstream.on('upgrade', (request, peer, head) => {
  const accept = createHash('sha1').update(request.headers['sec-websocket-key'] + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64');
  peer.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
  upstreamSockets.add(peer);
  peer.on('close', () => upstreamSockets.delete(peer));
  peer.on('error', () => {});
  let pending = head;
  const consume = () => {
    while (pending.length >= 2) {
      const opcode = pending[0] & 15;
      const masked = (pending[1] & 128) !== 0;
      let length = pending[1] & 127;
      let offset = 2;
      if (length === 126) {
        if (pending.length < 4) return;
        length = pending.readUInt16BE(2); offset = 4;
      } else if (length === 127) {
        if (pending.length < 10) return;
        length = Number(pending.readBigUInt64BE(2)); offset = 10;
      }
      const maskBytes = masked ? 4 : 0;
      if (pending.length < offset + maskBytes + length) return;
      const mask = pending.subarray(offset, offset + maskBytes);
      const body = Buffer.from(pending.subarray(offset + maskBytes, offset + maskBytes + length));
      pending = pending.subarray(offset + maskBytes + length);
      if (masked) for (let i = 0; i < body.length; i++) body[i] ^= mask[i % 4];
      if (opcode === 8) { peer.end(serverFrame(body, 8)); return; }
      if (opcode === 9) { peer.write(serverFrame(body, 10)); continue; }
      if (opcode !== 1) continue;
      const message = JSON.parse(body.toString('utf8'));
      assert.equal(message.type, 'response.create');
      requests.push(request.headers);
      peer.write(serverFrame(JSON.stringify({ type: 'response.completed', response: {
        id: 'identity-check', output: [], usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 },
      } })));
    }
  };
  peer.on('data', chunk => { pending = Buffer.concat([pending, chunk]); consume(); });
  consume();
});
let child;
let socket;
let logs = '';
try {
  const versionResult = binary
    ? await execute(binary, ['--version'])
    : await execute('docker', ['run', '--rm', image, '--version']);
  assert.equal(versionResult.stdout, `codex-app-server ${version}\n`);
  upstream.listen(0, '127.0.0.1');
  await once(upstream, 'listening');
  const url = `http://127.0.0.1:${upstream.address().port}`;
  const reservation = createTcpServer().listen(0, '127.0.0.1');
  await once(reservation, 'listening');
  const port = reservation.address().port;
  await new Promise(resolve => reservation.close(resolve));
  await mkdir(join(fixture, 'auths'));
  await mkdir(join(fixture, 'state', 'identity.json'), { recursive: true });
  const encode = value => Buffer.from(JSON.stringify(value)).toString('base64url');
  const token = `${encode({ alg: 'none', typ: 'JWT' })}.${encode({
    email: 'fixture@example.com',
    'https://api.openai.com/auth': { chatgpt_account_id: 'identity-test', chatgpt_user_id: 'fixture', chatgpt_plan_type: 'plus' },
  })}.c2lnbmF0dXJl`;
  await writeFile(join(fixture, 'auths', 'identity.json'), JSON.stringify({
    type: 'codex', id_token: token, access_token: 'fake-release-check',
    refresh_token: 'fake-release-check', account_id: 'identity-test', email: 'fixture@example.com',
    last_refresh: new Date().toISOString(), expired: '2099-01-01T00:00:00Z', codex_cli: { enabled: true },
  }));
  await writeFile(join(fixture, 'state', 'identity.json', 'config.toml'), `
model = "gpt-6.1-sol"
model_provider = "identity_fixture"
approval_policy = "never"
sandbox_mode = "read-only"
cli_auth_credentials_store = "file"
chatgpt_base_url = "${url}"
[model_providers.identity_fixture]
name = "OpenAI"
base_url = "${url}/v1"
wire_api = "responses"
requires_openai_auth = true
supports_websockets = true
[model_providers.identity_fixture.http_headers]
User-Agent = "old-cli-override"
ORIGINATOR = "old-cli-override"
Version = "0.1.0"
X-OAI-ATTESTATION = "provider-proof"
X-Custom = "preserve-me"
[model_providers.identity_fixture.env_http_headers]
user-AGENT = "CODEX_IDENTITY_CHECK_STALE_HEADER"
Originator = "CODEX_IDENTITY_CHECK_STALE_HEADER"
VERSION = "CODEX_IDENTITY_CHECK_STALE_HEADER"
x-oai-attestation = "CODEX_IDENTITY_CHECK_STALE_HEADER"
`);
  await mkdir(join(fixture, 'state', 'windows.json'), { recursive: true });
  const windowsCredential = JSON.parse(await readFile(join(fixture, 'auths', 'identity.json'), 'utf8'));
  windowsCredential.codex_client_system = 'windows';
  await writeFile(join(fixture, 'auths', 'windows.json'), JSON.stringify(windowsCredential));
  await writeFile(join(fixture, 'state', 'windows.json', 'config.toml'), await readFile(join(fixture, 'state', 'identity.json', 'config.toml')));
  const root = binary ? fixture : '/fixture';
  const environment = {
    CODEX_CPA_AUTH_DIR: `${root}/auths`, CODEX_HOME: `${root}/state`, CODEX_CPA_PORT: String(port),
    CODEX_APP_SERVER_LOGIN_ISSUER: url, CODEX_REFRESH_TOKEN_URL_OVERRIDE: `${url}/oauth/token`,
    CODEX_IDENTITY_CHECK_STALE_HEADER: 'old-cli-env-override',
  };
  const childEnv = { ...process.env, ...environment };
  delete childEnv.CODEX_CPA_AUTH_FILE;
  delete childEnv.CODEX_INTERNAL_ORIGINATOR_OVERRIDE;
  const args = ['run', '--rm', '--name', name, '--network', 'host',
    '--user', `${process.getuid()}:${process.getgid()}`, '-v', `${fixture}:/fixture`,
    ...Object.entries(environment).flatMap(([key, value]) => ['-e', `${key}=${value}`]), image];
  child = spawn(binary || 'docker', binary ? [] : args, { env: childEnv, stdio: ['ignore', 'pipe', 'pipe'] });
  for (const stream of [child.stdout, child.stderr]) {
    stream.on('data', chunk => { logs = (logs + chunk).slice(-12000); });
  }
  let ready = false;
  for (let attempt = 0; attempt < 120; attempt++) {
    if (child.exitCode !== null) throw new Error(`Runtime exited: ${child.exitCode}`);
    try {
      ready = (await fetch(`http://127.0.0.1:${port}/readyz`, { signal: AbortSignal.timeout(1000) })).ok;
    } catch { /* Wait for startup. */ }
    if (ready) break;
    await delay(250);
  }
  assert(ready, 'Runtime did not become ready');
  socket = new WebSocket(`ws://127.0.0.1:${port}/`);
  const replies = new Map();
  socket.addEventListener('message', event => {
    const message = JSON.parse(event.data);
    if (message.id !== undefined) replies.set(message.id, message);
  });
  await once(socket, 'open');
  async function rpc(id, method, params) {
    socket.send(JSON.stringify({ id, method, params }));
    for (let attempt = 0; attempt < 120; attempt++) {
      if (replies.has(id)) {
        const reply = replies.get(id);
        assert(!reply.error, JSON.stringify(reply.error));
        return reply.result;
      }
      await delay(250);
    }
    throw new Error(`No reply to ${method}`);
  }
  const initialized = await rpc(1, 'initialize', { clientInfo: { name: 'release-check', version: '1' }, capabilities: { experimentalApi: true } });
  assert(initialized.userAgent.startsWith(`release-check/${version} (`), initialized.userAgent);
  assert(initialized.userAgent.endsWith('(release-check; 1)'), initialized.userAgent);
  assert.equal(initialized.serverInfo, undefined);
  socket.send(JSON.stringify({ method: 'initialized' }));
  await rpc(2, 'cpa/inference/start', {
    requestId: 'identity-check', credentialId: 'identity.json', operation: 'responses',
    sourceFormat: 'openai-response', sessionId: 'identity-check',
    request: { model: 'gpt-6.1-sol', stream: true, instructions: '', input: [], tools: [] },
  });
  assert.equal(requests.length, 1, 'Expected exactly one local upstream inference request');
  const headers = requests[0];
  assert.equal(headers.originator, 'Codex Desktop');
  assert.equal(headers.version, version);
  assert.equal(headers['x-custom'], 'preserve-me');
  assert.equal(headers['user-agent'], `Codex Desktop/${version} (Mac OS 15.3.1; x86_64) dumb (Codex Desktop; ${desktopVersion})`);
  const envelope = JSON.parse(headers['x-oai-attestation']);
  assert.equal(envelope.v, 1); assert.equal(envelope.s, 0);
  assert(headers['x-oai-attestation'].startsWith('{"v":1,"s":0,"t":"v1.'));
  const inner = Buffer.from(envelope.t.slice(3), 'base64url');
  assert(inner.subarray(0, 13).equals(Buffer.from([0xa3, 0x6a, ...Buffer.from('error_code'), 2])));

  await rpc(3, 'cpa/inference/start', {
    requestId: 'windows-identity-check', credentialId: 'windows.json', operation: 'responses',
    sourceFormat: 'openai-response', sessionId: 'identity-check',
    request: { model: 'gpt-6.1-sol', stream: true, input: [], tools: [],
      headers: { 'user-agent': 'caller-cli', 'x-oai-attestation': 'caller-proof' } },
  });
  assert.equal(requests.length, 2, 'One model send per worker');
  const windowsHeaders = requests[1];
  assert.equal(windowsHeaders.originator, 'Codex Desktop');
  assert.equal(windowsHeaders.version, version);
  assert.equal(windowsHeaders['x-custom'], 'preserve-me');
  assert.equal(windowsHeaders['user-agent'], `Codex Desktop/${version} (Windows 10.0.19044; x86_64) dumb (Codex Desktop; ${desktopVersion})`);
  const windowsEnvelope = JSON.parse(windowsHeaders['x-oai-attestation']);
  assert.equal(windowsEnvelope.v, 1); assert.equal(windowsEnvelope.s, 0);
  const windowsInner = Buffer.from(windowsEnvelope.t.slice(3), 'base64url');
  assert(windowsInner.subarray(0, 13).equals(Buffer.from([0xa3, 0x6a, ...Buffer.from('error_code'), 1])));
  assert.notEqual(windowsEnvelope.t, envelope.t);
  console.log(JSON.stringify({ executableVersion: versionResult.stdout.trimEnd(), initializeUserAgent: initialized.userAgent,
    mac: { originator: headers.originator, version: headers.version, userAgent: headers['user-agent'], custom: headers['x-custom'], attestation: envelope },
    windows: { originator: windowsHeaders.originator, version: windowsHeaders.version, userAgent: windowsHeaders['user-agent'], custom: windowsHeaders['x-custom'], attestation: windowsEnvelope } }));
} catch (error) {
  console.error(logs);
  throw error;
} finally {
  socket?.close();
  if (binary) child?.kill();
  else await execute('docker', ['rm', '-f', name]).catch(() => {});
  for (const peer of upstreamSockets) peer.destroy();
  upstream.closeAllConnections();
  await new Promise(resolve => upstream.close(resolve));
  await rm(fixture, { recursive: true, force: true });
}
