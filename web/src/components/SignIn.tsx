import { useEffect, useRef, useState } from 'react';
import type { Wallet } from '@wallet-standard/base';
import { LogOut, Wallet as WalletIcon, X } from 'lucide-react';
import { useAuth, useWallets } from '../lib/auth';
import { short } from '../lib/format';
import { Spinner } from './ui';

const INSTALL = [
  { name: 'Phantom', url: 'https://phantom.com/download' },
  { name: 'Solflare', url: 'https://solflare.com/download' },
  { name: 'Backpack', url: 'https://backpack.app/download' },
];

/** Wallet picker. Signing proves you own the address; nothing is sent on chain. */
export function SignInDialog() {
  const { dialogOpen, closeDialog, signIn } = useAuth();
  const wallets = useWallets();
  const ref = useRef<HTMLDialogElement>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (dialogOpen && !d.open) d.showModal();
    if (!dialogOpen && d.open) d.close();
    if (dialogOpen) setError(null);
  }, [dialogOpen]);

  const pick = async (w: Wallet) => {
    setBusy(w.name);
    setError(null);
    try {
      await signIn(w);
    } catch (e) {
      const msg = (e as Error).message || 'Sign-in failed';
      setError(/reject|denied|cancel/i.test(msg) ? 'You cancelled the request in your wallet.' : msg);
    } finally {
      setBusy(null);
    }
  };

  return (
    <dialog
      ref={ref}
      onClose={closeDialog}
      onClick={(e) => e.target === ref.current && closeDialog()}
      aria-labelledby="signin-title"
      className="m-auto w-[min(420px,calc(100vw-2rem))] rounded-xl border border-line-strong bg-surface p-0 text-ink backdrop:bg-black/50"
    >
      <div className="flex items-start justify-between gap-4 border-b border-line px-5 py-4">
        <div>
          <h2 id="signin-title" className="font-semibold">Sign in with your wallet</h2>
          <p className="mt-1 text-sm text-ink-2">
            Your wallet signs a one-time message to prove it's you. No transaction, no fee.
          </p>
        </div>
        <button className="inline-flex size-8 items-center justify-center rounded text-ink-3 hover:bg-sunken hover:text-ink" onClick={closeDialog} aria-label="Close">
          <X className="size-4" aria-hidden />
        </button>
      </div>
      <div className="p-3">
        {wallets.length === 0 ? (
          <div className="px-2 py-4 text-sm">
            <p className="text-ink-2">No Solana wallet found in this browser.</p>
            <p className="mt-3 text-ink-3">Install one, then reload:</p>
            <ul className="mt-2 flex gap-2">
              {INSTALL.map((w) => (
                <li key={w.name}>
                  <a className="btn h-8 text-xs" href={w.url} target="_blank" rel="noreferrer">
                    {w.name}
                  </a>
                </li>
              ))}
            </ul>
          </div>
        ) : (
          <ul className="space-y-1">
            {wallets.map((w) => (
              <li key={w.name}>
                <button
                  className="flex w-full items-center gap-3 rounded-md px-3 py-2.5 text-left hover:bg-sunken disabled:opacity-60"
                  onClick={() => pick(w)}
                  disabled={busy !== null}
                >
                  <img src={w.icon} alt="" className="size-7 rounded-md" />
                  <span className="font-medium">{w.name}</span>
                  <span className="ml-auto text-xs text-ink-3">{busy === w.name ? <Spinner /> : 'Sign in'}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
        {error && (
          <p className="mx-2 mt-2 rounded bg-crit-soft px-3 py-2 text-sm text-crit" role="alert">
            {error}
          </p>
        )}
      </div>
    </dialog>
  );
}

/** Sidebar footer: signed-in wallet or the sign-in button. */
export function AccountChip() {
  const { account, loading, requestSignIn, signOut } = useAuth();
  if (loading) return null;
  if (!account) {
    return (
      <button className="btn w-full" onClick={requestSignIn}>
        <WalletIcon className="size-4" aria-hidden />
        Sign in with wallet
      </button>
    );
  }
  return (
    <div className="flex items-center gap-2 rounded-md border border-line bg-surface px-3 py-2">
      <span className="size-2 rounded-full bg-good" aria-hidden />
      <span className="min-w-0">
        <span className="block text-[11px] text-ink-3">Signed in</span>
        <span className="block font-mono text-xs text-ink truncate" title={account}>{short(account, 5)}</span>
      </span>
      <button
        className="ml-auto inline-flex size-8 items-center justify-center rounded text-ink-3 hover:bg-sunken hover:text-ink"
        onClick={signOut}
        aria-label="Sign out"
        title="Sign out"
      >
        <LogOut className="size-4" aria-hidden />
      </button>
    </div>
  );
}

/** Renders children when signed in; otherwise a prompt to sign in. */
export function RequireAccount({ children, what }: { children: React.ReactNode; what: string }) {
  const { account, loading, requestSignIn } = useAuth();
  if (loading) return null;
  if (account) return <>{children}</>;
  return (
    <div className="panel flex flex-col items-center gap-3 px-6 py-10 text-center">
      <WalletIcon className="size-6 text-ink-3" aria-hidden />
      <div className="font-medium">Sign in to {what}</div>
      <p className="max-w-sm text-sm text-ink-3">
        Alerts, webhooks and watchlists belong to your wallet. Signing in costs nothing and sends no transaction.
      </p>
      <button className="btn-primary" onClick={requestSignIn}>
        <WalletIcon className="size-4" aria-hidden />
        Sign in with wallet
      </button>
    </div>
  );
}
