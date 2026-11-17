import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createServer } from './server.js';
import { buildPrompt, cleanCompletion } from './completion.js';
import { readConfig } from './config.js';

const input = (overrides = {}) => ({
  path: 'C:\\work\\example.rs', language: 'rust', version: 1,
  position: { line: 1, character: 4 }, before: 'let ', after: ';',
  context: 'use std::fmt;', ...overrides,
});
const config = (overrides = {}) => ({ ...readConfig({ AI_ENABLED: 'false' }),
  enabled: true, debounce: 0, ...overrides });

async function fixture(ai, options = {}) {
  const server = createServer(config(options), ai);
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  return {
    base,
    post: (body = input(), init = {}) => fetch(`${base}/v1/completion`, {
      method: 'POST', headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body), ...init,
    }),
    close: () => new Promise(resolve => server.close(resolve)),
  };
}

test('configuration rejects invalid required values before serving', () => {
  assert.throws(() => readConfig({ AI_ENABLED: 'true' }), /GEMINI_API_KEY/);
  assert.throws(() => readConfig({ AI_ENABLED: 'false', AI_COMPLETION_MAX_TOKENS: '0' }), /AI_COMPLETION_MAX_TOKENS/);
});

test('prompt keeps caret and nearby Unicode suffix within a byte budget', () => {
  const prompt = buildPrompt(input({ before: 'a'.repeat(100) + '界', after: '🦀tail', context: 'imports' }), 70);
  const parsed = JSON.parse(prompt);
  assert.equal(parsed.file.path, 'C:\\work\\example.rs');
  assert.deepEqual(parsed.caret, { line: 1, character: 4 });
  assert.ok(Buffer.byteLength(parsed.source.before + parsed.source.after + parsed.source.context, 'utf8') <= 70);
  assert.ok(parsed.source.before.endsWith('界'));
  assert.ok(parsed.source.after.startsWith('🦀'));
  assert.ok(parsed.source.context.includes('imports'));
});

test('completion rejects fences and prose, removes overlap, and preserves semantic whitespace', () => {
  assert.equal(cleanCompletion('```rust\n    foo();\n```', input()), '');
  assert.equal(cleanCompletion('Here is the code: foo()', input()), '');
  assert.equal(cleanCompletion('let value;', input()), 'value');
  assert.equal(cleanCompletion('    return x\n', input({ language: 'python', before: 'def f():\n', after: 'next=1' })), '    return x\n');
  assert.equal(cleanCompletion('    x\n\ty', input({ language: 'python' })), '');
  assert.equal(cleanCompletion(' \tx', input({ language: 'python' })), '');
});

test('request rejects oversized path and language instead of prompt failure', async () => {
  const f = await fixture({ models: { generateContent: () => { throw Error('called'); } } });
  try {
    assert.equal((await f.post(input({ path: `C:\\${'x'.repeat(4100)}` }))).status, 400);
    assert.equal((await f.post(input({ language: 'x'.repeat(65) }))).status, 400);
  } finally { await f.close(); }
});

test('HTTP validates input, origin and route; disabled mode does not call AI', async () => {
  const f = await fixture({ models: { generateContent: () => { throw Error('called'); } } }, { enabled: false });
  try {
    assert.deepEqual(await (await f.post()).json(), { insertText: '' });
    assert.equal((await f.post(input({ position: { line: -1, character: 0 } }))).status, 400);
    assert.equal((await f.post(input(), { headers: { origin: 'http://evil.test' } })).status, 403);
    assert.equal((await fetch(`${f.base}/bad`)).status, 404);
    assert.equal((await fetch(`${f.base}/health`)).status, 200);
  } finally { await f.close(); }
});

test('streaming response is validated, cached, and counted without source', async () => {
  let calls = 0;
  const ai = { models: { async generateContentStream() {
    calls++;
    return (async function* () { yield { text: 'value', usageMetadata: { candidatesTokenCount: 2 } }; })();
  } } };
  const f = await fixture(ai);
  try {
    assert.deepEqual(await (await f.post()).json(), { insertText: 'value' });
    assert.deepEqual(await (await f.post()).json(), { insertText: 'value' });
    assert.equal(calls, 1);
    const metrics = await (await fetch(`${f.base}/metrics`)).text();
    assert.match(metrics, /completion_cache_hits 1/);
    assert.ok(!metrics.includes('std::fmt'));
  } finally { await f.close(); }
});

test('newer version cancels pending request and stale result cannot enter cache', async () => {
  let resolveOld;
  let calls = 0;
  const ai = { models: { generateContent: ({ contents }) => contents.includes('old')
    ? new Promise(resolve => { calls++; resolveOld = resolve; })
    : Promise.resolve({ text: 'fresh' }) } };
  const f = await fixture(ai, { streaming: false });
  try {
    const old = f.post(input({ before: 'old', version: 1 }));
    for (let i = 0; i < 100 && !resolveOld; i++) await new Promise(resolve => setTimeout(resolve, 1));
    assert.ok(resolveOld, 'old request did not reach AI');
    assert.deepEqual(await (await f.post(input({ before: 'new', version: 2 }))).json(), { insertText: 'fresh' });
    resolveOld({ text: 'stale' });
    assert.equal((await old).status, 409);
    const third = f.post(input({ before: 'old', version: 3 }));
    for (let i = 0; i < 100 && calls < 2; i++) await new Promise(resolve => setTimeout(resolve, 1));
    assert.equal(calls, 2);
    resolveOld({ text: 'later' });
    assert.deepEqual(await (await third).json(), { insertText: 'later' });
  } finally { resolveOld?.({ text: 'stale' }); await f.close(); }
});

test('timeout and upstream rate limit are sanitized', async () => {
  const f = await fixture({ models: { generateContent: () => new Promise(() => {}) } },
    { streaming: false, timeout: 15 });
  try { assert.equal((await f.post()).status, 504); }
  finally { await f.close(); }
  const g = await fixture({ models: { generateContent: async () => { throw { status: 429, message: 'secret' }; } } },
    { streaming: false });
  try {
    const response = await g.post();
    assert.equal(response.status, 429);
    assert.ok(!(await response.text()).includes('secret'));
  } finally { await g.close(); }
});

test('oversized input and full upstream capacity stop before another AI call', async () => {
  const releases = [];
  const f = await fixture({ models: { generateContent: () => new Promise(resolve => releases.push(resolve)) } },
    { streaming: false, timeout: 1000 });
  try {
    assert.equal((await f.post(input({ before: 'x'.repeat(100000) }))).status, 413);
    const pending = Array.from({ length: 4 }, (_, i) => f.post(input({ path: `C:\\work\\${i}.rs` })));
    for (let i = 0; i < 100 && releases.length < 4; i++) await new Promise(resolve => setTimeout(resolve, 1));
    assert.equal(releases.length, 4);
    assert.equal((await f.post(input({ path: 'C:\\work\\overflow.rs' }))).status, 429);
    releases.forEach(resolve => resolve({ text: '' }));
    for (const response of await Promise.all(pending)) assert.equal(response.status, 200);
  } finally { releases.forEach(resolve => resolve({ text: '' })); await f.close(); }
});

test('model prose and an empty model response produce an empty insertion', async () => {
  let calls = 0;
  const f = await fixture({ models: { generateContent: async () => ({
    text: calls++ ? '' : 'Here is the code: should_not_insert()',
  }) } }, { streaming: false, cacheEnabled: false });
  try {
    assert.deepEqual(await (await f.post()).json(), { insertText: '' });
    assert.deepEqual(await (await f.post()).json(), { insertText: '' });
    assert.equal(calls, 2);
  } finally { await f.close(); }
});

test('client disconnect aborts upstream and does not cache its late answer', async () => {
  let signal, resolveModel, calls = 0;
  const f = await fixture({ models: { generateContent: ({ config }) => {
    calls++;
    signal = config.abortSignal;
    return new Promise(resolve => { resolveModel = resolve; });
  } } }, { streaming: false });
  try {
    const abort = new AbortController();
    const first = f.post(input(), { signal: abort.signal });
    for (let i = 0; i < 100 && !signal; i++) await new Promise(resolve => setTimeout(resolve, 1));
    assert.ok(signal);
    abort.abort();
    await assert.rejects(first);
    for (let i = 0; i < 100 && !signal.aborted; i++) await new Promise(resolve => setTimeout(resolve, 1));
    assert.equal(signal.aborted, true);
    resolveModel({ text: 'stale' });
    const second = f.post(input({ version: 2 }));
    for (let i = 0; i < 100 && calls < 2; i++) await new Promise(resolve => setTimeout(resolve, 1));
    assert.equal(calls, 2);
    resolveModel({ text: 'fresh' });
    assert.deepEqual(await (await second).json(), { insertText: 'fresh' });
  } finally { resolveModel?.({ text: '' }); await f.close(); }
});

test('at full capacity, newer same-file request replaces old and duplicate request does not restart it', async () => {
  const releases = [];
  let calls = 0;
  const f = await fixture({ models: { generateContent: () => {
    calls++;
    return new Promise(resolve => releases.push(resolve));
  } } }, { streaming: false, timeout: 1000 });
  try {
    const first = f.post(input({ path: 'C:\\work\\same.rs', version: 1, before: 'old' }));
    const others = [1, 2, 3].map(i => f.post(input({ path: `C:\\work\\other${i}.rs` })));
    for (let i = 0; i < 100 && calls < 4; i++) await new Promise(resolve => setTimeout(resolve, 1));
    assert.equal(calls, 4);
    const second = f.post(input({ path: 'C:\\work\\same.rs', version: 2, before: 'new' }));
    for (let i = 0; i < 100 && calls < 5; i++) await new Promise(resolve => setTimeout(resolve, 1));
    assert.equal(calls, 5);
    assert.equal((await first).status, 409);
    assert.equal((await f.post(input({ path: 'C:\\work\\same.rs', version: 2, before: 'new' }))).status, 409);
    assert.equal(calls, 5);
    releases.forEach(resolve => resolve({ text: '42', usageMetadata: { candidatesTokenCount: 1, totalTokenCount: 3 } }));
    assert.equal((await second).status, 200);
    for (const response of await Promise.all(others)) assert.equal(response.status, 200);
    const metrics = await (await fetch(`${f.base}/metrics`)).text();
    assert.match(metrics, /completion_cancellations 1/);
    assert.match(metrics, /completion_upstream_calls 5/);
    assert.match(metrics, /completion_valid_generated 4/);
    assert.match(metrics, /completion_total_tokens_total 12/);
  } finally { releases.forEach(resolve => resolve({ text: '' })); await f.close(); }
});

test('A and B late results cannot overwrite C or enter cache', async () => {
  const releases = [];
  const f = await fixture({ models: { generateContent: () => new Promise(resolve => releases.push(resolve)) } },
    { streaming: false, timeout: 1000 });
  const request = (version, before) => f.post(input({ version, before }));
  try {
    const a = request(1, 'first');
    for (let i = 0; i < 100 && releases.length < 1; i++) await new Promise(resolve => setTimeout(resolve, 1));
    const b = request(2, 'second');
    for (let i = 0; i < 100 && releases.length < 2; i++) await new Promise(resolve => setTimeout(resolve, 1));
    const c = request(3, 'third');
    for (let i = 0; i < 100 && releases.length < 3; i++) await new Promise(resolve => setTimeout(resolve, 1));
    assert.equal(releases.length, 3);
    releases[2]({ text: 'current' });
    assert.equal((await a).status, 409);
    assert.equal((await b).status, 409);
    assert.deepEqual(await (await c).json(), { insertText: 'current' });
    releases[0]({ text: 'staleA' });
    releases[1]({ text: 'staleB' });
    const fresh = request(4, 'first');
    for (let i = 0; i < 100 && releases.length < 4; i++) await new Promise(resolve => setTimeout(resolve, 1));
    assert.equal(releases.length, 4);
    releases[3]({ text: 'fresh' });
    assert.deepEqual(await (await fresh).json(), { insertText: 'fresh' });
  } finally { releases.forEach(resolve => resolve({ text: '' })); await f.close(); }
});
