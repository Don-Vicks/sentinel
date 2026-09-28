//! Development-only traffic generator. Publishes synthetic transactions into
//! the Vortex hub so the full Sentinel pipeline can be exercised without a
//! gRPC key. Enabled with `SENTINEL_SIMULATE=<program_id>`; never on by default.
//! Every 4 minutes it injects a 60s slippage-failure burst, and between
//! bursts a small wave of a new error type.

use chrono::Utc;
use std::sync::Arc;
use vortex::events::logs::Invocation;
use vortex::events::{
    AccountRef, Instruction, TokenBalanceChange, Transfer, TransferKind, TxError, VortexTransaction,
};
use vortex::hub::VortexHub;

const MINT: &str = "Fk3sXcpGSr1Y8MNmeuwTVAvEx5uxUu7RfSjrpbYQpump";

fn key(prefix: &str, n: u64) -> String {
    // Base58-looking, fixed length; not a real key.
    let body = format!("{prefix}{n:0>12}").replace('0', "o").replace('l', "L");
    format!("{body:1<44}").chars().take(44).collect()
}

pub fn spawn(hub: Arc<VortexHub>, program: String) {
    tracing::warn!(%program, "SIMULATION MODE: publishing synthetic transactions (dev only)");
    tokio::spawn(async move {
        let mut n: u64 = 0;
        let started = Utc::now().timestamp();
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(125));
        loop {
            tick.tick().await;
            n += 1;
            let elapsed = Utc::now().timestamp() - started;
            let burst = elapsed > 180 && (elapsed - 180) % 240 < 60;
            let new_error_wave = elapsed > 180 && (elapsed - 180) % 240 >= 150 && (elapsed - 180) % 240 < 210;
            let seed = n.wrapping_mul(2654435761) % 1000;
            let fail = if burst { seed < 450 } else { seed < 25 };
            let account_error = !fail && new_error_wave && seed % 40 == 1;
            hub.record_slot(300_000_000 + n / 3);
            hub.publish(Arc::new(tx(&program, n, fail || account_error, seed, account_error)));
        }
    });
}

fn tx(program: &str, n: u64, fail: bool, seed: u64, account_error: bool) -> VortexTransaction {
    let (err_name, err_code, err_hex) = if account_error {
        ("AccountNotInitialized", 3012u32, "0xbc4")
    } else {
        ("TooLittleSolReceived", 6003u32, "0x1773")
    };
    let buy = seed % 2 == 0;
    let signer = key("Trader", seed % 180);
    let curve = key("Curve", 7);
    let curve_ata = key("CurveVault", 7);
    let user_ata = key("UserVault", seed % 180);
    let fee_acct = key("FeeRecipient", 1);
    let sol_amt = 0.05 + (seed as f64 / 1000.0) * 2.4;
    let tokens = sol_amt * 3_150_000.0;
    let cu = 38_000 + (seed * 31) % 22_000;
    let ix_name = if buy { "Buy" } else { "Sell" };
    let token = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
    let system = "11111111111111111111111111111111";

    let mut logs = vec![
        "Program ComputeBudget111111111111111111111111111111 invoke [1]".to_string(),
        "Program ComputeBudget111111111111111111111111111111 success".to_string(),
        format!("Program {program} invoke [1]"),
        format!("Program log: Instruction: {ix_name}"),
    ];
    let mut invocations = vec![
        Invocation {
            program_id: "ComputeBudget111111111111111111111111111111".into(),
            depth: 1,
            success: Some(true),
            ..Default::default()
        },
        Invocation {
            program_id: program.into(),
            depth: 1,
            instruction: Some(ix_name.into()),
            compute_consumed: Some(cu),
            compute_budget: Some(200_000),
            success: Some(!fail),
            failure: fail.then(|| format!("custom program error: {err_hex}")),
            ..Default::default()
        },
    ];
    let mut transfers = vec![];
    let mut balances = vec![];
    if fail {
        logs.push(format!("Program log: AnchorError caused by account: bonding_curve. Error Code: {err_name}. Error Number: {err_code}. Error Message: see IDL."));
        logs.push(format!("Program {program} consumed {cu} of 199850 compute units"));
        logs.push(format!("Program {program} failed: custom program error: {err_hex}"));
    } else {
        logs.push(format!("Program {token} invoke [2]"));
        logs.push("Program log: Instruction: Transfer".into());
        logs.push(format!("Program {token} consumed 4645 of 160000 compute units"));
        logs.push(format!("Program {token} success"));
        logs.push(format!("Program {system} invoke [2]"));
        logs.push(format!("Program {system} success"));
        logs.push(format!("Program {program} consumed {cu} of 199850 compute units"));
        logs.push(format!("Program {program} success"));
        invocations.push(Invocation {
            program_id: token.into(),
            depth: 2,
            parent: Some(1),
            instruction: Some("Transfer".into()),
            compute_consumed: Some(4645),
            success: Some(true),
            ..Default::default()
        });
        invocations.push(Invocation {
            program_id: system.into(),
            depth: 2,
            parent: Some(1),
            success: Some(true),
            ..Default::default()
        });
        let (tf, tt, tfo, tto) = if buy {
            (&curve_ata, &user_ata, &curve, &signer)
        } else {
            (&user_ata, &curve_ata, &signer, &curve)
        };
        transfers.push(Transfer {
            kind: TransferKind::Token,
            mint: Some(MINT.into()),
            from: Some(tf.clone()),
            to: Some(tt.clone()),
            from_owner: Some(tfo.clone()),
            to_owner: Some(tto.clone()),
            authority: Some(tfo.clone()),
            amount_raw: (tokens * 1e6) as u64,
            decimals: 6,
            amount: tokens,
            instruction: "2.0".into(),
        });
        let (sf, st) = if buy { (&signer, &curve) } else { (&curve, &signer) };
        transfers.push(Transfer {
            kind: TransferKind::Sol,
            mint: None,
            from: Some(sf.clone()),
            to: Some(st.clone()),
            from_owner: Some(sf.clone()),
            to_owner: Some(st.clone()),
            authority: None,
            amount_raw: (sol_amt * 1e9) as u64,
            decimals: 9,
            amount: sol_amt,
            instruction: "2.1".into(),
        });
        transfers.push(Transfer {
            kind: TransferKind::Sol,
            mint: None,
            from: Some(signer.clone()),
            to: Some(fee_acct.clone()),
            from_owner: Some(signer.clone()),
            to_owner: Some(fee_acct.clone()),
            authority: None,
            amount_raw: (sol_amt * 0.01 * 1e9) as u64,
            decimals: 9,
            amount: sol_amt * 0.01,
            instruction: "2.2".into(),
        });
        let sign = if buy { 1.0 } else { -1.0 };
        balances.push(TokenBalanceChange {
            account: user_ata.clone(),
            owner: Some(signer.clone()),
            mint: MINT.into(),
            decimals: 6,
            pre: 1_000_000.0,
            post: 1_000_000.0 + sign * tokens,
            delta: sign * tokens,
        });
        balances.push(TokenBalanceChange {
            account: curve_ata.clone(),
            owner: Some(curve.clone()),
            mint: MINT.into(),
            decimals: 6,
            pre: 500_000_000.0,
            post: 500_000_000.0 - sign * tokens,
            delta: -sign * tokens,
        });
    }

    let lamports = |base: u64, d: f64| (base as f64 + d * 1e9).max(0.0) as u64;
    let sol_d = if fail { 0.0 } else if buy { -sol_amt * 1.01 } else { sol_amt * 0.99 };
    let accounts = vec![
        AccountRef { pubkey: signer.clone(), signer: true, writable: true, from_lookup_table: false, pre_lamports: 5_000_000_000, post_lamports: lamports(5_000_000_000 - 5000, sol_d) },
        AccountRef { pubkey: curve.clone(), signer: false, writable: true, from_lookup_table: false, pre_lamports: 80_000_000_000, post_lamports: lamports(80_000_000_000, if fail { 0.0 } else if buy { sol_amt } else { -sol_amt }) },
        AccountRef { pubkey: curve_ata, signer: false, writable: true, from_lookup_table: false, pre_lamports: 2_039_280, post_lamports: 2_039_280 },
        AccountRef { pubkey: user_ata, signer: false, writable: true, from_lookup_table: false, pre_lamports: 2_039_280, post_lamports: 2_039_280 },
        AccountRef { pubkey: fee_acct, signer: false, writable: true, from_lookup_table: false, pre_lamports: 1_000_000_000, post_lamports: lamports(1_000_000_000, if fail { 0.0 } else { sol_amt * 0.01 }) },
        AccountRef { pubkey: program.into(), signer: false, writable: false, from_lookup_table: false, pre_lamports: 1, post_lamports: 1 },
    ];

    VortexTransaction {
        signature: format!("SiM{}{:0>60}", n, seed).chars().take(88).collect(),
        slot: 300_000_000 + n / 3,
        index: n % 400,
        received_at: Utc::now(),
        success: !fail,
        error: fail.then(|| TxError {
            message: format!("InstructionError(1, Custom({err_code}))"),
            instruction_index: Some(1),
            custom_code: Some(err_code),
            program_id: Some(program.into()),
            name: Some(err_name.into()),
            class: "Unknown".into(),
        }),
        fee: 5000 + seed * 40,
        compute_units: Some(cu + 450),
        compute_unit_limit: Some(200_000),
        compute_unit_price: Some(100_000 + seed * 500),
        instructions: vec![
            Instruction {
                path: "0".into(),
                top_index: 0,
                inner_index: None,
                stack_height: 1,
                program_id: "ComputeBudget111111111111111111111111111111".into(),
                program_name: Some("Compute Budget".into()),
                accounts: vec![],
                data: "3DdGGhkhJbjm".into(),
                name: Some("SetComputeUnitLimit".into()),
                parsed: Some(serde_json::json!({ "units": 200000 })),
            },
            Instruction {
                path: "1".into(),
                top_index: 1,
                inner_index: None,
                stack_height: 1,
                program_id: program.into(),
                program_name: vortex::events::programs::known_name(program).map(str::to_string),
                accounts: accounts.iter().take(5).map(|a| a.pubkey.clone()).collect(),
                data: "AJTQ2h9DXrBa3kDgQfNmpq".into(),
                name: Some(ix_name.into()),
                parsed: None,
            },
        ],
        accounts,
        invocations,
        logs,
        logs_truncated: false,
        token_balances: balances,
        transfers,
        filters: vec!["simulation".into()],
    }
}
