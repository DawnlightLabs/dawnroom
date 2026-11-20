// Three one-time tips after the very first image: what to try next, each with its docs page.
// Shown once a print has developed; "Got it" (or opening one) puts them away for good.

import { useState } from 'react';
import { go, useApp, type Page } from '../app/state';
import { DOCS } from '../config';
import { cx, HowLink, Icon, type IconName } from './ui';

const KEY = 'studio.tips.seen';

const TIPS: { icon: IconName; title: string; line: string; docs: string; page?: Page }[] = [
  { icon: 'edit', title: 'Edit this', line: 'Under a print: change it with words, like "make it night".', docs: DOCS.edit },
  { icon: 'library', title: 'Library', line: 'Every print, with the recipe to make it again.', docs: DOCS.library, page: 'library' },
  { icon: 'models', title: 'Models', line: 'More models, other sizes, or one of your own.', docs: DOCS.models, page: 'models' },
];

function seen(): boolean {
  try {
    return localStorage.getItem(KEY) === '1';
  } catch {
    return false;
  }
}

export function FirstTips() {
  const developed = useApp((s) => s.groups.some((g) => g.prints.some((p) => p.status === 'completed')));
  const [hidden, setHidden] = useState(seen);
  if (hidden || !developed) return null;
  const done = () => {
    try {
      localStorage.setItem(KEY, '1');
    } catch {
      // private mode: they show again next time
    }
    setHidden(true);
  };
  return (
    <section aria-label="Next" className="enter flex items-stretch gap-2 rounded-card border border-line bg-surface p-2">
      {TIPS.map((t) => (
        <div key={t.title} className="flex min-w-0 flex-1 items-start gap-2.5 rounded-control px-2.5 py-2">
          <span className="mt-0.5 text-muted">
            <Icon name={t.icon} size={15} />
          </span>
          <span className="flex min-w-0 flex-col gap-0.5">
            {t.page ? (
              <button type="button" onClick={() => (done(), go(t.page!))} className="self-start rounded-[4px] text-[13px] font-medium hover:underline">
                {t.title}
              </button>
            ) : (
              <span className="text-[13px] font-medium">{t.title}</span>
            )}
            <span className="text-[12px] leading-snug text-muted">{t.line}</span>
            <HowLink href={t.docs} className="self-start text-[12px]" />
          </span>
        </div>
      ))}
      <button type="button" onClick={done} className={cx('flex-none self-center rounded-control px-3 py-1.5 text-[12.5px] text-muted hover:bg-raised hover:text-text')}>
        Got it
      </button>
    </section>
  );
}
