import { useEffect, useState } from 'react';
import { Home } from './pages/Home';
import { LoadTest } from './pages/LoadTest';
import { Compare } from './pages/Compare';
import { Showcase } from './pages/Showcase';
import { useLeaseLifecycle } from './lib/useSubscriptions';

type Route = '/' | '/dashboard' | '/ab' | '/showcase';

const LINKS: { href: Route; label: string }[] = [
  { href: '/', label: 'Home' },
  { href: '/dashboard', label: 'Load test' },
  { href: '/ab', label: 'A/B' },
  { href: '/showcase', label: 'Showcase' },
];

function currentRoute(): Route {
  const h = window.location.hash.replace(/^#/, '') || '/';
  return (LINKS.find((l) => l.href === h)?.href ?? '/') as Route;
}

/** A hash router, hand-written. No router dependency for four routes. */
export function App() {
  const [route, setRoute] = useState<Route>(currentRoute);
  useLeaseLifecycle();

  useEffect(() => {
    const onHash = () => setRoute(currentRoute());
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);

  return (
    <div className="app">
      <header className="topbar">
        <span className="brand">
          lbsim <span>stand-in dashboard</span>
        </span>
        <nav className="navlinks">
          {LINKS.map((l) => (
            <a key={l.href} href={`#${l.href}`} aria-current={route === l.href ? 'page' : undefined}>
              {l.label}
            </a>
          ))}
        </nav>
        <div className="topbar-right">
          <span className="mock-global" title="Nothing here is connected to sim-ingress. Every number is generated in this browser.">
            mock data, no engine attached
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
