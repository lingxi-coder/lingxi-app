import { useCallback, useEffect, useId, useMemo, useRef, useState, type ReactNode } from 'react';
import { ShareLinkButton } from './ShareLinkButton';
import { useTeamsText } from './i18n.js';
import type { MessageApi, MessageScope, MessageSnapshot, PostInteraction, TeamMessage } from './messages.js';
import { postShareLink } from './share.js';
import { Icon } from './ui';

interface Props {
  api: Pick<MessageApi, 'messages' | 'watchMessages'>;
  scope: MessageScope;
  conversationId: string;
  post: TeamMessage;
  locale: string;
  online: boolean;
  participant: boolean;
  knownReplies: number;
  threadComplete: boolean;
  openForReply: boolean;
  needsThread: boolean;
  loadThread(): void;
  onReply?(): void;
  children: ReactNode;
}

/** Counts belong to the post. The viewer's own like comes from a private membership-checked receipt. */
export function PostActions({ api, scope: { teamId, generation }, conversationId, post, locale, online, participant,
  knownReplies, threadComplete, openForReply, needsThread, loadThread, onReply, children }: Props) {
  const t = useTeamsText(locale), threadId = useId();
  const replyButton = useRef<HTMLButtonElement>(null);
  const bar = useRef<HTMLDivElement>(null), version = useRef(0), saving = useRef(false);
  const [viewer, setViewer] = useState<PostInteraction>();
  const [live, setLive] = useState<Extract<MessageSnapshot, { kind: 'postStats' }>>();
  const [pendingLike, setPendingLike] = useState<{ liked: boolean; count: number }>();
  const [failed, setFailed] = useState(false);
  const [open, setOpen] = useState(openForReply);
  const numbers = useMemo(() => new Intl.NumberFormat(locale, { notation: 'compact', maximumFractionDigits: 1 }), [locale]);
  const interact = useCallback(async (change?: { liked: boolean; count: number }) => {
    if (saving.current) return;
    const request = ++version.current;
    setFailed(false);
    if (change) { saving.current = true; setPendingLike(change); }
    try {
      const result = await api.messages<PostInteraction>({ action: 'postInteraction', teamId, generation, conversationId, postId: post.id,
        ...(change ? { liked: change.liked } : {}) });
      if (!result?.stats || typeof result.liked !== 'boolean') throw new Error('INVALID_POST_STATE');
      if (request === version.current) setViewer(result);
    } catch { if (request === version.current) setFailed(true); }
    finally {
      if (request === version.current) { saving.current = false; setPendingLike(undefined); }
    }
  }, [api, teamId, generation, conversationId, post.id]);

  useEffect(() => () => { version.current++; }, [interact]);
  useEffect(() => {
    const row = bar.current?.closest('[data-channel-post]');
    if (!row || !online || post.recalled || typeof IntersectionObserver === 'undefined') return;
    let inView = false, recorded = false, stop: (() => void) | undefined;
    const observe = () => {
      if (!inView || document.visibilityState !== 'visible') {
        stop?.(); stop = undefined; return;
      }
      stop ??= api.watchMessages({ teamId, generation }, conversationId, snapshot => {
        if (snapshot.kind === 'postStats' && snapshot.postId === post.id) setLive(snapshot);
      }, post.id);
      if (!recorded) { recorded = true; void interact(); }
    };
    const visibility = () => { if (document.visibilityState === 'hidden') recorded = false; observe(); };
    const observer = new IntersectionObserver(([entry]) => {
      inView = Boolean(entry?.isIntersecting && entry.intersectionRect.height > 0);
      observe();
    });
    observer.observe(row);
    document.addEventListener('visibilitychange', visibility);
    return () => { stop?.(); observer.disconnect(); document.removeEventListener('visibilitychange', visibility); };
  }, [api, teamId, generation, conversationId, post.id, interact, online, post.recalled]);
  useEffect(() => { if (openForReply) setOpen(true); }, [openForReply]);
  useEffect(() => { if (open && online && needsThread) loadThread(); }, [open, online, needsThread, loadThread]);

  const incoming = live?.stats && (!post.postStats || live.stats.revision >= post.postStats.revision) ? live.stats : post.postStats;
  const stats = !viewer || (incoming && incoming.revision > viewer.stats.revision) ? incoming : viewer.stats;
  const lastLike = live?.reaction && live.reaction.revision >= (viewer?.stats.revision ?? 0) ? live.reaction.liked : viewer?.liked;
  const liked = pendingLike?.liked ?? lastLike ?? false;
  const likes = pendingLike?.count ?? stats?.likes;
  const count = (value?: number) => value === undefined ? '–' : numbers.format(Math.max(0, value));
  const replies = stats?.replies ?? (threadComplete ? knownReplies : undefined);
  const replyCount = replies === undefined && knownReplies > 0 ? `${count(knownReplies)}+` : count(replies);
  const replyLabel = t(onReply ? 'teams_message_reply' : 'tw_channel_replies'), likeLabel = t(liked ? 'tw_post_unlike' : 'tw_post_like'), viewsLabel = t('tw_post_views');
  return <div className="tw-post-discussion">
    <div ref={bar} className="tw-post-actions" role="group" aria-label={t('teams_message_actions')}>
      <button ref={replyButton} type="button" aria-expanded={open} aria-controls={threadId} aria-label={`${replyLabel} · ${replies ?? replyCount}`} title={replyLabel}
        onClick={() => { if (onReply) { setOpen(true); onReply(); } else setOpen(!open); }}>
        <Icon name="comment" size={18} /><span>{replyCount}</span>
      </button>
      {!post.recalled && <>
        <button type="button" className="tw-post-like" aria-pressed={liked} aria-label={likeLabel} title={likeLabel}
          disabled={!participant || !online || !!pendingLike || !viewer} onClick={() => void interact({ liked: !liked, count: Math.max(0, (likes ?? 0) + (liked ? -1 : 1)) })}>
          <Icon name="heart" size={18} /><span>{count(likes)}</span>
        </button>
        <span className="tw-post-views" role="img" aria-label={`${viewsLabel} · ${stats?.views ?? '–'}`} title={viewsLabel}>
          <Icon name="views" size={18} /><span>{count(stats?.views)}</span>
        </span>
        <ShareLinkButton post link={postShareLink(teamId, conversationId, post.id)} title={t('tw_channel_posts')} locale={locale} />
      </>}
    </div>
    {failed && online && <div className="tw-post-status" role="status">{t('tw_post_activity_failed')}
      <button type="button" className="tw-link" onClick={() => void interact()}>{t('common_retry')}</button></div>}
    <div id={threadId} hidden={!open} className="tw-post-replies">{open && <>
      {children}
      <div className="tw-post-thread-end">
        {onReply && knownReplies > 0 && <button type="button" className="tw-post-write" onClick={onReply}><Icon name="reply" size={15} />{t('teams_message_reply')}</button>}
        <button type="button" className="tw-post-write" onClick={() => { setOpen(false); replyButton.current?.focus(); }}>{t('tw_post_hide_replies')}</button>
      </div>
    </>}</div>
  </div>;
}
