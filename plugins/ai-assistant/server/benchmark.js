import assert from 'node:assert/strict';
import { setTimeout as delay } from 'node:timers/promises';
import { performance } from 'node:perf_hooks';
import { createServer } from './server.js';
import { readConfig } from './config.js';

const config = { ...readConfig({ AI_ENABLED: 'false' }), enabled: true, debounce: 0,
  timeout: 1000, streaming: true, cacheEnabled: true };
const ai = { models: { async generateContentStream() {
  return (async function* () {
    await delay(5);
    yield { text: '42', usageMetadata: { candidatesTokenCount: 1 } };
  })();
} } };
const server = createServer(config, ai);
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const base = `http://127.0.0.1:${server.address().port}`;
const input = i => ({ path: 'C:\\work\\benchmark.rs', language: 'rust', version: i + 1,
  position: { line: 0, character: 0 }, before: `let item_${i} = `,
  after: ';', context: 'fn main() {}' });
const durations = [];
const percentile = (values, p) => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.ceil(sorted.length * p) - 1].toFixed(2);
};
const post = async body => {
  const start = performance.now();
  const response = await fetch(`${base}/v1/completion`, {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body),
  });
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { insertText: '42' });
  return performance.now() - start;
};

try {
  const warm = input(0);
  await post(warm);
  for (let i = 0; i < 100; i++) durations.push(await post(warm));
  const cold = [];
  for (let i = 1; i <= 20; i++) cold.push(await post(input(i)));
  const metrics = await (await fetch(`${base}/metrics`)).text();
  const metric = name => Number(metrics.match(new RegExp(`^${name} ([\\d.]+)$`, 'm'))?.[1]);
  assert.equal(metric('completion_cache_hits'), 100);
  assert.equal(metric('completion_output_tokens_total'), 21);
  console.log(`Mock loopback warm cache, 100 requests: p50 ${percentile(durations, .5)} ms, p95 ${percentile(durations, .95)} ms`);
  console.log(`Mock loopback cold, 20 requests with 5 ms first chunk: p50 ${percentile(cold, .5)} ms, p95 ${percentile(cold, .95)} ms`);
  console.log(`Mock upstream first token mean: ${(metric('completion_first_token_ms_total') / metric('completion_first_token_count')).toFixed(2)} ms`);
  console.log(`Mock upstream total mean: ${(metric('completion_total_ms_total') / metric('completion_upstream_calls')).toFixed(2)} ms`);
} finally {
  await new Promise(resolve => server.close(resolve));
}
