import { createContext, useContext, type CSSProperties } from 'react';
import type { BotCreationProposal } from '../bridge/botCreation';
import { tr } from '../i18n/locale';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';
import './BotCreation.css';

/** Only the app's assistant transcript is given an action that opens the native form. */
export const BotProposalAction = createContext<((proposal: BotCreationProposal) => void) | undefined>(undefined);

export function BotProposalCard({ proposal }: { proposal: BotCreationProposal }) {
  const review = useContext(BotProposalAction);
  const t = useT();
  const style = { '--bot-proposal-bg': t.surface, '--bot-proposal-border': t.border, '--bot-proposal-muted': t.text3,
    '--bot-proposal-accent': t.accent, '--bot-proposal-text': t.text } as CSSProperties;
  return <section className="bot-proposal" style={style} aria-label={tr('bots_assist_proposal')}>
    <header><span className="bot-proposal-icon" aria-hidden="true"><Icon name="bot" size={24} /></span>
      <div><span className="bot-proposal-eyebrow">{tr('bots_assist_proposal')}</span><h3>{proposal.name}</h3></div>
    </header>
    {proposal.description ? <p>{proposal.description}</p> : null}
    <details><summary>{tr('bots_tab_persona')}</summary><p className="bot-proposal-persona">{proposal.persona}</p></details>
    <footer><span>{tr(review ? 'bots_assist_proposal_hint' : 'bots_assist_desktop_hint')}</span>
      {review ? <button type="button" onClick={() => review(proposal)}>{tr('bots_assist_review')}</button> : null}
    </footer>
  </section>;
}
