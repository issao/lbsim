import { loadTestInitial } from '../lib/config';
import { Dashboard } from './Dashboard';

export function LoadTest() {
  const initial = loadTestInitial(window.location.search, window.location.hash);
  return <Dashboard initial={initial} />;
}
