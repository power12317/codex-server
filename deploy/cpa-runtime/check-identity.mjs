// Exercise the published master and worker against a local upstream with fake
// credentials. No OpenAI request or local user credential is needed.
import assert from 'node:assert/strict';
import { spawn, execFile } from 'node:child_process';
import { once } from 'node:events';
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
model_provider = "openai"
approval_policy = "never"
sandbox_mode = "read-only"
cli_auth_credentials_store = "file"
openai_base_url = "${url}/v1"
chatgpt_base_url = "${url}"
`);
  const root = binary ? fixture : '/fixture';
  const environment = {
    CODEX_CPA_AUTH_DIR: `${root}/auths`, CODEX_HOME: `${root}/state`, CODEX_CPA_PORT: String(port),
    CODEX_APP_SERVER_LOGIN_ISSUER: url, CODEX_REFRESH_TOKEN_URL_OVERRIDE: `${url}/oauth/token`,
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
  socket = new WebSocket(`ws://127.0.0.1:${port}/cpa/v1/ws`);
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
  assert.equal(headers.originator, 'codex-tui');
  assert.equal(headers.version, version);
  assert(headers['user-agent'].startsWith(`codex-tui/${version} (`), headers['user-agent']);
  assert(headers['user-agent'].endsWith(`(codex-tui; ${version})`), headers['user-agent']);
  console.log(JSON.stringify({ executableVersion: versionResult.stdout.trimEnd(), initializeUserAgent: initialized.userAgent, originator: headers.originator, version: headers.version, userAgent: headers['user-agent'] }));
} catch (error) {
  console.error(logs);
  throw error;
} finally {
  socket?.close();
  if (binary) child?.kill();
  else await execute('docker', ['rm', '-f', name]).catch(() => {});
  upstream.closeAllConnections();
  await new Promise(resolve => upstream.close(resolve));
  await rm(fixture, { recursive: true, force: true });
}
