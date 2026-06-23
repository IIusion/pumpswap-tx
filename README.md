# pumpswap-tx

A minimal, dependency-light **instruction builder for the PumpSwap AMM**
    (Pump.fun's post-graduation AMM, `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA`).

    PumpSwap ships no public SDK for raw instruction construction, and the exact
    account ordering and instruction tails are undocumented. This crate encodes the
    **verified, on-chain-tested** layout so you can build `buy` / `sell` swaps against
    any PumpSwap pool — including **inverted pools** (`base = WSOL`).

    ## Why this exists

    Building a landing PumpSwap swap by hand means rediscovering a series of
    non-obvious, revert-on-mistake details. Each one below cost a failed mainnet
    transaction to nail down — they're handled for you here:

    - **Account tail** — the current on-chain layout requires
      `global_volume_accumulator`, `user_volume_accumulator`, `fee_config`,
      `fee_program`, and the buyback fee-recipient pair. Omit them → revert.
    - **Inverted pools** (`base = WSOL`) — buying the token is actually a `sell`
      instruction; selling it is `buy_exact_quote_in`. The orientation-agnostic
      `build_enter_ix` / `build_exit_ix` wrappers pick the right one.
    - **`buy_exact_quote_in` data length** must be exactly 24 bytes
      (`disc + 2×u64`); a trailing `track_volume` byte reverts with `ZeroBaseAmount`.
    - **`min_base_out >= 1`** — `0` drops into a degenerate program branch and
      reverts with `ZeroBaseAmount`.
    - **Fee / creator ATAs** are always derived on the **quote** mint, even on
      inverted pools.

    ## Usage

    ```rust
    use pumpswap_tx::{SwapContext, parse_pool, build_enter_ix, build_exit_ix, wsol_mint, token_program};
    use solana_sdk::pubkey::Pubkey;

    // 1. Fetch the pool account (getAccountInfo) and parse it.
    let (base, quote, base_vault, quote_vault, creator) = parse_pool(&pool_account_data).unwrap();
    let inverted = base == wsol_mint();

    // 2. Build a context.
    let ctx = SwapContext {
        pool,
        base_mint: base,
        quote_mint: quote,
        creator,
        pool_base_vault: base_vault,
        pool_quote_vault: quote_vault,
        base_token_program: token_program(),  // or token_2022_program() per the token's mint
        quote_token_program: token_program(),
        protocol_fee_recipient,               // read from the program's global config
        inverted,
    };

    // 3. Buy the token (spend up to 0.2 SOL), then later sell it back.
    let buy = build_enter_ix(&ctx, &user, /* amount_token_out */ 0, /* max_sol_in */ 200_000_000);
    let sell = build_exit_ix(&ctx, &user, /* amount_token_in */ tokens_held, /* min_sol_out */ 1);

    Wrap each instruction with a ComputeBudget price/limit and your ATA-create
    instructions, sign, and send. The builders return raw solana_sdk::Instruction
    values, so they compose with any sending path (RPC, Jito, etc.).
