import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from 'react';
import { getWallets } from '@wallet-standard/app';
import type { Wallet, WalletAccount } from '@wallet-standard/base';
import bs58 from 'bs58';
import { api, send } from './api';
import { live } from './live';

type ConnectFeature = {
  'standard:connect': { connect(input?: { silent?: boolean }): Promise<{ accounts: readonly WalletAccount[] }> };
};
type SignMessageFeature = {
  'solana:signMessage': {
    signMessage(
      ...inputs: { account: WalletAccount; message: Uint8Array }[]
    ): Promise<readonly { signedMessage: Uint8Array; signature: Uint8Array }[]>;
  };
};
type DisconnectFeature = { 'standard:disconnect'?: { disconnect(): Promise<void> } };

/** Wallets that can connect and sign a message on Solana. */
function usable(w: Wallet): boolean {
  return (
    'standard:connect' in w.features &&
    'solana:signMessage' in w.features &&
    w.chains.some((c) => c.startsWith('solana:'))
  );
}

export function useWallets() {
  const [wallets, setWallets] = useState<Wallet[]>([]);
  useEffect(() => {
    const registry = getWallets();
    const update = () => setWallets(registry.get().filter(usable));
    update();
    const off1 = registry.on('register', update);
    const off2 = registry.on('unregister', update);
    return () => {
      off1();
      off2();
    };
  }, []);
  return wallets;
}

interface Me {
  account: string | null;
  watching: string[];
}

interface AuthCtx extends Me {
  loading: boolean;
  signIn(wallet: Wallet): Promise<void>;
  signOut(): Promise<void>;
  refresh(): Promise<void>;
  /** Opens the sign-in dialog (e.g. when a signed-out user clicks a gated action). */
  requestSignIn(): void;
  dialogOpen: boolean;
  closeDialog(): void;
}

const Ctx = createContext<AuthCtx | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [me, setMe] = useState<Me>({ account: null, watching: [] });
  const [loading, setLoading] = useState(true);
  const [dialogOpen, setDialogOpen] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setMe(await api<Me>('/api/auth/me'));
    } catch {
      setMe({ account: null, watching: [] });
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const signIn = useCallback(
    async (wallet: Wallet) => {
      const features = wallet.features as unknown as ConnectFeature & SignMessageFeature;
      const { accounts } = await features['standard:connect'].connect();
      const account = accounts.find((a) => a.chains.some((c) => c.startsWith('solana:'))) ?? accounts[0];
      if (!account) throw new Error(`${wallet.name} didn't share an account`);

      const { message } = await send<{ message: string }>('POST', '/api/auth/challenge', { pubkey: account.address });
      const [signed] = await features['solana:signMessage'].signMessage({
        account,
        message: new TextEncoder().encode(message),
      });
      await send('POST', '/api/auth/verify', {
        pubkey: account.address,
        message,
        signature: bs58.encode(signed.signature),
      });
      await refresh();
      live.reconnect();
      setDialogOpen(false);
      // The session lives in our cookie; the wallet connection isn't needed anymore.
      await (wallet.features as unknown as DisconnectFeature)['standard:disconnect']?.disconnect().catch(() => {});
    },
    [refresh],
  );

  const signOut = useCallback(async () => {
    await send('POST', '/api/auth/logout');
    setMe({ account: null, watching: [] });
    live.reconnect();
  }, []);

  return (
    <Ctx.Provider
      value={{
        ...me,
        loading,
        signIn,
        signOut,
        refresh,
        requestSignIn: () => setDialogOpen(true),
        dialogOpen,
        closeDialog: () => setDialogOpen(false),
      }}
    >
      {children}
    </Ctx.Provider>
  );
}

export function useAuth() {
  const c = useContext(Ctx);
  if (!c) throw new Error('useAuth outside AuthProvider');
  return c;
}
