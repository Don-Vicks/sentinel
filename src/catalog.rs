//! Well-known Solana programs people ask to watch. Every entry was checked on mainnet: the
//! address is an executable program, and for Anchor programs the on-chain IDL name matches the
//! label (`cargo run --example idl_names`). Programs with no recent traffic (Drift, Marinade,
//! Phoenix, Zeta, Moonshot, Realms, CCTP at the time of writing) are left out so nothing in the
//! catalog sits at "No traffic yet".

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Entry {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    /// One line on what the program does.
    pub about: &'static str,
    /// Rough traffic, so a demo can mix busy and quiet programs: "busy", "moderate" or "quiet".
    pub load: &'static str,
}

const fn e(id: &'static str, name: &'static str, category: &'static str, about: &'static str, load: &'static str) -> Entry {
    Entry { id, name, category, about, load }
}

pub const CATALOG: &[Entry] = &[
    e("JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4", "Jupiter v6", "Swaps", "The main swap aggregator; routes through other DEXes", "busy"),
    e("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc", "Orca Whirlpool", "Swaps", "Concentrated-liquidity AMM", "busy"),
    e("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK", "Raydium CLMM", "Swaps", "Raydium's concentrated-liquidity pools", "busy"),
    e("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C", "Raydium CPMM", "Swaps", "Raydium's constant-product pools", "moderate"),
    e("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8", "Raydium AMM v4", "Swaps", "The original Raydium AMM", "moderate"),
    e("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo", "Meteora DLMM", "Swaps", "Dynamic liquidity market maker with bins", "busy"),
    e("cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG", "Meteora DAMM v2", "Swaps", "Meteora's dynamic AMM, version 2", "busy"),
    e("Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB", "Meteora Pools", "Swaps", "Meteora's original dynamic pools", "quiet"),
    e("opnb2LAfJYbRMAHHvqjCwQxanZn7ReEHp1k81EohpZb", "OpenBook v2", "Swaps", "On-chain order book", "quiet"),
    e("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P", "Pump.fun", "Launchpads", "Bonding-curve token launches", "busy"),
    e("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA", "PumpSwap AMM", "Launchpads", "AMM that graduated Pump.fun tokens trade on (very high volume)", "busy"),
    e("LanMV9sAd7wArD4vJFi2qDdfnVhFxYSUg6eADduJ3uj", "Raydium LaunchLab", "Launchpads", "Raydium's token launchpad", "quiet"),
    e("dbcij3LWUppWqq96dh6gJWwBifmcGfLSB5D4DuSMaqN", "Meteora DBC", "Launchpads", "Dynamic bonding curve launches", "moderate"),
    e("KLend2g3cP87fffoy8q1mQqGKjrxjC8boSyAYavgmjD", "Kamino Lend", "Lending and perps", "Money market and leverage", "quiet"),
    e("MFv2hWf31Z9kbCa1snEPYctwafyhdvnV7FZnsebVacA", "MarginFi v2", "Lending and perps", "Lending protocol", "quiet"),
    e("PERPHjGBqRHArX4DySjwM6UJHiR3sWAatqfdBS2qQJu", "Jupiter Perps", "Lending and perps", "Perpetuals exchange", "quiet"),
    e("SPoo1Ku8WFXoNDMHPsrGSTSG1Y47rzgn41SLUNakuHy", "SPL Stake Pool", "Staking", "The stake pool program behind most liquid staking tokens", "quiet"),
    e("rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ", "Pyth Receiver", "Oracles", "Receives verified Pyth price updates", "quiet"),
    e("pythWSnswVUd12oZpeFP8e9CVaEqJg25g1Vtc2biRsT", "Pyth Push Oracle", "Oracles", "Pyth price feeds pushed on chain", "quiet"),
    e("CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d", "Metaplex Core", "NFTs and assets", "Lightweight NFT standard", "moderate"),
    e("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s", "Metaplex Token Metadata", "NFTs and assets", "Names and images for tokens and NFTs", "quiet"),
    e("M2mx93ekt1fmXSVkTrUL9xVFHkmME8HTUi5Cyc5aF7K", "Magic Eden v2", "NFTs and assets", "NFT marketplace", "moderate"),
    e("SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf", "Squads v4", "Infrastructure", "Multisig wallets that hold most protocol treasuries", "quiet"),
    e("strmRqUCoQUgGUan5YhzUZa6KqdzwX5L6FpUxfmKg5m", "Streamflow", "Infrastructure", "Token vesting and payment streams", "quiet"),
    e("worm2ZoG2kUd4vFXhvjh93UUH596ayRfgQ2MgjNMTth", "Wormhole Core", "Infrastructure", "Cross-chain messaging", "quiet"),
];

/// Names for programs the monitored list and Vortex don't know, used in tables and traces.
pub fn name_of(program_id: &str) -> Option<&'static str> {
    CATALOG.iter().find(|e| e.id == program_id).map(|e| e.name)
}

/// A balanced set that stays inside what one stream can carry on a normal connection: a few
/// busy, well-known programs plus a spread of quieter ones across every category. Meteora DLMM
/// and the Pump programs are left out on purpose: their messages are large and bursty (DLMM
/// alone is about 2 MB/s), and they push a home connection behind the chain.
pub fn showcase() -> Vec<&'static Entry> {
    const IDS: &[&str] = &[
        "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4", // Jupiter v6
        "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc", // Orca Whirlpool
        "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C", // Raydium CPMM
        "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8", // Raydium AMM v4
        "dbcij3LWUppWqq96dh6gJWwBifmcGfLSB5D4DuSMaqN", // Meteora DBC
        "KLend2g3cP87fffoy8q1mQqGKjrxjC8boSyAYavgmjD", // Kamino Lend
        "MFv2hWf31Z9kbCa1snEPYctwafyhdvnV7FZnsebVacA", // MarginFi v2
        "PERPHjGBqRHArX4DySjwM6UJHiR3sWAatqfdBS2qQJu", // Jupiter Perps
        "rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ", // Pyth Receiver
        "CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d", // Metaplex Core
        "M2mx93ekt1fmXSVkTrUL9xVFHkmME8HTUi5Cyc5aF7K", // Magic Eden v2
        "SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf", // Squads v4
        "strmRqUCoQUgGUan5YhzUZa6KqdzwX5L6FpUxfmKg5m", // Streamflow
        "worm2ZoG2kUd4vFXhvjh93UUH596ayRfgQ2MgjNMTth", // Wormhole Core
        "SPoo1Ku8WFXoNDMHPsrGSTSG1Y47rzgn41SLUNakuHy", // SPL Stake Pool
    ];
    IDS.iter().filter_map(|id| CATALOG.iter().find(|e| e.id == *id)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::str::FromStr;

    #[test]
    fn every_entry_is_a_valid_unique_program_address() {
        let mut seen = HashSet::new();
        for e in CATALOG {
            solana_sdk::pubkey::Pubkey::from_str(e.id).unwrap_or_else(|_| panic!("{} has an invalid address", e.name));
            assert!(seen.insert(e.id), "{} is listed twice", e.name);
            assert!(["busy", "moderate", "quiet"].contains(&e.load), "{} has load {:?}", e.name, e.load);
        }
    }

    #[test]
    fn the_showcase_is_part_of_the_catalog_and_spans_categories() {
        let set = showcase();
        assert_eq!(set.len(), 15, "an ID in the showcase isn't in the catalog");
        let categories: HashSet<_> = set.iter().map(|e| e.category).collect();
        assert!(categories.len() >= 6, "{categories:?}");
    }
}
