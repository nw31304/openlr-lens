// ── LLM provider definitions ──────────────────────────────────────────────────
//
// authStyle:
//   'bearer'     → Authorization: Bearer <key>   (OpenAI, OpenRouter, Ollama, most others)
//   'anthropic'  → x-api-key: <key> + anthropic-version header + native message schema
//   'none'       → no auth header (Ollama local with no key set)

export const PROVIDERS = [
  {
    id: 'anthropic',
    label: 'Anthropic (direct)',
    baseUrl: 'https://api.anthropic.com',
    authStyle: 'anthropic',
    defaultModel: 'claude-sonnet-4-6',
    note: 'Requires anthropic-dangerous-direct-browser-access header (Anthropic\'s opt-in for browser use).',
  },
  {
    id: 'openrouter',
    label: 'OpenRouter',
    baseUrl: 'https://openrouter.ai/api/v1',
    authStyle: 'bearer',
    defaultModel: 'anthropic/claude-sonnet-4-6',
    note: 'One key for Claude, GPT, Gemini, and others. OpenAI-compatible API.',
  },
  {
    id: 'openai',
    label: 'OpenAI',
    baseUrl: 'https://api.openai.com/v1',
    authStyle: 'bearer',
    defaultModel: 'gpt-4o',
    note: null,
  },
  {
    id: 'ollama',
    label: 'Ollama (local)',
    baseUrl: 'http://localhost:11434/v1',
    authStyle: 'none',
    defaultModel: 'qwen2.5-coder:14b',
    note: 'Requires OLLAMA_ORIGINS=* if the app is not served from localhost.',
  },
  {
    id: 'custom',
    label: 'Custom…',
    baseUrl: '',
    authStyle: 'bearer',
    defaultModel: '',
    note: 'Any OpenAI-compatible endpoint.',
  },
];

// ── Storage ───────────────────────────────────────────────────────────────────
//
// The API key is stored in localStorage under a dedicated key, separate from
// the main settings store.  It is only ever sent to the configured provider URL.

const STORAGE_KEY = 'openlrlab.llm';

export function loadLlmConfig() {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null;
  }
}

export function saveLlmConfig(config) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(config));
}

export function clearLlmConfig() {
  localStorage.removeItem(STORAGE_KEY);
}

// ── SSE reader ────────────────────────────────────────────────────────────────
//
// Yields parsed JSON objects from a streaming fetch response.
// Ignores event-type lines, comment lines, and the [DONE] sentinel.

async function* readSSE(response) {
  const reader = response.body.getReader();
  const dec = new TextDecoder();
  let buf = '';

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    buf += dec.decode(value, { stream: true });
    const lines = buf.split('\n');
    buf = lines.pop() ?? '';
    for (const line of lines) {
      const t = line.trim();
      if (!t || t.startsWith(':') || t.startsWith('event:')) continue;
      if (t === 'data: [DONE]') return;
      if (t.startsWith('data: ')) {
        try { yield JSON.parse(t.slice(6)); } catch { /* skip malformed */ }
      }
    }
  }
}

// ── API call ──────────────────────────────────────────────────────────────────
//
// config: { providerId, baseUrl, apiKey, model }
// messages: [{ role: 'system'|'user'|'assistant', content: string }]
// tools: OpenAI-format tool definitions (optional)
// onDelta: optional callback(chunk: string) — called for each text chunk when streaming
//
// Returns: { ok: bool, content: string|null, tool_calls: array|null, error: string|null }

export async function chatComplete(config, messages, tools, onDelta) {
  const provider = PROVIDERS.find(p => p.id === config.providerId);
  const authStyle = provider?.authStyle ?? 'bearer';

  if (authStyle === 'anthropic') {
    return anthropicComplete(config, messages, tools, onDelta);
  }
  return openaiComplete(config, messages, tools, authStyle, onDelta);
}

async function openaiComplete({ baseUrl, apiKey, model }, messages, tools, authStyle, onDelta) {
  const headers = { 'Content-Type': 'application/json' };
  if (authStyle === 'bearer' && apiKey) {
    headers['Authorization'] = `Bearer ${apiKey}`;
  }

  const body = { model, messages };
  if (tools?.length) body.tools = tools;
  if (onDelta) body.stream = true;

  try {
    const res = await fetch(`${baseUrl}/chat/completions`, {
      method: 'POST',
      headers,
      body: JSON.stringify(body),
    });

    if (!onDelta) {
      const data = await res.json();
      if (!res.ok) return { ok: false, content: null, tool_calls: null, error: data.error?.message ?? `HTTP ${res.status}` };
      const msg = data.choices?.[0]?.message;
      return { ok: true, content: msg?.content ?? null, tool_calls: msg?.tool_calls ?? null, error: null };
    }

    // Streaming path
    if (!res.ok) {
      const data = await res.json().catch(() => ({}));
      return { ok: false, content: null, tool_calls: null, error: data.error?.message ?? `HTTP ${res.status}` };
    }

    let content = '';
    const tcAccum = {}; // index → { id, name, args }

    for await (const event of readSSE(res)) {
      const delta = event.choices?.[0]?.delta;
      if (!delta) continue;

      if (delta.content) {
        content += delta.content;
        onDelta(delta.content);
      }

      for (const tc of delta.tool_calls ?? []) {
        const idx = tc.index ?? 0;
        if (!tcAccum[idx]) tcAccum[idx] = { id: '', name: '', args: '' };
        if (tc.id) tcAccum[idx].id = tc.id;
        if (tc.function?.name) tcAccum[idx].name += tc.function.name;
        if (tc.function?.arguments) tcAccum[idx].args += tc.function.arguments;
      }
    }

    const tcArr = Object.values(tcAccum);
    const tool_calls = tcArr.length
      ? tcArr.map(tc => ({ id: tc.id, type: 'function', function: { name: tc.name, arguments: tc.args } }))
      : null;

    return { ok: true, content: content || null, tool_calls, error: null };
  } catch (e) {
    return { ok: false, content: null, tool_calls: null, error: e.message };
  }
}

// Convert an OpenAI-format message array to Anthropic message format.
// Handles tool call / tool result messages in conversation history:
//   - role:'tool' → role:'user' with tool_result content blocks (grouped so
//     consecutive tool results become a single user turn, as Anthropic requires)
//   - assistant messages with tool_calls → tool_use content blocks
function toAnthropicMessages(messages) {
  const out = [];
  let i = 0;
  while (i < messages.length) {
    const m = messages[i];
    // Group consecutive tool-result messages into one user turn
    if (m.role === 'tool') {
      const blocks = [];
      while (i < messages.length && messages[i].role === 'tool') {
        blocks.push({
          type: 'tool_result',
          tool_use_id: messages[i].tool_call_id,
          content: messages[i].content,
        });
        i++;
      }
      out.push({ role: 'user', content: blocks });
      continue;
    }
    // Assistant message that called tools → tool_use content blocks
    if (m.tool_calls?.length) {
      const content = [];
      if (m.content) content.push({ type: 'text', text: m.content });
      for (const tc of m.tool_calls) {
        content.push({
          type: 'tool_use',
          id: tc.id,
          name: tc.function.name,
          input: JSON.parse(tc.function.arguments),
        });
      }
      out.push({ role: 'assistant', content });
      i++;
      continue;
    }
    out.push(m);
    i++;
  }
  return out;
}

async function anthropicComplete({ baseUrl, apiKey, model }, messages, tools, onDelta) {
  const systemMsg = messages.find(m => m.role === 'system');
  const nonSystem = toAnthropicMessages(messages.filter(m => m.role !== 'system'));

  const headers = {
    'Content-Type': 'application/json',
    'x-api-key': apiKey,
    'anthropic-version': '2023-06-01',
    'anthropic-dangerous-direct-browser-access': 'true',
  };

  const body = { model, messages: nonSystem, max_tokens: 4096 };
  if (systemMsg) body.system = systemMsg.content;
  if (tools?.length) {
    body.tools = tools.map(t => ({
      name: t.function.name,
      description: t.function.description,
      input_schema: t.function.parameters,
    }));
  }
  if (onDelta) body.stream = true;

  try {
    const res = await fetch(`${baseUrl}/v1/messages`, {
      method: 'POST',
      headers,
      body: JSON.stringify(body),
    });

    if (!onDelta) {
      const data = await res.json();
      if (!res.ok) return { ok: false, content: null, tool_calls: null, error: data.error?.message ?? `HTTP ${res.status}` };

      // Normalize response back to OpenAI-like shape
      const textBlock = data.content?.find(b => b.type === 'text');
      const toolBlocks = data.content?.filter(b => b.type === 'tool_use') ?? [];
      const tool_calls = toolBlocks.length
        ? toolBlocks.map(b => ({ id: b.id, type: 'function', function: { name: b.name, arguments: JSON.stringify(b.input) } }))
        : null;
      return { ok: true, content: textBlock?.text ?? null, tool_calls, error: null };
    }

    // Streaming path
    if (!res.ok) {
      const data = await res.json().catch(() => ({}));
      return { ok: false, content: null, tool_calls: null, error: data.error?.message ?? `HTTP ${res.status}` };
    }

    let textContent = '';
    const toolBlocks = {}; // index → { id, name, args }

    for await (const event of readSSE(res)) {
      if (event.type === 'content_block_start' && event.content_block?.type === 'tool_use') {
        toolBlocks[event.index] = { id: event.content_block.id, name: event.content_block.name, args: '' };
      } else if (event.type === 'content_block_delta') {
        const d = event.delta;
        if (d?.type === 'text_delta' && d.text) {
          textContent += d.text;
          onDelta(d.text);
        } else if (d?.type === 'input_json_delta' && d.partial_json) {
          if (toolBlocks[event.index]) toolBlocks[event.index].args += d.partial_json;
        }
      }
    }

    const toolArr = Object.values(toolBlocks);
    const tool_calls = toolArr.length
      ? toolArr.map(b => ({ id: b.id, type: 'function', function: { name: b.name, arguments: b.args } }))
      : null;

    return { ok: true, content: textContent || null, tool_calls, error: null };
  } catch (e) {
    return { ok: false, content: null, tool_calls: null, error: e.message };
  }
}

// Convenience: send a minimal message to verify connectivity and auth.
export async function testConnection(config) {
  return chatComplete(
    config,
    [{ role: 'user', content: 'Reply with just the word OK.' }],
  );
}
