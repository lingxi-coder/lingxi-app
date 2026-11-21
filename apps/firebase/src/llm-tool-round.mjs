const MAX_CALLS = 16;
const MAX_ROUND_BYTES = 256 * 1024;

function invalidRound() {
  return Object.assign(new Error('INVALID_TOOL_ROUND'), { code: 'INVALID_TOOL_ROUND' });
}

function readArguments(value) {
  const args = typeof value === 'string' ? JSON.parse(value) : value;
  if (!args || typeof args !== 'object' || Array.isArray(args)) throw invalidRound();
  return args;
}

function toolResult(results, id) {
  if (!Object.hasOwn(results, id) || results[id] === undefined) throw invalidRound();
  return results[id];
}

export function toolRequest(protocol, tools, rounds) {
  const declarations = tools.map(({ name, description, parameters }) => ({ name, description, parameters }));
  switch (protocol) {
    case 'open_ai_chat':
      return {
        tools: declarations.map(fn => ({ type: 'function', function: fn })),
        continuation: rounds.flatMap(({ assistant, calls, results }) => [
          assistant,
          ...calls.map(({ id }) => ({ role: 'tool', tool_call_id: id, content: JSON.stringify(toolResult(results, id)) })),
        ]),
      };
    case 'open_ai_responses':
      return {
        tools: declarations.map(fn => ({ type: 'function', strict: false, ...fn })),
        continuation: rounds.flatMap(({ assistant, calls, results }) => [
          ...assistant,
          ...calls.map(({ id }) => ({ type: 'function_call_output', call_id: id, output: JSON.stringify(toolResult(results, id)) })),
        ]),
      };
    case 'anthropic_messages':
      return {
        tools: declarations.map(({ parameters, ...fn }) => ({ ...fn, input_schema: parameters })),
        continuation: rounds.flatMap(({ assistant, calls, results }) => [
          assistant,
          { role: 'user', content: calls.map(({ id }) => ({ type: 'tool_result', tool_use_id: id, content: JSON.stringify(toolResult(results, id)) })) },
        ]),
      };
    case 'gemini_generate_content':
      return {
        tools: [{ functionDeclarations: declarations.map(({ parameters, ...fn }) => ({ ...fn, parametersJsonSchema: parameters })) }],
        continuation: rounds.flatMap(({ assistant, calls, results }) => [
          assistant,
          { role: 'user', parts: calls.map(({ id, name, providerId }) => ({
            functionResponse: { ...(providerId ? { id: providerId } : {}), name, response: { result: toolResult(results, id) } },
          })) },
        ]),
      };
    default:
      throw invalidRound();
  }
}

// Tool calls become eligible for approval only after a successful terminal frame.
export function createToolRound(protocol, toolNames, { geminiCompatibility = false } = {}) {
  const indexed = new Map();
  const seenSequences = new Set();
  const geminiParts = [];
  let chatText = '';
  let chatReasoning = '';
  let responsesOutput = [];
  let responsesSawCall = false;
  let bytes = 0;
  let complete = false;

  function accept(value, type) {
    const sequence = value.sequence_number;
    if (protocol === 'open_ai_responses' && Number.isSafeInteger(sequence)) {
      if (seenSequences.has(sequence)) return false;
      seenSequences.add(sequence);
    }
    bytes += Buffer.byteLength(JSON.stringify(value));
    if (bytes > MAX_ROUND_BYTES) throw invalidRound();
    switch (protocol) {
      case 'open_ai_chat': {
        const choice = value.choices?.[0], delta = choice?.delta;
        if (typeof delta?.content === 'string') chatText += delta.content;
        if (typeof delta?.reasoning_content === 'string') chatReasoning += delta.reasoning_content;
        for (const part of delta?.tool_calls ?? []) {
          let index = part.index;
          // Gemini's compatibility endpoint can send each complete call without an index.
          if (geminiCompatibility && index === undefined && typeof part.id === 'string' && part.id) {
            readArguments(part.function?.arguments);
            index = indexed.size;
            if (indexed.has(index)) throw invalidRound();
          }
          if (!Number.isInteger(index) || index < 0 || index >= MAX_CALLS) throw invalidRound();
          const call = indexed.get(index) ?? { id: '', name: '', arguments: '' };
          if (part.id) {
            if (call.id && call.id !== part.id) throw invalidRound();
            call.id = part.id;
          }
          if (typeof part.function?.name === 'string') call.name += part.function.name;
          if (typeof part.function?.arguments === 'string') call.arguments += part.function.arguments;
          const signature = part.extra_content?.google?.thought_signature;
          if (geminiCompatibility && typeof signature === 'string' && signature) {
            if (call.signature && call.signature !== signature) throw invalidRound();
            call.signature = signature;
          }
          indexed.set(index, call);
        }
        if (choice?.finish_reason) {
          complete = choice.finish_reason === 'tool_calls' || (geminiCompatibility && choice.finish_reason === 'stop');
        }
        break;
      }
      case 'open_ai_responses':
        if (value.item?.type === 'function_call' || (typeof type === 'string' && type.startsWith('response.function_call_arguments.'))) responsesSawCall = true;
        if (type === 'response.completed' && Array.isArray(value.response?.output)) {
          responsesOutput = value.response.output;
          complete = true;
        }
        break;
      case 'anthropic_messages': {
        if (type === 'content_block_start') {
          const block = value.content_block;
          if (!Number.isInteger(value.index) || value.index < 0 || indexed.has(value.index) || !block) throw invalidRound();
          indexed.set(value.index, { block: { ...block }, input: '', closed: false });
        } else if (type === 'content_block_delta') {
          const entry = indexed.get(value.index), delta = value.delta;
          if (!entry || entry.closed || !delta) throw invalidRound();
          if (entry.block.type === 'tool_use' && delta.type === 'input_json_delta') entry.input += delta.partial_json;
          if (entry.block.type === 'text' && delta.type === 'text_delta') entry.block.text += delta.text;
          if (entry.block.type === 'thinking' && delta.type === 'thinking_delta') entry.block.thinking += delta.thinking;
          if (entry.block.type === 'thinking' && delta.type === 'signature_delta') entry.block.signature = delta.signature;
        } else if (type === 'content_block_stop') {
          const entry = indexed.get(value.index);
          if (!entry || entry.closed) throw invalidRound();
          entry.closed = true;
        } else if (type === 'message_delta') {
          complete = value.delta?.stop_reason === 'tool_use';
        }
        break;
      }
      case 'gemini_generate_content': {
        const candidate = value.candidates?.[0];
        if (candidate?.content?.parts) geminiParts.push(...candidate.content.parts);
        if (candidate?.finishReason) complete = candidate.finishReason === 'STOP';
        break;
      }
      default:
        throw invalidRound();
    }
    return true;
  }

  function result() {
    const ordered = [...indexed].sort(([left], [right]) => left - right).map(([, entry]) => entry);
    let raw, assistant;
    switch (protocol) {
      case 'open_ai_chat':
        raw = ordered;
        assistant = { role: 'assistant', content: chatText || null,
          ...(chatReasoning ? { reasoning_content: chatReasoning } : {}),
          tool_calls: raw.map(({ id, name, arguments: args, signature }) => ({ id, type: 'function',
            function: { name, arguments: args }, ...(signature ? { extra_content: { google: { thought_signature: signature } } } : {}) })) };
        break;
      case 'open_ai_responses':
        raw = responsesOutput.filter(item => item.type === 'function_call')
          .map(({ call_id: id, name, arguments: args }) => ({ id, name, arguments: args }));
        if (responsesSawCall && (!complete || !raw.length)) throw invalidRound();
        assistant = responsesOutput;
        break;
      case 'anthropic_messages': {
        const tools = ordered.filter(({ block }) => block.type === 'tool_use');
        raw = tools.map(({ block, input }) => ({ id: block.id, name: block.name, arguments: input || block.input }));
        if (raw.length && ordered.some(entry => !entry.closed)) throw invalidRound();
        assistant = { role: 'assistant', content: ordered.map(({ block, input }) =>
          block.type === 'tool_use' ? { ...block, input: readArguments(input || block.input) } : block) };
        break;
      }
      case 'gemini_generate_content': {
        const functions = geminiParts.flatMap(part => part.functionCall ? [part.functionCall] : []);
        raw = functions.map(({ id, name, args }, index) => ({
          id: id || 'gemini_' + index, providerId: id, name, arguments: args ?? {},
        }));
        assistant = { role: 'model', parts: geminiParts };
        break;
      }
      default:
        throw invalidRound();
    }
    if (!raw.length) return null;
    if (!complete || raw.length > MAX_CALLS) throw invalidRound();
    const seenIds = new Set();
    const calls = raw.map(({ id, providerId, name, arguments: args }) => {
      if (typeof id !== 'string' || !id || id.length > 256 || /[\u0000-\u001f\u007f]/u.test(id) || seenIds.has(id) || !toolNames.has(name)) throw invalidRound();
      seenIds.add(id);
      return { id, ...(providerId ? { providerId } : {}), name, arguments: readArguments(args) };
    });
    return { type: 'toolRound', assistant, calls };
  }

  return { accept, result };
}
