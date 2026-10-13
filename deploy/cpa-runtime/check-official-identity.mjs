// Compare matching official release artifacts on this host. No account is used.
import assert from 'node:assert/strict';
import { execFile, spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { promisify } from 'node:util';

const execute = promisify(execFile);
const [officialCli, officialServer, candidateCli, candidateServer] = process.argv.slice(2);
assert(candidateServer, 'usage: node check-official-identity.mjs OFFICIAL_CODEX OFFICIAL_APP_SERVER CODEX APP_SERVER');
const version = (await readFile(new URL('./release-version', import.meta.url), 'utf8')).trim();
const root = await mkdtemp(join(tmpdir(), 'codex-official-identity-'));
const home = join(root, 'state');
const auth = join(root, 'auth');
await mkdir(home);
await mkdir(auth);
await writeFile(join(home, 'config.toml'), 'cli_auth_credentials_store = "file"\nopenai_base_url = "http://127.0.0.1:1/v1"\nchatgpt_base_url = "http://127.0.0.1:1"\n');
const baseEnv = { PATH: process.env.PATH, HOME: root, CODEX_HOME: home, TERM: 'dumb', RUST_LOG: 'error' };
const clients = [
  { name: 'codex_app_server_daemon', version },
  { name: 'codex-tui', version },
  { name: 'codex-backend', version },
  { name: 'identity-fixture', version: '1', title: 'Caller title' },
  { name: 'invalid\nname', version: '1' },
];

async function unusedPort() {
  const server = createServer().listen(0, '127.0.0.1');
  await once(server, 'listening');
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  return port;
}

async function openSocket(url, child) {
  for (let attempt = 0; attempt < 200; attempt++) {
    assert.equal(child.exitCode, null, 'Server exited before accepting connections');
    const socket = new WebSocket(url);
    try {
      await once(socket, 'open', { signal: AbortSignal.timeout(1000) });
      return socket;
    } catch {
      socket.close();
      await delay(50);
    }
  }
  throw new Error('Server did not accept connections');
}

async function initialize(socket, client, id) {
  const result = new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('initialize timed out')), 10000);
    const receive = event => {
      const message = JSON.parse(event.data);
      if (message.id !== id) return;
      clearTimeout(timer);
      socket.removeEventListener('message', receive);
      resolve(message);
    };
    socket.addEventListener('message', receive);
  });
  socket.send(JSON.stringify({ id, method: 'initialize', params: { clientInfo: client } }));
  return result;
}

async function handshakeSequence(binary, entry, originator) {
  const port = await unusedPort();
  const master = entry === 'master';
  const args = entry === 'cli' ? ['app-server'] : [];
  if (!master) args.push('--listen', `ws://127.0.0.1:${port}`);
  const env = { ...baseEnv };
  if (originator) env.CODEX_INTERNAL_ORIGINATOR_OVERRIDE = originator;
  if (master) Object.assign(env, { CODEX_CPA_AUTH_DIR: auth, CODEX_CPA_PORT: String(port) });
  const child = spawn(binary, args, { env, stdio: ['ignore', 'ignore', 'pipe'] });
  let logs = '';
  child.stderr.on('data', chunk => { logs = (logs + chunk).slice(-4000); });
  const responses = [];
  try {
    for (const client of clients) {
      const socket = await openSocket(`ws://127.0.0.1:${port}${master ? '/cpa/v1/ws' : ''}`, child);
      try {
        responses.push(await initialize(socket, client, 1));
      } finally {
        socket.close();
        await once(socket, 'close');
      }
    }
    return responses;
  } catch (error) {
    error.message += `\n${logs}`;
    throw error;
  } finally {
    child.kill();
    if (child.exitCode === null && child.signalCode === null) await once(child, 'exit');
  }
}

try {
  const versions = {};
  for (const [name, official, candidate] of [
    ['codex-cli', officialCli, candidateCli],
    ['codex-app-server', officialServer, candidateServer],
  ]) {
    const reference = await execute(official, ['--version'], { env: baseEnv });
    const actual = await execute(candidate, ['--version'], { env: baseEnv });
    assert.equal(reference.stdout, `${name} ${version}\n`);
    assert.deepEqual(actual, reference);
    versions[name] = actual.stdout.trimEnd();
  }
  const handshakes = [];
  for (const originator of [undefined, 'codex-tui']) {
    const official = await handshakeSequence(officialServer, 'server', originator);
    for (const [name, binary, entry] of [
      ['official-cli', officialCli, 'cli'],
      ['candidate-cli', candidateCli, 'cli'],
      ['candidate-server', candidateServer, 'server'],
      ['candidate-master', candidateServer, 'master'],
    ]) {
      const actual = await handshakeSequence(binary, entry, originator);
      assert.deepEqual(actual, official, `${name}, originator override=${originator}`);
    }
    handshakes.push({ originatorOverride: originator ?? null, responses: official });
  }
  console.log(JSON.stringify({ versions, matchingHandshakeSequences: 8, handshakes }, null, 2));
} finally {
  await rm(root, { recursive: true, force: true });
}
