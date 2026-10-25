import { KeyboardEvent, useState } from 'react';
import { IconArrowRight, IconCheck, IconSearch, IconSpark } from './Icons';
import { Locale, pickLocaleText } from '../utils/locale';

const scenarios = [
  {
    id: 'ideas',
    Icon: IconSpark,
    label: { en: 'Think it through', zh: '梳理想法' },
    title: { en: 'A little clarity. A place to start.', zh: '想法有了，下一步也清楚了。' },
    prompt: {
      en: 'I want to host a weekend book club. Help me turn the idea into a practical plan.',
      zh: '想做一个周末读书会，帮我把想法整理成可执行的计划。',
    },
    steps: [
      { title: { en: 'Find the focus', zh: '先确定主题' }, detail: { en: 'Choose a book and a topic everyone can join in on.', zh: '选一本书、一个大家都能参与的话题。' } },
      { title: { en: 'Make it manageable', zh: '把准备拆小' }, detail: { en: 'Work out the time, place, and a short reading list.', zh: '确认时间、地点，列出简短的共读清单。' } },
      { title: { en: 'Take the first step', zh: '从一件事开始' }, detail: { en: 'Draft an invitation to share with your friends.', zh: '写一份可以发给朋友的邀请。' } },
    ],
  },
  {
    id: 'research',
    Icon: IconSearch,
    label: { en: 'Explore a question', zh: '查找资料' },
    title: { en: 'Find what matters. Make your choice.', zh: '先找到重点，再做选择。' },
    prompt: {
      en: 'Help me explore note-taking methods. What should I compare to find one that suits me?',
      zh: '我想选一套适合自己的笔记方法，帮我理清该比较什么。',
    },
    steps: [
      { title: { en: 'Start with your needs', zh: '明确使用场景' }, detail: { en: 'Are you learning, gathering ideas, or keeping track of work?', zh: '是学习整理、灵感收集，还是工作记录？' } },
      { title: { en: 'Compare the essentials', zh: '建立比较维度' }, detail: { en: 'Look at capture, organization, and finding things again.', zh: '围绕记录、组织和回顾，整理要查找的信息。' } },
      { title: { en: 'Try it in everyday life', zh: '回到实际使用' }, detail: { en: 'Test a small example before committing to a whole system.', zh: '用一个小例子试试，再决定是否长期使用。' } },
    ],
  },
  {
    id: 'tasks',
    Icon: IconCheck,
    label: { en: 'Plan your next step', zh: '推进任务' },
    title: { en: 'Less on your mind. More getting done.', zh: '把零散的事，理出头绪。' },
    prompt: {
      en: 'My to-do list is all over the place. Help me work out what to do first today.',
      zh: '把这些零散的待办整理一下，帮我排出今天的优先级。',
    },
    steps: [
      { title: { en: 'Check the deadlines', zh: '先看截止时间' }, detail: { en: 'Identify what needs your attention today.', zh: '找出今天必须处理、需要提前准备的事情。' } },
      { title: { en: 'Make room for focus', zh: '给重要的事留时间' }, detail: { en: 'Group small errands and protect time for the bigger task.', zh: '合并零碎待办，为重要任务留出完整时间。' } },
      { title: { en: 'Keep the next step clear', zh: '让下一步具体一点' }, detail: { en: 'Turn each priority into one action you can start.', zh: '把优先事项拆成现在就能开始的一个动作。' } },
    ],
  },
];

export function HomeScenarios({ locale }: { locale: Locale }) {
  const [activeIndex, setActiveIndex] = useState(0);
  const active = scenarios[activeIndex]!;
  const text = (value: { en: string; zh: string }) => pickLocaleText(locale, value);
  const t = (en: string, zh: string) => text({ en, zh });

  const handleKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    let nextIndex: number;
    if (event.key === 'ArrowRight') nextIndex = (index + 1) % scenarios.length;
    else if (event.key === 'ArrowLeft') nextIndex = (index + scenarios.length - 1) % scenarios.length;
    else if (event.key === 'Home') nextIndex = 0;
    else if (event.key === 'End') nextIndex = scenarios.length - 1;
    else return;
    event.preventDefault();
    setActiveIndex(nextIndex);
    document.getElementById(`scenario-tab-${scenarios[nextIndex]!.id}`)?.focus();
  };

  return (
    <section id="landing-scenarios" className="landing-scenarios" aria-labelledby="landing-scenarios-title">
      <div className="landing-scenario-heading">
        <div>
          <span className="landing-eyebrow">{t('A little help, every day', '从日常的一件小事开始')}</span>
          <h2 id="landing-scenarios-title">{t('What’s on your mind?', '你想做的事，一起往前一步。')}</h2>
        </div>
        <p>{t('Explore a few examples of where to begin.', '选择一个场景示例，看看可以怎样开始。')}</p>
      </div>
      <div className="landing-scenario-workspace">
        <div className="landing-scenario-toolbar">
          <div className="landing-scenario-tabs" role="tablist" aria-label={t('Everyday scenarios', '日常使用场景')}>
            {scenarios.map((scenario, index) => (
              <button
                key={scenario.id}
                id={`scenario-tab-${scenario.id}`}
                type="button"
                role="tab"
                aria-selected={activeIndex === index}
                aria-controls={`scenario-panel-${scenario.id}`}
                tabIndex={activeIndex === index ? 0 : -1}
                onClick={() => setActiveIndex(index)}
                onKeyDown={(event) => handleKeyDown(event, index)}
              >
                <scenario.Icon width={17} height={17} />
                {text(scenario.label)}
              </button>
            ))}
          </div>
          <span className="landing-scenario-caption">{t('Illustrative scenarios', '使用场景示例')}</span>
        </div>
        {scenarios.map((scenario, index) => (
          <div
            key={scenario.id}
            id={`scenario-panel-${scenario.id}`}
            className="landing-scenario-panel"
            role="tabpanel"
            aria-labelledby={`scenario-tab-${scenario.id}`}
            hidden={activeIndex !== index}
            tabIndex={0}
          >
            {activeIndex === index ? <>
              <div className="landing-scenario-request">
                <span className="landing-scenario-index" aria-hidden="true">0{activeIndex + 1} / 03</span>
                <h3>{locale === 'zh' ? active.title.zh.split('，').map((line, lineIndex) => <span key={line}>{line}{lineIndex === 0 ? '，' : ''}</span>) : active.title.en}</h3>
                <div className="landing-scenario-prompt"><span>{t('You could start with', '试着这样开始')}</span><p>“{text(active.prompt)}”</p></div>
              </div>
              <div className="landing-scenario-response">
                <div className="landing-scenario-response-label"><IconSpark width={17} height={17} />{t('A possible way forward', '把事情一步步理清')}</div>
                <ol>
                  {active.steps.map((step, stepIndex) => (
                    <li key={stepIndex}><span className="landing-scenario-step" aria-hidden="true">{stepIndex + 1}</span><div><h4>{text(step.title)}</h4><p>{text(step.detail)}</p></div></li>
                  ))}
                </ol>
                <span className="landing-scenario-endnote">{t('Your ideas. A little more direction.', '你的想法，多一点方向。')}<IconArrowRight width={15} height={15} /></span>
              </div>
            </> : null}
          </div>
        ))}
      </div>
    </section>
  );
}
