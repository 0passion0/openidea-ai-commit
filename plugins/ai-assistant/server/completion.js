const encoder = new TextEncoder();
const bytes = value => encoder.encode(value).length;

export function validateInput(value) {
  const integer = number => Number.isSafeInteger(number) && number >= 0;
  if (!value || typeof value !== 'object' || Array.isArray(value) ||
      typeof value.path !== 'string' || !value.path ||
      !/^(?:[A-Za-z]:[\\/]|\/|\\\\[^\\/]+[\\/][^\\/]+)/.test(value.path) ||
      bytes(value.path) > 4096 ||
      typeof value.language !== 'string' || !value.language || bytes(value.language) > 64 ||
      !integer(value.version) ||
      !value.position || typeof value.position !== 'object' ||
      !integer(value.position.line) || !integer(value.position.character) ||
      typeof value.before !== 'string' || typeof value.after !== 'string' ||
      typeof value.context !== 'string') return false;
  return true;
}

export function buildPrompt(input, limit) {
  const contextParts = [];
  let remaining = limit;
  const contextBudget = Math.floor(limit / 5);
  let contextBytes = 0;
  for (const char of input.context) {
    const size = bytes(char);
    if (contextBytes + size > contextBudget) break;
    contextParts.push(char); contextBytes += size; remaining -= size;
  }
  const prefix = Array.from(input.before);
  const suffix = Array.from(input.after);
  const left = [], right = [];
  let i = prefix.length - 1, j = 0;
  while (i >= 0 || j < suffix.length) {
    let added = false;
    if (i >= 0) {
      const size = bytes(prefix[i]);
      if (size <= remaining) { left.push(prefix[i--]); remaining -= size; added = true; }
    }
    if (j < suffix.length) {
      const size = bytes(suffix[j]);
      if (size <= remaining) { right.push(suffix[j++]); remaining -= size; added = true; }
    }
    if (!added) break;
  }
  return JSON.stringify({
    file: { path: input.path, language: input.language },
    caret: input.position,
    source: { before: left.reverse().join(''), context: contextParts.join(''), after: right.join('') },
  });
}

export function cleanCompletion(raw, input) {
  if (typeof raw !== 'string') return '';
  if (raw.includes('```')) return '';
  let text = raw;
  if (/^(?:here(?:'s| is)|sure[,!:]|the completion|explanation:)/i.test(text.trimStart())) return '';
  if (input.language.toLowerCase() === 'python') {
    const whitespace = [...text.matchAll(/^[ \t]+/gm)].map(match => match[0]).join('');
    if (whitespace.includes(' ') && whitespace.includes('\t')) return '';
  }
  for (let size = Math.min(text.length, input.before.length); size > 0; size--) {
    if (text.slice(0, size).trim() && input.before.endsWith(text.slice(0, size))) {
      text = text.slice(size); break;
    }
  }
  for (let size = Math.min(text.length, input.after.length); size > 0; size--) {
    if (input.after.startsWith(text.slice(-size))) { text = text.slice(0, -size); break; }
  }
  return text;
}
