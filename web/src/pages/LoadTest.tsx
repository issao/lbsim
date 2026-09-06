import { BASE } from '../lib/config';
import { Dashboard } from './Dashboard';

export function LoadTest() {
  return <Dashboard initial={BASE} />;
}
