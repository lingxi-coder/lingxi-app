import { Markdown } from '../remote/markdown';
import type { RenderMarkdown } from '../../../../packages/teams/ui';

/** Bot messages in Chat use Web's own Markdown renderer: React elements only, links limited to http(s) and mailto. */
export const renderMarkdown: RenderMarkdown = text => <Markdown text={text} />;
