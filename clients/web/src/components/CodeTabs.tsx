import { useState } from 'react';
import { IconCheck, IconCopy } from './Icons';

interface CodeTab {
  id: string;
  label: string;
  code: string;
}

interface CodeTabsProps {
  tabs: CodeTab[];
}

export function CodeTabs({ tabs }: CodeTabsProps) {
  const [activeTab, setActiveTab] = useState(tabs[0]?.id ?? '');
  const [copiedTab, setCopiedTab] = useState<string | null>(null);
  const currentTab = tabs.find((tab) => tab.id === activeTab) ?? tabs[0];

  const handleCopy = async () => {
    if (!currentTab) {
      return;
    }
    try {
      await navigator.clipboard.writeText(currentTab.code);
      setCopiedTab(currentTab.id);
      window.setTimeout(() => setCopiedTab(null), 1200);
    } catch {
      setCopiedTab(null);
    }
  };

  if (!currentTab) {
    return null;
  }

  return (
    <div className="code-tabs">
      <div className="code-tabs-header">
        <div className="segmented-control">
          {tabs.map((tab) => (
            <button
              key={tab.id}
              type="button"
              className={tab.id === currentTab.id ? 'active' : ''}
              onClick={() => setActiveTab(tab.id)}
            >
              {tab.label}
            </button>
          ))}
        </div>
        <button type="button" className="control-chip" onClick={handleCopy}>
          {copiedTab === currentTab.id ? <IconCheck width={16} height={16} /> : <IconCopy width={16} height={16} />}
          <span>{copiedTab === currentTab.id ? 'Copied' : 'Copy'}</span>
        </button>
      </div>
      <pre className="code-sample">
        <code>{currentTab.code}</code>
      </pre>
    </div>
  );
}
