import bs58 from 'bs58';

export type Parsed = { kind: 'signature' | 'address'; value: string } | null;

const B58 = /^[1-9A-HJ-NP-Za-km-z]{32,88}$/;

/** What was pasted: a transaction signature or an address, also from an explorer link. Mirrors the server's parser. */
export function parseQuery(input: string): Parsed {
  const text = input.trim();
  if (!text) return null;
  let candidate = text;
  if (text.includes('/')) {
    const path = text.split(/[?#]/)[0].replace(/\/+$/, '');
    const seg = path
      .split('/')
      .reverse()
      .find((s) => B58.test(s));
    if (!seg) return null;
    candidate = seg;
  }
  if (!B58.test(candidate)) return null;
  try {
    const len = bs58.decode(candidate).length;
    if (len === 64) return { kind: 'signature', value: candidate };
    if (len === 32) return { kind: 'address', value: candidate };
  } catch {
    /* not base58 */
  }
  return null;
}
