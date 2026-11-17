const defaults = {
  AI_ENABLED: 'false', GEMINI_MODEL: 'gemini-3.8-flash',
  AI_COMPLETION_DEBOUNCE: '300', AI_COMPLETION_MAX_TOKENS: '256',
  AI_COMPLETION_TEMPERATURE: '0.15', AI_COMPLETION_THINKING_LEVEL: 'LOW',
  AI_COMPLETION_CONTEXT_LIMIT: '24000', AI_COMPLETION_CACHE_ENABLED: 'true',
  AI_COMPLETION_STREAMING: 'true',
  AI_COMPLETION_TIMEOUT: '10000', AI_COMPLETION_CACHE_SIZE: '128',
  AI_COMPLETION_PORT: '8765',
};

export function readConfig(env) {
  const value = name => env[name] ?? defaults[name];
  const bool = name => {
    const raw = value(name);
    if (raw !== 'true' && raw !== 'false') throw Error(`${name} must be true or false`);
    return raw === 'true';
  };
  const integer = (name, min, max) => {
    const raw = value(name);
    if (!/^\d+$/.test(raw) || Number(raw) < min || Number(raw) > max)
      throw Error(`${name} must be an integer from ${min} to ${max}`);
    return Number(raw);
  };
  const enabled = bool('AI_ENABLED');
  if (enabled && !env.GEMINI_API_KEY?.trim()) throw Error('GEMINI_API_KEY is required when AI_ENABLED=true');
  const model = value('GEMINI_MODEL');
  if (!model.trim()) throw Error('GEMINI_MODEL is required');
  const rawTemperature = value('AI_COMPLETION_TEMPERATURE');
  const temperature = Number(rawTemperature);
  if (!rawTemperature.trim() || !Number.isFinite(temperature) || temperature < 0 || temperature > 2)
    throw Error('AI_COMPLETION_TEMPERATURE must be between 0 and 2');
  const thinkingLevel = value('AI_COMPLETION_THINKING_LEVEL');
  if (!['MINIMAL', 'LOW', 'MEDIUM', 'HIGH'].includes(thinkingLevel))
    throw Error('AI_COMPLETION_THINKING_LEVEL is invalid');
  return {
    enabled, apiKey: env.GEMINI_API_KEY, model,
    debounce: integer('AI_COMPLETION_DEBOUNCE', 0, 60000),
    maxTokens: integer('AI_COMPLETION_MAX_TOKENS', 1, 8192),
    temperature, thinkingLevel,
    contextLimit: integer('AI_COMPLETION_CONTEXT_LIMIT', 128, 1000000),
    cacheEnabled: bool('AI_COMPLETION_CACHE_ENABLED'),
    streaming: bool('AI_COMPLETION_STREAMING'),
    timeout: integer('AI_COMPLETION_TIMEOUT', 1, 120000),
    cacheSize: integer('AI_COMPLETION_CACHE_SIZE', 1, 10000),
    port: integer('AI_COMPLETION_PORT', 1, 65535),
  };
}
