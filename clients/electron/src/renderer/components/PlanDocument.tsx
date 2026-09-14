import { useState } from 'react';
import { useT } from '../theme/ThemeContext';
import { MarkdownContent } from './MarkdownContent';
import { Icon } from './Icon';
import type { SubmittedPlan } from '../bridge/submittedPlan';
export function PlanDocument({ content, status }: { content: string; status?: string }) {
  const t = useT();
  const [copyStatus, setCopyStatus] = useState('');
  async function copy() {
    try {
      await (window.lingxi?.copyText(content) ?? navigator.clipboard.writeText(content));
      setCopyStatus('Copied');
    } catch { setCopyStatus('Copy failed'); }
  }
  return <article style={{ padding: '20px 24px 40px', color: t.text, fontSize: 14, lineHeight: 1.7, overflowWrap: 'anywhere' }}>
    <button type="button" aria-label="Copy plan" onClick={() => void copy()} style={{ float: 'right', margin: '0 0 12px 16px', border: 0, background: 'transparent', color: t.text3, cursor: 'pointer', padding: 5 }}><Icon name="copy" size={16}/>{copyStatus && <span role="status">{copyStatus}</span>}</button>
    <MarkdownContent variant="plan" text={content}/>
    {status && <p role="status" style={{color:t.text3,fontSize:12,marginTop:24}}>{status}</p>}
  </article>;
}
export function PlanPreview({ content, status = 'submitted', writing = false, onOpen }: { content: string; status?: SubmittedPlan['status']; writing?: boolean; onOpen(): void }) {
 const t=useT();
 const [copyStatus, setCopyStatus] = useState('');
 async function copy() {
   try {
     await (window.lingxi?.copyText(content) ?? navigator.clipboard.writeText(content));
     setCopyStatus('Copied');
   } catch { setCopyStatus('Copy failed'); }
 }
 const hasContent = content.trim().length > 0;
 const label = status === 'failed' ? 'Plan submission failed'
   : status === 'rejected' ? 'Plan rejected'
   : status === 'pending' ? 'Waiting for plan approval'
   : status === 'approved' && !hasContent ? 'Plan mode exited'
   : writing && status === 'submitted' ? 'Writing plan' : 'Plan';
 return <div style={{position:'relative',width:'100%',border:`1px solid ${t.border}`,borderRadius:14,overflow:'hidden',padding:'16px 18px',background:'transparent'}}>
   <div style={{display:'flex',alignItems:'center',gap:8,color:t.text3,fontSize:13,marginBottom:18}}><svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="M9 18h6m-6 3h6M8 13a6 6 0 1 1 8 0c-1 1-1 2-1 3H9c0-1 0-2-1-3ZM12 1v2M3 5l2 2m14-2-2 2M1 11h3m16 0h3"/></svg><span>{label}</span>
     {hasContent && <button type="button" aria-label="Copy plan" title="Copy plan" onClick={event => { event.stopPropagation(); void copy(); }} style={{position:'relative',zIndex:1,marginLeft:'auto',display:'flex',alignItems:'center',gap:6,border:0,background:'transparent',color:t.text3,cursor:'pointer',padding:5}}><Icon name="copy" size={16}/><span role="status" aria-live="polite">{copyStatus}</span></button>}
   </div>
   {hasContent ? <div style={{maxHeight:230,overflow:'hidden',maskImage:'linear-gradient(#000 65%, transparent)',fontSize:14,lineHeight:1.65}}><MarkdownContent variant="plan" text={content}/></div> : <p role="status" style={{margin:0,fontSize:14,lineHeight:1.65}}>{status === 'submitted' ? 'Preparing plan…' : status === 'pending' ? 'Waiting for approval…' : 'No plan document was submitted.'}</p>}
   {hasContent && <button type="button" aria-label="Open full plan" onClick={onOpen} style={{position:'absolute',inset:0,border:0,borderRadius:14,background:'transparent',cursor:'pointer'}}/>}
 </div>;
}
