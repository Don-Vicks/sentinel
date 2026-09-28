import { Link } from 'react-router';
import { CheckCircle2, XCircle } from 'lucide-react';
import type { TxSummary } from '../lib/types';
import { clock, compact, short, usd } from '../lib/format';

export function TxTable({ rows, fresh }: { rows: TxSummary[]; fresh?: Set<string> }) {
  return (
    <div className="overflow-x-auto">
      <table className="table">
        <thead>
          <tr>
            <th>Signature</th>
            <th>Time</th>
            <th>Result</th>
            <th>Instruction</th>
            <th className="text-right">Compute</th>
            <th>Signer</th>
            <th className="text-right">Largest transfer</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((t) => (
            <tr key={t.signature} className={fresh?.has(t.signature) ? 'row-enter' : ''}>
              <td>
                <Link to={`/tx/${t.signature}`} className="link font-mono text-xs whitespace-nowrap">
                  {short(t.signature, 6)}
                </Link>
              </td>
              <td className="num text-xs text-ink-2 whitespace-nowrap">{clock(t.received_at)}</td>
              <td className="whitespace-nowrap">
                {t.success ? (
                  <span className="inline-flex items-center gap-1 text-xs text-good">
                    <CheckCircle2 className="size-3.5" aria-hidden /> Success
                  </span>
                ) : (
                  <span className="inline-flex items-center gap-1 text-xs text-crit" title={t.error ?? undefined}>
                    <XCircle className="size-3.5" aria-hidden />
                    <span className="max-w-48 truncate">{t.error ?? 'Failed'}</span>
                  </span>
                )}
              </td>
              <td className="text-xs text-ink-2 max-w-40 truncate">{t.instructions.join(', ') || '—'}</td>
              <td className="num text-xs text-right">{compact(t.compute_units)}</td>
              <td className="font-mono text-xs text-ink-2">{short(t.fee_payer)}</td>
              <td className="num text-xs text-right whitespace-nowrap">
                {t.largest_transfer ? (
                  <>
                    {compact(t.largest_transfer.amount)} {t.largest_transfer.symbol}
                    {usd(t.largest_transfer.usd) && <span className="block text-ink-3">{usd(t.largest_transfer.usd)}</span>}
                  </>
                ) : (
                  '—'
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
