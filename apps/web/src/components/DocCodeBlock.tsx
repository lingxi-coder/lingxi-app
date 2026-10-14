import { useState } from 'react';
import { DocCode } from '../data/docs/types';
import { codeLabel, verificationLabels } from '../data/docs/sources';
import { IconCheck, IconCopy } from './Icons';
import { Locale } from '../utils/locale';

function highlight(code: string) {
  const pattern = /(\/\/[^\n]*|#[^\n]*|"(?:\\.|[^"\\])*"|'[^'\n]*'|\b(?:use|pub|fn|async|await|let|mut|const|struct|enum|impl|trait|return|match|if|else|for|in|import|from|export|class|new|try|catch|val|func|throws|true|false|None|Some|Ok|Err)\b|\b\d+(?:\.\d+)?\b)/gu;
  const keywords = /^(?:use|pub|fn|async|await|let|mut|const|struct|enum|impl|trait|return|match|if|else|for|in|import|from|export|class|new|try|catch|val|func|throws|true|false|None|Some|Ok|Err)$/u;
  return code.split(pattern).map((token, index) => <span key={index} className={/^\/\/|^#/u.test(token) ? 'code-comment' : /^["']/u.test(token) ? 'code-string' : /^\d/u.test(token) ? 'code-number' : keywords.test(token) ? 'code-keyword' : undefined}>{token}</span>);
}


export function DocCodeBlock({ blocks, locale }: { blocks: DocCode[]; locale: Locale }) {
  const [selected, setSelected] = useState(0);
  const [status, setStatus] = useState<'idle' | 'copied' | 'failed'>('idle');
  const current = blocks[selected] ?? blocks[0];
  if (!current) return null;
  const copy = async () => {
    try { await navigator.clipboard.writeText(current.code); setStatus('copied'); }
    catch { setStatus('failed'); }
  };
  return <div className="developer-code-block">
    <div className="developer-code-header"><div className="developer-code-tabs">{blocks.map((block, index) => <button type="button" key={index} aria-pressed={selected === index} className={selected === index ? 'active' : ''} onClick={() => { setSelected(index); setStatus('idle'); }}>{codeLabel(block.label, locale)}</button>)}</div><button type="button" className="developer-copy-code" onClick={copy} aria-label={locale === 'zh' ? '复制代码' : 'Copy code'}>{status === 'copied' ? <IconCheck width={15} height={15} /> : <IconCopy width={15} height={15} />}<span>{status === 'copied' ? (locale === 'zh' ? '已复制' : 'Copied') : (locale === 'zh' ? '复制' : 'Copy')}</span></button></div>
    <pre><code className={`language-${current.language}`}>{highlight(current.code)}</code></pre>
    {current.verification ? <div className="developer-code-verification">{locale === 'zh' ? '验证方式' : 'Verification method'}: {verificationLabels[current.verification][locale]}</div> : null}
    <span role="status" className="developer-visually-hidden">{status === 'copied' ? (locale === 'zh' ? '代码已复制' : 'Code copied') : ''}</span>
    {status === 'failed' ? <p role="status" className="developer-copy-error">{locale === 'zh' ? '复制不可用，请选中代码手动复制。' : 'Copy is unavailable. Select the code to copy manually.'}</p> : null}
  </div>;
}
