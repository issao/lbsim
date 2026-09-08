import { useEffect, useLayoutEffect, useState, useSyncExternalStore } from 'react';
import { Home } from './pages/Home';
import { LoadTest } from './pages/LoadTest';
import { Compare } from './pages/Compare';
import { Showcase } from './pages/Showcase';
import { useLeaseLifecycle } from './lib/useSubscriptions';
import { activeMode, badgeText, badgeTitle, setActiveMode, subscribeActiveMode } from './lib/mode';

type Route = '/' | '/dashboard' | '/ab' | '/showcase';

const LINKS: { href: Route; label: string }[] = [
  { href: '/', label: 'Home' },
  { href: '/dashboard', label: 'Load test' },
  { href: '/ab', label: 'A/B' },
  { href: '/showcase', label: 'Showcase' },
];

function currentRoute(): Route {
  // A page keeps its own state after `?` (`#/showcase?script=x`), so the route is the part before it.
  const h = window.location.hash.replace(/^#/, '').replace(/\?.*$/, '') || '/';
  return (LINKS.find((l) => l.href === h)?.href ?? '/') as Route;
}

/** A hash router, hand-written. No router dependency for four routes. */
export function App() {
  const [route, setRoute] = useState<Route>(currentRoute);
  useLeaseLifecycle();
  // The dashboard resolves its source after a probe; the badge follows it, and says nothing until
  // then, so it never shows a claim left over from whatever page was on screen before this one.
  const mode = useSyncExternalStore(subscribeActiveMode, activeMode, activeMode);

  // A layout effect, not a passive one: on mount, passive effects fire child-before-parent, so a
  // page's own mount effect (Compare's `setActiveMode('mock')`, say) would run before this one and
  // then get clobbered back to 'none'. All layout effects finish before any passive effect runs,
  // so this always lands first regardless of where in the tree the page's own effect sits.
  useLayoutEffect(() => {
    setActiveMode('none');
  }, []);

  // The shell routes pin the header and bars and let only the panels scroll; Home and the reports
  // keep ordinary page scrolling. The class lives on <html> because that is where the overflow rule
  // has to sit, and a layout effect so the first paint of a route is already in the right mode.
  useLayoutEffect(() => {
    document.documentElement.classList.toggle('shell', route !== '/');
  }, [route]);

  useEffect(() => {
    const onHash = () => {
      setActiveMode('none');
      setRoute(currentRoute());
    };
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);

  return (
    <div className="app">
      <header className="topbar">
        <span className="brand">
          lbsim <span>inference fleet simulator</span>
        </span>
        <nav className="navlinks">
          {LINKS.map((l) => (
            <a key={l.href} href={`#${l.href}`} aria-current={route === l.href ? 'page' : undefined}>
              {l.label}
            </a>
          ))}
        </nav>
        <div className="topbar-right">
          <span className="mock-global" title={badgeTitle(mode)}>
            {badgeText(mode)}
          </span>
        </div>
      </header>
      <main className="page">
        {route === '/' ? <Home /> : null}
        {route === '/dashboard' ? <LoadTest /> : null}
        {route === '/ab' ? <Compare /> : null}
        {route === '/showcase' ? <Showcase /> : null}
      </main>
    </div>
  );
}
