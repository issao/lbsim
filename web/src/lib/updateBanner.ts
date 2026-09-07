import type { UpdateResponse } from './types';

/**
 * U57 (found by U28): `UpdateBanner` only ever branched on `requiredResimulation`, so a refused
 * update -- replay mode refusing a physics change, or the live server answering 501 -- rendered
 * `required_resimulation = false ... nothing was re-simulated`, which reads as success. `accepted`
 * is checked first, ahead of everything else, so a rejection can never fall through to either
 * success wording.
 */
export interface UpdateBannerText {
  kind: 'rejected' | 'resim' | 'applied';
  headline: string;
  detail: string;
}

export function updateBannerText(u: UpdateResponse): UpdateBannerText {
  if (!u.accepted) {
    return { kind: 'rejected', headline: 'not applied', detail: u.rejectedReason };
  }
  const changed = u.changed.join(', ');
  if (u.requiredResimulation) {
    return {
      kind: 'resim',
      headline: 'required_resimulation = true',
      detail: `${changed} changed. Physics changed, so the run rewound to the ${u.rewoundToS.toFixed(0)} s snapshot and re-simulated from there. History after that point is new.`,
    };
  }
  return {
    kind: 'applied',
    headline: 'required_resimulation = false',
    detail: `${changed} changed. View only: nothing was re-simulated, the panels re-derived from the recording.`,
  };
}
