import type {
  AskUserQuestionRequestDto,
  UiJsonValue,
} from '@lingxi/bridge-client';
import type { CommandRunItem, NarrationRunItem, RunItem, ToolRunItem } from '../model/runItem';
import type { TranscriptToolGroup } from './transcriptRows';

export type NativeUiSiteProps = Record<string, UiJsonValue>;

function jsonRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

export function nativeAskUserQuestionProps(request: AskUserQuestionRequestDto): NativeUiSiteProps {
  return {
    tool: 'AskUserQuestion',
    questions: request.questions.map((question) => ({
      question: question.question,
      header: question.header,
      multi_select: question.multi_select,
      options: question.options.map((option) => ({
        label: option.label,
        description: option.description,
        ...(option.preview === undefined ? {} : { preview: option.preview }),
      })),
    })),
  };
}

function questionList(value: unknown): AskUserQuestionRequestDto['questions'] | null {
  if (!Array.isArray(value)) return null;
  const result: AskUserQuestionRequestDto['questions'] = [];
  for (const question of value) {
    if (!jsonRecord(question) || typeof question.question !== 'string' || typeof question.header !== 'string'
      || typeof question.multi_select !== 'boolean' || !Array.isArray(question.options)) return null;
    const options = [];
    for (const option of question.options) {
      if (!jsonRecord(option) || typeof option.label !== 'string' || typeof option.description !== 'string'
        || (option.preview !== undefined && typeof option.preview !== 'string')) return null;
      options.push({
        label: option.label,
        description: option.description,
        ...(option.preview === undefined ? {} : { preview: option.preview }),
      });
    }
    result.push({ question: question.question, header: question.header, multi_select: question.multi_select, options });
  }
  return result;
}

/** Preserve the original request when an engine fallback carries invalid rewritten question data. */
export function askUserQuestionWithResponseProps(
  request: AskUserQuestionRequestDto,
  responseProps: Record<string, UiJsonValue>,
): AskUserQuestionRequestDto {
  const questions = questionList(responseProps.questions);
  return questions === null ? request : { ...request, questions };
}

export function nativeNarrationProps(item: NarrationRunItem, isFirstOfReply: boolean): NativeUiSiteProps {
  const props: NativeUiSiteProps = { text: item.text };
  if (item.role === 'assistant') props.isFirstOfReply = isFirstOfReply;
  return props;
}

export function nativeToolUseProps(item: ToolRunItem): NativeUiSiteProps {
  return {
    tool_use_id: item.id,
    tool: item.tool,
    ...(item.nativeInput === undefined ? {} : { input: item.nativeInput }),
    isRunning: item.status === 'running',
    isErrored: item.status === 'error',
    ...(item.nativeOutput === undefined ? {} : { output: item.nativeOutput }),
  };
}

export function nativeToolResultProps(item: ToolRunItem): NativeUiSiteProps {
  return {
    tool_use_id: item.id,
    tool: item.tool,
    ...(item.nativeOutput === undefined ? {} : { output: item.nativeOutput }),
    isErrored: item.status === 'error',
  };
}

export function nativeToolGroupProps(group: TranscriptToolGroup, isExpanded: boolean): NativeUiSiteProps {
  return {
    calls: group.tools.map((tool) => nativeToolUseProps(tool)),
    isActive: group.tools.some((tool) => tool.status === 'running'),
    isExpanded,
  };
}

export function nativeCommandOutputProps(item: CommandRunItem): NativeUiSiteProps {
  return {
    command: item.name,
    ...(item.args === undefined ? {} : { args: item.args }),
    text: item.output,
    isErrored: item.isError,
  };
}

export function assistantFirstOfReplyByItem(items: readonly RunItem[]): ReadonlyMap<string, boolean> {
  const result = new Map<string, boolean>();
  let assistantSeen = false;
  for (const item of items) {
    if (item.type !== 'narration') continue;
    if (item.role === 'user') {
      assistantSeen = false;
    } else if (item.role === 'assistant') {
      result.set(item.id, !assistantSeen);
      assistantSeen = true;
    }
  }
  return result;
}
