//! # pumpswap-tx
    //!
    //! Minimal, dependency-light instruction builder for the **PumpSwap AMM**
    //! (Pump.fun's post-graduation AMM, program `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA`).
    //!
    //! PumpSwap has no public SDK for raw instruction construction, and the exact
    //! account ordering / instruction tails are undocumented. This crate encodes the
    //! **verified, on-chain-tested** layout so you can build `buy` / `sell` swaps
    //! against any PumpSwap pool — including **inverted pools** (where `base = WSOL`).
    //!
    //! Every layout decision below was validated against landed mainnet transactions.
    //! Notable footguns that are handled for you:
    //!
    //! - **Account tail** — current on-chain IDL requires `global_volume_accumulator`,
    //!   `user_volume_accumulator`, `fee_config`, `fee_program`, and the buyback
    //!   fee-recipient pair. Omitting them reverts.
    //! - **Inverted pools** (`base = WSOL`) — a "buy token" is actually a `sell`
    //!   instruction, and a "sell token" is `buy_exact_quote_in`. The
    //!   orientation-agnostic [`build_enter_ix`] / [`build_exit_ix`] wrappers handle this.
    //! - **`buy_exact_quote_in` data length** — must be exactly 24 bytes
    //!   (`disc + 2×u64`); a trailing `track_volume` byte causes `ZeroBaseAmount`.
    //! - **`min_base_out >= 1`** — a value of `0` drops into a degenerate program
    //!   branch and reverts with `ZeroBaseAmount`.
    //!
    //! ## Example
    //! ```no_run
    //! use pumpswap_tx::{SwapContext, build_enter_ix};
    //! use solana_sdk::pubkey::Pubkey;
    //!
    //! let ctx = SwapContext {
    //!     pool: Pubkey::new_unique(),
    //!     base_mint: Pubkey::new_unique(),
    //!     quote_mint: pumpswap_tx::wsol_mint(),
    //!     creator: Pubkey::new_unique(),
    //!     pool_base_vault: Pubkey::new_unique(),
    //!     pool_quote_vault: Pubkey::new_unique(),
    //!     base_token_program: pumpswap_tx::token_program(),
    //!     quote_token_program: pumpswap_tx::token_program(),
    //!     protocol_fee_recipient: Pubkey::new_unique(),
    //!     inverted: false,
    //! };
    //! let user = Pubkey::new_unique();
    //! // Buy: spend up to 0.2 SOL, request 1_000_000 base-token units.
    //! let ix = build_enter_ix(&ctx, &user, 1_000_000, 200_000_000);
    //! ```

    use std::str::FromStr;
    use solana_sdk::{
        instruction::{AccountMeta, Instruction},
        pubkey::Pubkey,
    };

    // ───────────────────────────── Program IDs ─────────────────────────────

    /// PumpSwap AMM program.
    pub const PUMPSWAP_PROGRAM: &str = "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA";
    /// PumpSwap global config account.
    pub const GLOBAL_CONFIG: &str = "ADyA8hdefvWN2dbGGWFotbzWxrAvLW83WG6QCVXvJKqw";
    /// Wrapped SOL mint.
    pub const WSOL_MINT: &str = "So11111111111111111111111111111111111111112";
    /// SPL Token program.
    pub const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
    /// SPL Token-2022 program.
    pub const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
    /// Associated Token Account program.
    pub const ATA_PROGRAM: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
    /// System program.
    pub const SYSTEM_PROGRAM: &str = "11111111111111111111111111111111";
    /// Pump.fun fee program (owns `fee_config` / buyback logic).
    pub const FEE_PROGRAM: &str = "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ";

    /// Anchor discriminator for `buy`.
    pub const BUY_DISC: [u8; 8] = [102, 6, 61, 18, 1, 218, 235, 234];
    /// Anchor discriminator for `sell`.
    pub const SELL_DISC: [u8; 8] = [51, 230, 133, 164, 1, 127, 131, 173];
    /// Anchor discriminator for `buy_exact_quote_in`.
    pub const BUY_EXACT_QUOTE_IN_DISC: [u8; 8] = [198, 46, 21, 82, 180, 217, 232, 112];

    /// Buyback fee-recipient pool. The program checks *membership* in this set
    /// (not a 1:1 pairing with `protocol_fee_recipient`), so any member is valid.
    const PROTOCOL_EXTRA_FEE_RECIPIENTS: [&str; 8] = [
        "5YxQFdt3Tr9zJLvkFccqXVUwhdTWJQc1fFg2YPbxvxeD",
        "9M4giFFMxmFGXtc3feFzRai56WbBqehoSeRE5GK7gf7",
        "GXPFM2caqTtQYC2cJ5yJRi9VDkpsYZXzYdwYpGnLmtDL",
        "3BpXnfJaUTiwXnJNe7Ej1rcbzqTTQUvLShZaWazebsVR",
        "5cjcW9wExnJJiqgLjq7DEG75Pm6JBgE1hNv4B2vHXUW6",
        "EHAAiTxcdDwQ3U4bU6YcMsQGaekdzLS3B5SmYo46kJtL",
        "5eHhjP8JaYkz83CWwvGU2uMUXefd3AazWGx4gpcuEEYD",
        "A7hAgCzFw14fejgCp387JUJRMNyz4j89JKnhtKU8piqW",
    ];

    #[inline]
    fn pk(s: &str) -> Pubkey {
        Pubkey::from_str(s).expect("hardcoded pubkey is valid")
    }

    /// WSOL mint as a [`Pubkey`].
    pub fn wsol_mint() -> Pubkey { pk(WSOL_MINT) }
    /// SPL Token program as a [`Pubkey`].
    pub fn token_program() -> Pubkey { pk(TOKEN_PROGRAM) }
    /// SPL Token-2022 program as a [`Pubkey`].
    pub fn token_2022_program() -> Pubkey { pk(TOKEN_2022_PROGRAM) }

    // ───────────────────────────── Swap context ─────────────────────────────

    /// Everything needed to build a swap against one PumpSwap pool.
    ///
    /// `base_mint` / `quote_mint` and the vaults are taken **from the pool account**
    /// (see [`parse_pool`]) because pools may be *inverted* (`base = WSOL`). Set
    /// [`inverted`](SwapContext::inverted) to `true` when `base_mint == WSOL`.
    #[derive(Debug, Clone)]
    pub struct SwapContext {
        /// Pool account.
        pub pool: Pubkey,
        /// Pool's base mint (may be WSOL on an inverted pool).
        pub base_mint: Pubkey,
        /// Pool's quote mint.
        pub quote_mint: Pubkey,
        /// Pool creator (`coin_creator`) — used to derive the creator-vault PDA.
        pub creator: Pubkey,
        /// Pool's base token vault.
        pub pool_base_vault: Pubkey,
        /// Pool's quote token vault.
        pub pool_quote_vault: Pubkey,
        /// Token program owning the base mint (Token vs Token-2022).
        pub base_token_program: Pubkey,
        /// Token program owning the quote mint.
        pub quote_token_program: Pubkey,
        /// Protocol fee recipient (account #9).
        pub protocol_fee_recipient: Pubkey,
        /// `true` when `base_mint == WSOL` (inverted pool).
        pub inverted: bool,
    }

    // ───────────────────────────── Pool parsing ─────────────────────────────

    /// Parse a PumpSwap `Pool` account (Anchor layout).
    ///
    /// Layout: `8 disc + 1 bump + 2 index + 32 creator + 32 base_mint +
    /// 32 quote_mint + 32 lp_mint + 32 pool_base_token_account +
    /// 32 pool_quote_token_account + 8 lp_supply + 32 coin_creator`.
    ///
    /// Returns `(base_mint, quote_mint, pool_base_vault, pool_quote_vault, coin_creator)`.
    pub fn parse_pool(data: &[u8]) -> Option<(Pubkey, Pubkey, Pubkey, Pubkey, Pubkey)> {
        let g = |o: usize| -> Option<Pubkey> {
            let a: [u8; 32] = data.get(o..o + 32)?.try_into().ok()?;
            Some(Pubkey::new_from_array(a))
        };
        let base = g(43)?;
        let quote = g(75)?;
        let pool_base_vault = g(139)?;
        let pool_quote_vault = g(171)?;
        let coin_creator = g(211)?;
        Some((base, quote, pool_base_vault, pool_quote_vault, coin_creator))
    }

    // ───────────────────────────── PDA derivations ─────────────────────────────

    fn derive_ata(wallet: &Pubkey, token_program: &Pubkey, mint: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[wallet.as_ref(), token_program.as_ref(), mint.as_ref()],
            &pk(ATA_PROGRAM),
        ).0
    }

    fn event_authority() -> Pubkey {
        Pubkey::find_program_address(&[b"__event_authority"], &pk(PUMPSWAP_PROGRAM)).0
    }

    fn creator_vault_authority(coin_creator: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[b"creator_vault", coin_creator.as_ref()],
            &pk(PUMPSWAP_PROGRAM),
        ).0
    }

    fn global_volume_accumulator() -> Pubkey {
        Pubkey::find_program_address(&[b"global_volume_accumulator"], &pk(PUMPSWAP_PROGRAM)).0
    }

    fn user_volume_accumulator(user: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[b"user_volume_accumulator", user.as_ref()],
            &pk(PUMPSWAP_PROGRAM),
        ).0
    }

    fn fee_config() -> Pubkey {
        Pubkey::find_program_address(
            &[b"fee_config", pk(PUMPSWAP_PROGRAM).as_ref()],
            &pk(FEE_PROGRAM),
        ).0
    }

    /// Buyback fee recipient + its ATA (on the quote mint). Any pool member is
    /// accepted by the program; we pick deterministically from `base_mint`.
    fn buyback_pair(ctx: &SwapContext) -> (Pubkey, Pubkey) {
        let idx = (ctx.base_mint.to_bytes()[0] as usize) % PROTOCOL_EXTRA_FEE_RECIPIENTS.len();
        let recipient = pk(PROTOCOL_EXTRA_FEE_RECIPIENTS[idx]);
        let ata = derive_ata(&recipient, &ctx.quote_token_program, &ctx.quote_mint);
        (recipient, ata)
    }

    // ───────────────────────────── Account / data layout ─────────────────────────────

    /// Canonical 19-account prefix shared by `buy` / `sell` / `buy_exact_quote_in`.
    fn base_accounts(ctx: &SwapContext, user: &Pubkey) -> Vec<AccountMeta> {
        let user_base_ata = derive_ata(user, &ctx.base_token_program, &ctx.base_mint);
        let user_quote_ata = derive_ata(user, &ctx.quote_token_program, &ctx.quote_mint);
        // Fee/creator ATAs are ALWAYS on the quote mint (verified on inverted pools).
        let fee_recipient_ata =
            derive_ata(&ctx.protocol_fee_recipient, &ctx.quote_token_program, &ctx.quote_mint);
        let cv_auth = creator_vault_authority(&ctx.creator);
        let cv_ata = derive_ata(&cv_auth, &ctx.quote_token_program, &ctx.quote_mint);

        vec![
            AccountMeta::new(ctx.pool, false),                            // 0  pool (W)
            AccountMeta::new(*user, true),                               // 1  user (W,S)
            AccountMeta::new_readonly(pk(GLOBAL_CONFIG), false),         // 2  global_config
            AccountMeta::new_readonly(ctx.base_mint, false),             // 3  base_mint
            AccountMeta::new_readonly(ctx.quote_mint, false),            // 4  quote_mint
            AccountMeta::new(user_base_ata, false),                      // 5  user_base_ata
            AccountMeta::new(user_quote_ata, false),                     // 6  user_quote_ata
            AccountMeta::new(ctx.pool_base_vault, false),                // 7  pool_base_vault
            AccountMeta::new(ctx.pool_quote_vault, false),               // 8  pool_quote_vault
            AccountMeta::new_readonly(ctx.protocol_fee_recipient, false),// 9  protocol_fee_recipient
            AccountMeta::new(fee_recipient_ata, false),                  // 10 fee_recipient_ata
            AccountMeta::new_readonly(ctx.base_token_program, false),    // 11 base_token_program
            AccountMeta::new_readonly(ctx.quote_token_program, false),   // 12 quote_token_program
            AccountMeta::new_readonly(pk(SYSTEM_PROGRAM), false),        // 13 system_program
            AccountMeta::new_readonly(pk(ATA_PROGRAM), false),           // 14 associated_token_program
            AccountMeta::new_readonly(event_authority(), false),         // 15 event_authority
            AccountMeta::new_readonly(pk(PUMPSWAP_PROGRAM), false),      // 16 program
            AccountMeta::new(cv_ata, false),                            // 17 coin_creator_vault_ata
            AccountMeta::new_readonly(cv_auth, false),                  // 18 coin_creator_vault_authority
        ]
    }

    fn ix_data(disc: [u8; 8], arg1: u64, arg2: u64) -> Vec<u8> {
        let mut d = Vec::with_capacity(24);
        d.extend_from_slice(&disc);
        d.extend_from_slice(&arg1.to_le_bytes());
        d.extend_from_slice(&arg2.to_le_bytes());
        d
    }

    // ───────────────────────────── Instruction builders ─────────────────────────────

    /// `buy`: receive `base_amount_out` base-token units, spending at most
    /// `max_quote_amount_in` quote (SOL) units.
    pub fn build_buy_ix(
        ctx: &SwapContext,
        user: &Pubkey,
        base_amount_out: u64,
        max_quote_amount_in: u64,
    ) -> Instruction {
        let mut accounts = base_accounts(ctx, user);
        accounts.push(AccountMeta::new_readonly(global_volume_accumulator(), false)); // 19
        accounts.push(AccountMeta::new(user_volume_accumulator(user), false));        // 20
        accounts.push(AccountMeta::new_readonly(fee_config(), false));                // 21
        accounts.push(AccountMeta::new_readonly(pk(FEE_PROGRAM), false));             // 22
        let (bb_recipient, bb_ata) = buyback_pair(ctx);
        accounts.push(AccountMeta::new_readonly(bb_recipient, false));                // 23
        accounts.push(AccountMeta::new(bb_ata, false));                              // 24
        Instruction {
            program_id: pk(PUMPSWAP_PROGRAM),
            accounts,
            data: ix_data(BUY_DISC, base_amount_out, max_quote_amount_in),
        }
    }

    /// `sell`: sell `base_amount_in` base-token units, receiving at least
    /// `min_quote_amount_out` quote (SOL) units.
    pub fn build_sell_ix(
        ctx: &SwapContext,
        user: &Pubkey,
        base_amount_in: u64,
        min_quote_amount_out: u64,
    ) -> Instruction {
        let mut accounts = base_accounts(ctx, user);
        accounts.push(AccountMeta::new_readonly(fee_config(), false));    // 19
        accounts.push(AccountMeta::new_readonly(pk(FEE_PROGRAM), false)); // 20
        let (bb_recipient, bb_ata) = buyback_pair(ctx);
        accounts.push(AccountMeta::new_readonly(bb_recipient, false));    // 21
        accounts.push(AccountMeta::new(bb_ata, false));                  // 22
        Instruction {
            program_id: pk(PUMPSWAP_PROGRAM),
            accounts,
            data: ix_data(SELL_DISC, base_amount_in, min_quote_amount_out),
        }
    }

    /// `buy_exact_quote_in`: spend exactly `spendable_quote_in` quote units,
    /// receive at least `min_base_out` base units.
    ///
    /// Two hard-won constraints are enforced here:
    /// - data must be exactly 24 bytes (no trailing `track_volume` byte), and
    /// - `min_base_out` is clamped to `>= 1` (a `0` reverts with `ZeroBaseAmount`).
    pub fn build_buy_exact_quote_in_ix(
        ctx: &SwapContext,
        user: &Pubkey,
        spendable_quote_in: u64,
        min_base_out: u64,
    ) -> Instruction {
        let mut accounts = base_accounts(ctx, user);
        accounts.push(AccountMeta::new_readonly(global_volume_accumulator(), false)); // 19
        accounts.push(AccountMeta::new(user_volume_accumulator(user), false));        // 20
        accounts.push(AccountMeta::new_readonly(fee_config(), false));                // 21
        accounts.push(AccountMeta::new_readonly(pk(FEE_PROGRAM), false));             // 22
        let (bb_recipient, bb_ata) = buyback_pair(ctx);
        accounts.push(AccountMeta::new_readonly(bb_recipient, false));                // 23
        accounts.push(AccountMeta::new(bb_ata, false));                              // 24

        let min_base_out = min_base_out.max(1);
        let mut data = Vec::with_capacity(24);
        data.extend_from_slice(&BUY_EXACT_QUOTE_IN_DISC);
        data.extend_from_slice(&spendable_quote_in.to_le_bytes());
        data.extend_from_slice(&min_base_out.to_le_bytes());
        Instruction { program_id: pk(PUMPSWAP_PROGRAM), accounts, data }
    }

    // ───────────────── Orientation-agnostic wrappers (token in / token out) ─────────────────

    /// **Enter** a position (buy the token with SOL), correct for either pool orientation.
    ///
    /// - Normal pool (`token = base`): `buy(amount_token_out, max_sol_in)`.
    /// - Inverted pool (`token = quote`, `WSOL = base`): `sell(max_sol_in, amount_token_out)`.
    pub fn build_enter_ix(
        ctx: &SwapContext,
        user: &Pubkey,
        amount_token_out: u64,
        max_sol_in: u64,
    ) -> Instruction {
        if !ctx.inverted {
            build_buy_ix(ctx, user, amount_token_out, max_sol_in)
        } else {
            build_sell_ix(ctx, user, max_sol_in, amount_token_out)
        }
    }

    /// **Exit** a position (sell the token for SOL), correct for either pool orientation.
    ///
    /// - Normal pool: `sell(amount_token_in, min_sol_out)`.
    /// - Inverted pool: `buy_exact_quote_in(amount_token_in, min_sol_out)`.
    pub fn build_exit_ix(
        ctx: &SwapContext,
        user: &Pubkey,
        amount_token_in: u64,
        min_sol_out: u64,
    ) -> Instruction {
        if !ctx.inverted {
            build_sell_ix(ctx, user, amount_token_in, min_sol_out)
        } else {
            build_buy_exact_quote_in_ix(ctx, user, amount_token_in, min_sol_out)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn ctx() -> SwapContext {
            SwapContext {
                pool: Pubkey::new_unique(),
                base_mint: Pubkey::new_unique(),
                quote_mint: wsol_mint(),
                creator: Pubkey::new_unique(),
                pool_base_vault: Pubkey::new_unique(),
                pool_quote_vault: Pubkey::new_unique(),
                base_token_program: token_program(),
                quote_token_program: token_program(),
                protocol_fee_recipient: Pubkey::new_unique(),
                inverted: false,
            }
        }

        #[test]
        fn buy_layout_is_25_accounts_with_correct_args() {
            let ix = build_buy_ix(&ctx(), &Pubkey::new_unique(), 1_000_000, 206_000_000);
            assert_eq!(ix.accounts.len(), 25);
            assert_eq!(&ix.data[0..8], &BUY_DISC);
            assert_eq!(u64::from_le_bytes(ix.data[8..16].try_into().unwrap()), 1_000_000);
            assert_eq!(u64::from_le_bytes(ix.data[16..24].try_into().unwrap()), 206_000_000);
        }

        #[test]
        fn sell_layout_is_23_accounts_with_correct_args() {
            let ix = build_sell_ix(&ctx(), &Pubkey::new_unique(), 250_000, 95_000_000);
            assert_eq!(ix.accounts.len(), 23);
            assert_eq!(&ix.data[0..8], &SELL_DISC);
            assert_eq!(u64::from_le_bytes(ix.data[8..16].try_into().unwrap()), 250_000);
            assert_eq!(u64::from_le_bytes(ix.data[16..24].try_into().unwrap()), 95_000_000);
        }

        #[test]
        fn buy_exact_quote_in_data_is_exactly_24_bytes() {
            let ix = build_buy_exact_quote_in_ix(&ctx(), &Pubkey::new_unique(), 1_000_000_000, 1);
            assert_eq!(ix.accounts.len(), 25);
            assert_eq!(ix.data.len(), 24, "trailing track_volume byte reverts with ZeroBaseAmount");
            assert_eq!(&ix.data[0..8], &BUY_EXACT_QUOTE_IN_DISC);
        }

        #[test]
        fn buy_exact_quote_in_clamps_min_base_out_to_one() {
            let ix = build_buy_exact_quote_in_ix(&ctx(), &Pubkey::new_unique(), 1_000_000_000, 0);
            // min_base_out of 0 would drop into a degenerate branch → ZeroBaseAmount.
            assert_eq!(u64::from_le_bytes(ix.data[16..24].try_into().unwrap()), 1);
        }

        #[test]
        fn enter_uses_buy_on_normal_pool_sell_on_inverted() {
            let user = Pubkey::new_unique();
            let normal = ctx();
            assert_eq!(&build_enter_ix(&normal, &user, 1_000, 100).data[0..8], &BUY_DISC);

            let mut inverted = ctx();
            inverted.inverted = true;
            assert_eq!(&build_enter_ix(&inverted, &user, 1_000, 100).data[0..8], &SELL_DISC);
        }

        #[test]
        fn exit_uses_sell_on_normal_pool_buy_exact_on_inverted() {
            let user = Pubkey::new_unique();
            let normal = ctx();
            assert_eq!(&build_exit_ix(&normal, &user, 1_000, 1).data[0..8], &SELL_DISC);

            let mut inverted = ctx();
            inverted.inverted = true;
            assert_eq!(&build_exit_ix(&inverted, &user, 1_000, 1).data[0..8], &BUY_EXACT_QUOTE_IN_DISC);
        }

        #[test]
        fn parse_pool_rejects_short_data() {
            assert!(parse_pool(&[0u8; 100]).is_none());
        }
    }
