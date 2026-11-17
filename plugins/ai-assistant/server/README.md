# AI code completion companion

Requires Node.js 22 or newer. From this directory:

```sh
npm ci
```

Set `AI_ENABLED=true` and `GEMINI_API_KEY` in the server process environment, then run `npm start`. The server listens only on `127.0.0.1:8765` by default. Set `AI_COMPLETION_PORT` to change the port. Keep `GEMINI_API_KEY` out of editor settings and completion requests.

The editor sends `POST /v1/completion` with JSON fields `path`, `language`, `version`, `position: {line, character}`, `before`, `after`, and `context`. The response is `{ "insertText": "..." }`. `GET /health` and `GET /metrics` expose status and aggregate counters without code or credentials. Requests with a browser `Origin` are rejected.

Settings use these environment variables: `GEMINI_MODEL` (default `gemini-3.8-flash`), `AI_COMPLETION_DEBOUNCE` (300 ms), `AI_COMPLETION_MAX_TOKENS` (256), `AI_COMPLETION_TEMPERATURE` (0.15), `AI_COMPLETION_THINKING_LEVEL` (`LOW`), `AI_COMPLETION_CONTEXT_LIMIT` (24000 UTF-8 bytes of source context), `AI_COMPLETION_CACHE_ENABLED` (`true`), `AI_COMPLETION_STREAMING` (`true`), `AI_COMPLETION_TIMEOUT` (10000 ms), and `AI_COMPLETION_CACHE_SIZE` (128). The server validates these before listening. `AI_ENABLED=false` returns an empty insertion without contacting Gemini. Automatic and manual triggering are controlled by the native editor; the completion request has no trigger-reason field.

See [.env.example](.env.example) for every supported setting. The plugin's `completionRequestTimeoutMs` defaults to 12 seconds; keep it greater than `AI_COMPLETION_DEBOUNCE + AI_COMPLETION_TIMEOUT` so the service can return its timeout response before the editor stops waiting. The default service timeout is 10000 ms.

Run `npm test` for the local request, cancellation, validation, and cache tests. Gemini usage requires a valid key and model; the tests do not send code to Gemini.

Run `npm run benchmark` for a deterministic loopback benchmark. It reports warm-cache HTTP latency, cold HTTP latency with a mock stream delayed 5 ms, and the mock upstream first-token and total timing counters. These measurements are not Gemini or editor/LSP latency.
