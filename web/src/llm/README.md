# LLM System Prompt

`SYSTEM_PROMPT.md` is the source of truth for the AI chat system prompt.
Edit it freely — plain markdown, no special syntax required. `llmDiagnosis.js`
imports it directly at build time via Vite's `?raw` loader
(`import SYSTEM_PROMPT from './llm/SYSTEM_PROMPT.md?raw'`) — there is no
generation step and no separate `systemPrompt.js` file. Editing
`SYSTEM_PROMPT.md` and reloading (or restarting `npm run dev`) is enough.

The tool contracts it documents (parameters, when to call each one) must stay
in sync with the actual `tools` array and `case` handlers in `tools.js` — when
you add, rename, or change the parameters of a tool there, update its
description here in the same change.

## Asking an LLM to improve the prompt

Paste the contents of `SYSTEM_PROMPT.md` into a capable model (Claude, GPT-4o)
with a prompt like:

> You are reviewing a system prompt for an OpenLR decode/encode diagnostic
> assistant. Improve clarity, fix any inaccuracies, and add a worked example
> for [failure mode]. Keep the existing structure. Return only the revised
> markdown.

Copy the response back into `SYSTEM_PROMPT.md`.

## Files

| File | Purpose |
|---|---|
| `SYSTEM_PROMPT.md` | The system prompt — edit this, no build step needed |
| `tools.js` | Tool schemas + handlers the prompt above describes |
