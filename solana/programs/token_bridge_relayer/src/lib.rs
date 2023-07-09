use anchor_lang::{
    prelude::*,
    system_program::{self, Transfer},
};
use anchor_spl::{
    token::{self, spl_token},
};
//use solana_program::{bpf_loader_upgradeable, program::invoke};

pub use context::*;
pub use error::*;
pub use message::*;
pub use state::*;
pub use native_program::*;

pub mod context;
pub mod error;
pub mod message;
pub mod state;
pub mod native_program;

declare_id!("5S5LeEiouw4AdyUXBoDThpsepQha2HH8Qt5AMDn9zsk1");

#[program]
pub mod token_bridge_relayer {
    use super::*;
    use wormhole_anchor_sdk::{token_bridge, wormhole};

    /// This instruction is be used to generate your program's config.
    /// And for convenience, we will store Wormhole-related PDAs in the
    /// config so we can verify these accounts with a simple == constraint.
    /// # Arguments
    ///
    /// * `ctx`           - `Initialize` context
    /// * `fee_recipient` - Recipient of all relayer fees and swap proceeds
    /// * `assistant`     - Priviledged key to manage certain accounts
    pub fn initialize(
        ctx: Context<Initialize>,
        fee_recipient: Pubkey,
        assistant: Pubkey
    ) -> Result<()> {
        require!(
            fee_recipient != Pubkey::default() &&
            assistant != Pubkey::default(),
            TokenBridgeRelayerError::InvalidPublicKey
        );

        // Initial precision value for both relayer fees and swap rates.
        let initial_precision: u32 = 100000000;

        // Initialize program's sender config.
        let sender_config = &mut ctx.accounts.sender_config;

        // Set the owner of the sender config (effectively the owner of the
        // program).
        sender_config.owner = ctx.accounts.owner.key();
        sender_config.bump = *ctx
            .bumps
            .get("sender_config")
            .ok_or(TokenBridgeRelayerError::BumpNotFound)?;

        // Set the initial precision values.
        sender_config.relayer_fee_precision = initial_precision;
        sender_config.swap_rate_precision = initial_precision;

        // Set the paused boolean to false. This value controls whether the
        // program will allow outbound transfers.
        sender_config.paused = false;

        // Set Token Bridge related addresses.
        {
            let token_bridge = &mut sender_config.token_bridge;
            token_bridge.config = ctx.accounts.token_bridge_config.key();
            token_bridge.authority_signer = ctx.accounts.token_bridge_authority_signer.key();
            token_bridge.custody_signer = ctx.accounts.token_bridge_custody_signer.key();
            token_bridge.emitter = ctx.accounts.token_bridge_emitter.key();
            token_bridge.sequence = ctx.accounts.token_bridge_sequence.key();
            token_bridge.wormhole_bridge = ctx.accounts.wormhole_bridge.key();
            token_bridge.wormhole_fee_collector = ctx.accounts.wormhole_fee_collector.key();
        }

        // Initialize program's redeemer config.
        let redeemer_config = &mut ctx.accounts.redeemer_config;

        // Set the owner of the redeemer config (effectively the owner of the
        // program).
        redeemer_config.owner = ctx.accounts.owner.key();
        redeemer_config.bump = *ctx
            .bumps
            .get("redeemer_config")
            .ok_or(TokenBridgeRelayerError::BumpNotFound)?;

        // Set the initial precision values and the fee recipient.
        redeemer_config.relayer_fee_precision = initial_precision;
        redeemer_config.swap_rate_precision = initial_precision;
        redeemer_config.fee_recipient = fee_recipient;

        // Set Token Bridge related addresses.
        {
            let token_bridge = &mut redeemer_config.token_bridge;
            token_bridge.config = ctx.accounts.token_bridge_config.key();
            token_bridge.custody_signer = ctx.accounts.token_bridge_custody_signer.key();
            token_bridge.mint_authority = ctx.accounts.token_bridge_mint_authority.key();
        }

        // Initialize program's owner config.
        let owner_config = &mut ctx.accounts.owner_config;

        // Set the owner and assistant for the owner config.
        owner_config.owner = ctx.accounts.owner.key();
        owner_config.assistant = assistant;
        owner_config.pending_owner = None;

        // // Make the contract immutable by setting the new program authority
        // // to `None`.
        // invoke(
        //     &bpf_loader_upgradeable::set_upgrade_authority(
        //         &ID,
        //         &ctx.accounts.owner.key(),
        //         None
        //     ),
        //     &ctx.accounts.to_account_infos()
        // ).map_err(|_| TokenBridgeRelayerError::FailedToMakeImmutable)?;

        // Done.
        Ok(())
    }

    /// This instruction registers a new foreign contract (from another
    /// network) and saves the emitter information in a ForeignEmitter account.
    /// This instruction is owner-only, meaning that only the owner of the
    /// program (defined in the [Config] account) can add and update foreign
    /// contracts.
    ///
    /// # Arguments
    ///
    /// * `ctx`     - `RegisterForeignContract` context
    /// * `chain`   - Wormhole Chain ID
    /// * `address` - Wormhole Emitter Address
    pub fn register_foreign_contract(
        ctx: Context<RegisterForeignContract>,
        chain: u16,
        address: [u8; 32],
    ) -> Result<()> {
        // Foreign emitter cannot share the same Wormhole Chain ID as the
        // Solana Wormhole program's. And cannot register a zero address.
        require!(
            chain > wormhole::CHAIN_ID_SOLANA && !address.iter().all(|&x| x == 0),
            TokenBridgeRelayerError::InvalidForeignContract,
        );

        // Save the emitter info into the ForeignEmitter account.
        let emitter = &mut ctx.accounts.foreign_contract;
        emitter.chain = chain;
        emitter.address = address;
        emitter.token_bridge_foreign_endpoint = ctx.accounts.token_bridge_foreign_endpoint.key();

        // Done.
        Ok(())
    }

    /// This instruction registers a new token and saves the initial `swap_rate`
    /// and `max_native_token_amount` in a RegisteredToken account.
    /// This instruction is owner-only, meaning that only the owner of the
    /// program (defined in the [Config] account) can register a token.
    ///
    /// # Arguments
    ///
    /// * `ctx` - `RegisterToken` context
    /// * `swap_rate`:
    ///    - USD converion rate scaled by the `swap_rate_precision`. For example,
    ///    - if the conversion rate is $15 and the `swap_rate_precision` is
    ///    - 1000000, the `swap_rate` should be set to 15000000.
    /// * `max_native_swap_amount`:
    ///    - Maximum amount of native tokens that can be swapped for this token
    ///    - on this chain.
    pub fn register_token(
        ctx: Context<RegisterToken>,
        swap_rate: u64,
        max_native_swap_amount: u64
    ) -> Result<()> {
        require!(
            !ctx.accounts.registered_token.is_registered,
            TokenBridgeRelayerError::TokenAlreadyRegistered
        );
        require!(
            swap_rate > 0,
            TokenBridgeRelayerError::ZeroSwapRate
        );

        // The max_native_swap_amount must be set to zero for the native mint.
        require!(
            ctx.accounts.mint.key() != spl_token::native_mint::ID
            || max_native_swap_amount == 0,
            TokenBridgeRelayerError::SwapsNotAllowedForNativeMint
        );

        // Register the token by setting the swap_rate and max_native_swap_amount.
        ctx.accounts.registered_token.set_inner(RegisteredToken {
            swap_rate,
            max_native_swap_amount,
            is_registered: true
        });

        Ok(())
    }

    /// This instruction deregisters a token by setting the `is_registered`
    /// field in the `RegisteredToken` account to `false`. It also sets the
    /// `swap_rate` and `max_native_swap_amount` to zero. This instruction
    /// is owner-only, meaning that only the owner of the program (defined
    /// in the [Config] account) can register a token.
    pub fn deregister_token(
        ctx: Context<DeregisterToken>
    ) -> Result<()> {
        require!(
            ctx.accounts.registered_token.is_registered,
            TokenBridgeRelayerError::TokenAlreadyRegistered
        );

        // Register the token by setting the swap_rate and max_native_swap_amount.
        ctx.accounts.registered_token.set_inner(RegisteredToken {
            swap_rate: 0,
            max_native_swap_amount: 0,
            is_registered: false
        });

        Ok(())
    }

    /// This instruction updates the `relayer_fee` in the `RelayerFee` account.
    /// The `relayer_fee` is scaled by the `relayer_fee_precision`. For example,
    /// if the `relayer_fee` is $15 and the `relayer_fee_precision` is 1000000,
    /// the `relayer_fee` should be set to 15000000. This instruction can
    /// only be called by the owner or assistant, which are defined in the
    /// [OwnerConfig] account.
    ///
    /// # Arguments
    ///
    /// * `ctx`   - `UpdateRelayerFee` context
    /// * `chain` - Wormhole Chain ID
    /// * `fee`   - Relayer fee scaled by the `relayer_fee_precision`
    pub fn update_relayer_fee(
        ctx: Context<UpdateRelayerFee>,
        chain: u16,
        fee: u64
    ) -> Result<()> {
        // Check that the signer is the owner or assistant.
        require!(
            ctx.accounts.owner_config.is_authorized(&ctx.accounts.payer.key()),
            TokenBridgeRelayerError::OwnerOrAssistantOnly
        );

        // NOTE: We do not have to check if the chainId is valid, or if a chainId
        // has been registered with a foreign emitter. Since the ForeignContract
        // account is required, this means the account has been created and
        // passed the checks required for successfully registering an emitter.

        // Save the chain and fee information in the RelayerFee account.
        let relayer_fee = &mut ctx.accounts.relayer_fee;
        relayer_fee.chain = chain;
        relayer_fee.fee = fee;

        Ok(())
    }

    /// This instruction updates the `relayer_fee_precision` in the
    /// `SenderConfig` and `RedeemerConfig` accounts. The `relayer_fee_precision`
    /// is used to scale the `relayer_fee`. This instruction is owner-only,
    /// meaning that only the owner of the program (defined in the [Config]
    /// account) can register a token.
    ///
    /// # Arguments
    ///
    /// * `ctx` - `UpdatePrecision` context
    /// * `relayer_fee_precision` - Precision used to scale the relayer fee.
    pub fn update_relayer_fee_precision(
        ctx: Context<UpdatePrecision>,
        relayer_fee_precision: u32,
    ) -> Result<()> {
        require!(
            relayer_fee_precision > 0,
            TokenBridgeRelayerError::InvalidPrecision,
        );

        // Update redeemer config.
        let redeemer_config = &mut ctx.accounts.redeemer_config;
        redeemer_config.relayer_fee_precision = relayer_fee_precision;

        // Update sender config.
        let sender_config = &mut ctx.accounts.sender_config;
        sender_config.relayer_fee_precision = relayer_fee_precision;

        // Done.
        Ok(())
    }

    /// This instruction updates the `swap_rate` in the `RegisteredToken`
    /// account. This instruction can only be called by the owner or
    /// assistant, which are defined in the [OwnerConfig] account.
    ///
    /// # Arguments
    ///
    /// * `ctx`       - `UpdateSwapRate` context
    /// * `swap_rate` - USD conversion rate for the specified token.
    pub fn update_swap_rate(
        ctx: Context<UpdateSwapRate>,
        swap_rate: u64
    ) -> Result<()> {
        // Check that the signer is the owner or assistant.
        require!(
            ctx.accounts.owner_config.is_authorized(&ctx.accounts.payer.key()),
            TokenBridgeRelayerError::OwnerOrAssistantOnly
        );

        // Confirm that the token is registered and the new swap rate
        // is nonzero.
        require!(
            ctx.accounts.registered_token.is_registered,
            TokenBridgeRelayerError::TokenNotRegistered
        );
        require!(
            swap_rate > 0,
            TokenBridgeRelayerError::ZeroSwapRate
        );

        // Set the new swap rate.
        let registered_token = &mut ctx.accounts.registered_token;
        registered_token.swap_rate = swap_rate;

        Ok(())
    }

    /// This instruction updates the `swap_rate_precision` in the
    /// `SenderConfig` and `RedeemerConfig` accounts. The `swap_rate_precision`
    /// is used to scale the `swap_rate`. This instruction is owner-only,
    /// meaning that only the owner of the program (defined in the [Config]
    /// account) can register a token.
    ///
    /// # Arguments
    ///
    /// * `ctx` - `UpdatePrecision` context
    /// * `swap_rate_precision` - Precision used to scale the `swap_rate`.
    pub fn update_swap_rate_precision(
        ctx: Context<UpdatePrecision>,
        swap_rate_precision: u32,
    ) -> Result<()> {
        require!(
            swap_rate_precision > 0,
            TokenBridgeRelayerError::InvalidPrecision,
        );

        // Update redeemer config.
        let redeemer_config = &mut ctx.accounts.redeemer_config;
        redeemer_config.swap_rate_precision = swap_rate_precision;

        // Update sender config.
        let sender_config = &mut ctx.accounts.sender_config;
        sender_config.swap_rate_precision = swap_rate_precision;

        // Done.
        Ok(())
    }

    /// This instruction updates the `max_native_swap_amount` in the
    /// `RegisteredToken` account. This instruction is owner-only,
    /// meaning that only the owner of the program (defined in the [Config]
    /// account) can register a token.
    ///
    /// # Arguments
    ///
    /// * `ctx` - `UpdateMaxNativeSwapAmount` context
    /// * `max_native_swap_amount`:
    ///    - Maximum amount of native tokens that can be swapped for this token
    ///    - on this chain.
    pub fn update_max_native_swap_amount(
        ctx: Context<ManageToken>,
        max_native_swap_amount: u64
    ) -> Result<()> {
        require!(
            ctx.accounts.registered_token.is_registered,
            TokenBridgeRelayerError::TokenNotRegistered
        );

        // The max_native_swap_amount must be set to zero for the native mint.
        require!(
            ctx.accounts.mint.key() != spl_token::native_mint::ID
            || max_native_swap_amount == 0,
            TokenBridgeRelayerError::SwapsNotAllowedForNativeMint
        );

        // Set the new max_native_swap_amount.
        let registered_token = &mut ctx.accounts.registered_token;
        registered_token.max_native_swap_amount = max_native_swap_amount;

        Ok(())
    }

    /// This instruction updates the `paused` boolean in the `SenderConfig`
    /// account. This instruction is owner-only, meaning that only the owner
    /// of the program (defined in the [Config] account) can pause outbound
    /// transfers.
    ///
    /// # Arguments
    ///
    /// * `ctx` - `PauseOutboundTransfers` context
    /// * `paused` - Boolean indicating whether outbound transfers are paused.
    pub fn set_pause_for_transfers(
        ctx: Context<PauseOutboundTransfers>,
        paused: bool
    ) -> Result<()> {
        // Set the new paused boolean.
        let sender_config = &mut ctx.accounts.config;
        sender_config.paused = paused;

        Ok(())
    }

    /// This instruction sets the `pending_owner` field in the `OwnerConfig`
    /// account. This instruction is owner-only, meaning that only the owner
    /// of the program (defined in the [Config] account) can submit an
    /// ownership transfer request.
    ///
    /// # Arguments
    ///
    /// * `ctx`       - `RegisterForeignContract` context
    /// * `new_owner` - Pubkey of the pending owner.
    pub fn submit_ownership_transfer_request(
        ctx: Context<ManageOwnershipTransfer>,
        new_owner: Pubkey
    ) -> Result<()> {
        require_keys_neq!(
            new_owner,
            Pubkey::default(),
            TokenBridgeRelayerError::InvalidPublicKey
        );
        require_keys_neq!(
            new_owner,
            ctx.accounts.owner_config.owner,
            TokenBridgeRelayerError::AlreadyTheOwner
        );

        let owner_config= &mut ctx.accounts.owner_config;
        owner_config.pending_owner = Some(new_owner);

        Ok(())
    }

    /// This instruction confirms that the `pending_owner` is the signer of
    /// the transaction and updates the `owner` field in the `SenderConfig`,
    /// `RedeemerConfig`, and `OwnerConfig` accounts.
    pub fn confirm_ownership_transfer_request(
        ctx: Context<ConfirmOwnershipTransfer>
    ) -> Result<()> {
        // Check that the signer is the pending owner.
        require!(
            ctx.accounts.owner_config.is_pending_owner(&ctx.accounts.payer.key()),
            TokenBridgeRelayerError::NotPendingOwner
        );

        // Unwrap the pending owner.
        let pending_owner = ctx.accounts.owner_config.pending_owner.unwrap();

        // Update the sender config.
        let sender_config = &mut ctx.accounts.sender_config;
        sender_config.owner = pending_owner;

        // Update the redeemer config.
        let redeemer_config = &mut ctx.accounts.redeemer_config;
        redeemer_config.owner = pending_owner;

        let owner_config = &mut ctx.accounts.owner_config;
        owner_config.owner = pending_owner;
        owner_config.pending_owner = None;

        Ok(())
    }

    /// This instruction cancels the ownership transfer request by setting
    /// the `pending_owner` field in the `OwnerConfig` account to `None`.
    /// This instruction is owner-only, meaning that only the owner of the
    /// program (defined in the [Config] account) can cancel an ownership
    /// transfer request.
    pub fn cancel_ownership_transfer_request(
        ctx: Context<ManageOwnershipTransfer>
    ) -> Result<()> {
        let owner_config = &mut ctx.accounts.owner_config;
        owner_config.pending_owner = None;

        Ok(())
    }

    /// This instruction is used to transfer native tokens from Solana to a
    /// foreign blockchain. The user can optionally specify a
    /// `to_native_token_amount` to swap some of the tokens for the native
    /// asset on the target chain. For a fee, an off-chain relayer will redeem
    /// the transfer on the target chain. If the user is transferring native
    /// SOL, the contract will autormatically wrap the lamports into a WSOL.
    ///
    /// # Arguments
    ///
    /// * `ctx` - `SendNativeTokensWithPayload` context
    /// * `amount` - Amount of tokens to send
    /// * `to_native_token_amount`:
    ///     - Amount of tokens to swap for native assets on the target chain
    /// * `recipient_chain` - Chain ID of the target chain
    /// * `recipient_address` - Address of the target wallet on the target chain
    /// * `batch_id` - Nonce of Wormhole message
    /// * `wrap_native` - Whether to wrap native SOL
    pub fn send_native_tokens_with_payload(
        ctx: Context<SendNativeTokensWithPayload>,
        amount: u64,
        to_native_token_amount: u64,
        recipient_chain: u16,
        recipient_address: [u8; 32],
        batch_id: u32,
        wrap_native: bool
    ) -> Result<()> {
        // Confirm that outbound transfers are not paused.
        require!(
            !ctx.accounts.config.paused,
            TokenBridgeRelayerError::OutboundTransfersPaused
        );

        // Confirm that the mint is a registered token.
        require!(
            ctx.accounts.registered_token.is_registered,
            TokenBridgeRelayerError::TokenNotRegistered
        );

        // Confirm that the user passed a valid target wallet on a registered
        // chain.
        require!(
            recipient_chain > wormhole::CHAIN_ID_SOLANA
            && !recipient_address.iter().all(|&x| x == 0),
            TokenBridgeRelayerError::InvalidRecipient,
        );

        // Token Bridge program truncates amounts to 8 decimals, so there will
        // be a residual amount if decimals of the SPL is >8. We need to take
        // into account how much will actually be bridged.
        let truncated_amount = token_bridge::truncate_amount(
            amount,
            ctx.accounts.mint.decimals
        );
        require!(
            truncated_amount > 0,
            TokenBridgeRelayerError::ZeroBridgeAmount
        );

        // Normalize the to_native_token_amount to 8 decimals.
        let normalized_to_native_amount = token_bridge::normalize_amount(
            to_native_token_amount,
            ctx.accounts.mint.decimals
        );
        require!(
            to_native_token_amount == 0 ||
            normalized_to_native_amount > 0,
            TokenBridgeRelayerError::InvalidToNativeAmount
        );

        // Compute the relayer fee in terms of the native token being
        // transfered.
        let token_fee = ctx.accounts.relayer_fee.checked_token_fee(
            ctx.accounts.mint.decimals,
            ctx.accounts.registered_token.swap_rate,
            ctx.accounts.config.swap_rate_precision,
            ctx.accounts.config.relayer_fee_precision
        ).ok_or(TokenBridgeRelayerError::FeeCalculationError)?;

        // Normalize the transfer amount and relayer fee and confirm that the
        // user has sent enough tokens to cover the native swap on the target
        // chain and to pay the relayer fee.
        let normalized_relayer_fee = token_bridge::normalize_amount(
            token_fee,
            ctx.accounts.mint.decimals
        );
        let normalized_amount = token_bridge::normalize_amount(
            amount,
            ctx.accounts.mint.decimals
        );
        require!(
            normalized_amount > normalized_to_native_amount + normalized_relayer_fee,
            TokenBridgeRelayerError::InsufficientFunds
        );

        // These seeds are used to:
        // 1.  Sign the Sender Config's token account to delegate approval
        //     of truncated_amount.
        // 2.  Sign Token Bridge program's transfer_native instruction.
        // 3.  Close tmp_token_account.
        let config_seeds = &[
            SenderConfig::SEED_PREFIX.as_ref(),
            &[ctx.accounts.config.bump],
        ];

        // If the user wishes to transfer native SOL, we need to transfer the
        // lamports to the tmp_token_account and then convert it to native SOL. Otherwise,
        // we can just transfer the specified token to the tmp_token_account.
        if wrap_native {
            require!(
                ctx.accounts.mint.key() == spl_token::native_mint::ID,
                TokenBridgeRelayerError::NativeMintRequired
            );

            // Transfer lamports to the tmp_token_account (these lamports will be our WSOL).
            system_program::transfer(
                CpiContext::new(
                    ctx.accounts.system_program.to_account_info(),
                    Transfer {
                        from: ctx.accounts.payer.to_account_info(),
                        to: ctx.accounts.tmp_token_account.to_account_info(),
                    },
                ),
                truncated_amount,
            )?;

            // Sync the token account based on the lamports we sent it,
            // this is where the wrapping takes place.
            token::sync_native(CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                token::SyncNative {
                    account: ctx.accounts.tmp_token_account.to_account_info(),
                },
            ))?;
        } else {
            anchor_spl::token::transfer(
                CpiContext::new(
                    ctx.accounts.token_program.to_account_info(),
                    anchor_spl::token::Transfer {
                        from: ctx.accounts.from_token_account.to_account_info(),
                        to: ctx.accounts.tmp_token_account.to_account_info(),
                        authority: ctx.accounts.payer.to_account_info(),
                    },
                ),
                truncated_amount,
            )?;
        }

        // Delegate spending to Token Bridge program's authority signer.
        anchor_spl::token::approve(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                anchor_spl::token::Approve {
                    to: ctx.accounts.tmp_token_account.to_account_info(),
                    delegate: ctx.accounts.token_bridge_authority_signer.to_account_info(),
                    authority: ctx.accounts.config.to_account_info(),
                },
                &[&config_seeds[..]],
            ),
            truncated_amount,
        )?;

        // Serialize TokenBridgeRelayerMessage as encoded payload for Token Bridge
        // transfer.
        let payload = TokenBridgeRelayerMessage::TransferWithRelay {
            target_relayer_fee: normalized_relayer_fee,
            to_native_token_amount: normalized_to_native_amount,
            recipient: recipient_address
        }
        .try_to_vec()?;

        // Bridge native token with encoded payload.
        token_bridge::transfer_native_with_payload(
            CpiContext::new_with_signer(
                ctx.accounts.token_bridge_program.to_account_info(),
                token_bridge::TransferNativeWithPayload {
                    payer: ctx.accounts.payer.to_account_info(),
                    config: ctx.accounts.token_bridge_config.to_account_info(),
                    from: ctx.accounts.tmp_token_account.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    custody: ctx.accounts.token_bridge_custody.to_account_info(),
                    authority_signer: ctx.accounts.token_bridge_authority_signer.to_account_info(),
                    custody_signer: ctx.accounts.token_bridge_custody_signer.to_account_info(),
                    wormhole_bridge: ctx.accounts.wormhole_bridge.to_account_info(),
                    wormhole_message: ctx.accounts.wormhole_message.to_account_info(),
                    wormhole_emitter: ctx.accounts.token_bridge_emitter.to_account_info(),
                    wormhole_sequence: ctx.accounts.token_bridge_sequence.to_account_info(),
                    wormhole_fee_collector: ctx.accounts.wormhole_fee_collector.to_account_info(),
                    clock: ctx.accounts.clock.to_account_info(),
                    sender: ctx.accounts.config.to_account_info(),
                    rent: ctx.accounts.rent.to_account_info(),
                    system_program: ctx.accounts.system_program.to_account_info(),
                    token_program: ctx.accounts.token_program.to_account_info(),
                    wormhole_program: ctx.accounts.wormhole_program.to_account_info(),
                },
                &[
                    &config_seeds[..],
                    &[
                        SEED_PREFIX_BRIDGED,
                        &ctx.accounts
                            .token_bridge_sequence
                            .next_value()
                            .to_le_bytes()[..],
                        &[*ctx
                            .bumps
                            .get("wormhole_message")
                            .ok_or(TokenBridgeRelayerError::BumpNotFound)?],
                    ],
                ],
            ),
            batch_id,
            truncated_amount,
            ctx.accounts.foreign_contract.address,
            recipient_chain,
            payload,
            &ctx.program_id.key(),
        )?;

        // Finish instruction by closing tmp_token_account.
        anchor_spl::token::close_account(CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            anchor_spl::token::CloseAccount {
                account: ctx.accounts.tmp_token_account.to_account_info(),
                destination: ctx.accounts.payer.to_account_info(),
                authority: ctx.accounts.config.to_account_info(),
            },
            &[&config_seeds[..]],
        ))
    }

    /// This instruction is used to redeem token transfers from foreign emitters.
    /// It takes custody of the released native tokens and sends the tokens to the
    /// encoded `recipient`. It pays the `fee_recipient` in the token
    /// denomination. If requested by the user, it will perform a swap with the
    /// off-chain relayer to provide the user with lamports. If the token
    /// being transferred is WSOL, the contract will unwrap the WSOL and send
    /// the lamports to the recipient and pay the relayer in lamports.
    ///
    /// # Arguments
    ///
    /// * `ctx` - `RedeemNativeTransferWithPayload` context
    /// * `vaa_hash` - Hash of the VAA that triggered the transfer
    pub fn redeem_native_transfer_with_payload(
        ctx: Context<RedeemNativeTransferWithPayload>,
        _vaa_hash: [u8; 32],
    ) -> Result<()> {
        // The Token Bridge program's claim account is only initialized when
        // a transfer is redeemed (and the boolean value `true` is written as
        // its data).
        //
        // The Token Bridge program will automatically fail if this transfer
        // is redeemed again. But we choose to short-circuit the failure as the
        // first evaluation of this instruction.
        require!(
            ctx.accounts.token_bridge_claim.data_is_empty(),
            TokenBridgeRelayerError::AlreadyRedeemed
        );

        // Confirm that the mint is a registered token.
        require!(
            ctx.accounts.registered_token.is_registered,
            TokenBridgeRelayerError::TokenNotRegistered
        );

        // The intended recipient must agree with the recipient account.
        let TokenBridgeRelayerMessage::TransferWithRelay {
            target_relayer_fee,
            to_native_token_amount,
            recipient
        } = ctx.accounts.vaa.message().data();
        require!(
            ctx.accounts.recipient.key().to_bytes() == *recipient,
            TokenBridgeRelayerError::InvalidRecipient
        );

        // These seeds are used to:
        // 1.  Redeem Token Bridge program's
        //     complete_transfer_native_with_payload.
        // 2.  Transfer tokens to relayer if it exists.
        // 3.  Transfer remaining tokens to recipient.
        // 4.  Close tmp_token_account.
        let config_seeds = &[
            RedeemerConfig::SEED_PREFIX.as_ref(),
            &[ctx.accounts.config.bump],
        ];

        // Redeem the token transfer to the tmp_token_account.
        token_bridge::complete_transfer_native_with_payload(CpiContext::new_with_signer(
            ctx.accounts.token_bridge_program.to_account_info(),
            token_bridge::CompleteTransferNativeWithPayload {
                payer: ctx.accounts.payer.to_account_info(),
                config: ctx.accounts.token_bridge_config.to_account_info(),
                vaa: ctx.accounts.vaa.to_account_info(),
                claim: ctx.accounts.token_bridge_claim.to_account_info(),
                foreign_endpoint: ctx.accounts.token_bridge_foreign_endpoint.to_account_info(),
                to: ctx.accounts.tmp_token_account.to_account_info(),
                redeemer: ctx.accounts.config.to_account_info(),
                custody: ctx.accounts.token_bridge_custody.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                custody_signer: ctx.accounts.token_bridge_custody_signer.to_account_info(),
                rent: ctx.accounts.rent.to_account_info(),
                system_program: ctx.accounts.system_program.to_account_info(),
                token_program: ctx.accounts.token_program.to_account_info(),
                wormhole_program: ctx.accounts.wormhole_program.to_account_info(),
            },
            &[&config_seeds[..]],
        ))?;

        // Denormalize the transfer amount and target relayer fee encoded in
        // the VAA.
        let amount = token_bridge::denormalize_amount(
            ctx.accounts.vaa.data().amount(),
            ctx.accounts.mint.decimals,
        );
        let denormalized_relayer_fee = token_bridge::denormalize_amount(
            *target_relayer_fee,
            ctx.accounts.mint.decimals,
        );

        // Check to see if the transfer is for wrapped SOL. If it is,
        // unwrap and transfer the SOL to the recipient and relayer.
        // Since we are unwrapping the SOL, this contract will not
        // perform a swap with the off-chain relayer.
        if ctx.accounts.mint.key() == spl_token::native_mint::ID {
            // Transfer all lamports to the payer.
            anchor_spl::token::close_account(CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                anchor_spl::token::CloseAccount {
                    account: ctx.accounts.tmp_token_account.to_account_info(),
                    destination: ctx.accounts.payer.to_account_info(),
                    authority: ctx.accounts.config.to_account_info(),
                },
                &[&config_seeds[..]],
            ))?;

            // If the payer is a relayer, we need to send the expected lamports
            // to the recipient, less the relayer fee.
            if ctx.accounts.payer.key() != ctx.accounts.recipient.key() {
                system_program::transfer(
                    CpiContext::new(
                        ctx.accounts.system_program.to_account_info(),
                        Transfer {
                            from: ctx.accounts.payer.to_account_info(),
                            to: ctx.accounts.recipient.to_account_info(),
                        },
                    ),
                    amount - denormalized_relayer_fee,
                )?;
            }

            // We're done here.
            Ok(())
        } else {
            // Handle self redemptions. If payer is the recipient, we should
            // send the entire transfer amount.
            if ctx.accounts.payer.key() == ctx.accounts.recipient.key() {
                // Transfer tokens from tmp_token_account to recipient.
                anchor_spl::token::transfer(
                    CpiContext::new_with_signer(
                        ctx.accounts.token_program.to_account_info(),
                        anchor_spl::token::Transfer {
                            from: ctx.accounts.tmp_token_account.to_account_info(),
                            to: ctx.accounts.recipient_token_account.to_account_info(),
                            authority: ctx.accounts.config.to_account_info(),
                        },
                        &[&config_seeds[..]],
                    ),
                    amount,
                )?;
            } else {
                // Denormalize the to_native_token_amount.
                let denormalized_to_native_token_amount =
                    token_bridge::denormalize_amount(
                        *to_native_token_amount,
                        ctx.accounts.mint.decimals,
                    );

                // Calculate the amount of SOL that should be sent to the
                // recipient.
                let (token_amount_in, native_amount_out) =
                    ctx.accounts.registered_token.calculate_native_swap_amounts(
                        ctx.accounts.mint.decimals,
                        ctx.accounts.native_registered_token.swap_rate,
                        ctx.accounts.config.swap_rate_precision,
                        denormalized_to_native_token_amount
                    ).ok_or(TokenBridgeRelayerError::InvalidSwapCalculation)?;

                // Transfer lamports from the payer to the recipient if the
                // native_amount_out is nonzero.
                if native_amount_out > 0 {
                    system_program::transfer(
                        CpiContext::new(
                            ctx.accounts.system_program.to_account_info(),
                            Transfer {
                                from: ctx.accounts.payer.to_account_info(),
                                to: ctx.accounts.recipient.to_account_info(),
                            },
                        ),
                        native_amount_out
                    )?;
                }

                // Calculate the amount for the fee recipient.
                let amount_for_fee_recipient = token_amount_in + denormalized_relayer_fee;

                // Transfer tokens from tmp_token_account to the fee recipient.
                if amount_for_fee_recipient > 0 {
                    anchor_spl::token::transfer(
                        CpiContext::new_with_signer(
                            ctx.accounts.token_program.to_account_info(),
                            anchor_spl::token::Transfer {
                                from: ctx.accounts.tmp_token_account.to_account_info(),
                                to: ctx.accounts.fee_recipient_token_account.to_account_info(),
                                authority: ctx.accounts.config.to_account_info(),
                            },
                            &[&config_seeds[..]],
                        ),
                        amount_for_fee_recipient
                    )?;
                }

                // Transfer tokens from tmp_token_account to recipient.
                anchor_spl::token::transfer(
                    CpiContext::new_with_signer(
                        ctx.accounts.token_program.to_account_info(),
                        anchor_spl::token::Transfer {
                            from: ctx.accounts.tmp_token_account.to_account_info(),
                            to: ctx.accounts.recipient_token_account.to_account_info(),
                            authority: ctx.accounts.config.to_account_info(),
                        },
                        &[&config_seeds[..]],
                    ),
                    amount - amount_for_fee_recipient
                )?;
            }

            // Finish instruction by closing tmp_token_account.
            anchor_spl::token::close_account(CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                anchor_spl::token::CloseAccount {
                    account: ctx.accounts.tmp_token_account.to_account_info(),
                    destination: ctx.accounts.payer.to_account_info(),
                    authority: ctx.accounts.config.to_account_info(),
                },
                &[&config_seeds[..]],
            ))
        }
    }

    /// This instruction is used to transfer wrapped tokens from Solana to a
    /// foreign blockchain. The user can optionally specify a
    /// `to_native_token_amount` to swap some of the tokens for the native
    /// assets on the target chain. For a fee, an off-chain relayer will redeem
    /// the transfer on the target chain. This instruction should only be called
    /// when the user is transferring a wrapped token.
    ///
    /// # Arguments
    ///
    /// * `ctx` - `SendWrappedTokensWithPayload` context
    /// * `amount` - Amount of tokens to send
    /// * `to_native_token_amount`:
    ///    - Amount of tokens to swap for native assets on the target chain
    /// * `recipient_chain` - Chain ID of the target chain
    /// * `recipient_address` - Address of the target wallet on the target chain
    /// * `batch_id` - Nonce of Wormhole message
    pub fn send_wrapped_tokens_with_payload(
        ctx: Context<SendWrappedTokensWithPayload>,
        amount: u64,
        to_native_token_amount: u64,
        recipient_chain: u16,
        recipient_address: [u8; 32],
        batch_id: u32
    ) -> Result<()> {
        // Confirm that outbound transfers are not paused.
        require!(
            !ctx.accounts.config.paused,
            TokenBridgeRelayerError::OutboundTransfersPaused
        );

        require!(amount > 0, TokenBridgeRelayerError::ZeroBridgeAmount);

        // Confirm that the mint is a registered token.
        require!(
            ctx.accounts.registered_token.is_registered,
            TokenBridgeRelayerError::TokenNotRegistered
        );

        // Confirm that the user passed a valid target wallet on a registered
        // chain.
        require!(
            recipient_chain > wormhole::CHAIN_ID_SOLANA
            && !recipient_address.iter().all(|&x| x == 0),
            TokenBridgeRelayerError::InvalidRecipient,
        );

        // Compute the relayer fee in terms of the native token being
        // transfered.
        let relayer_fee = ctx.accounts.relayer_fee.checked_token_fee(
            ctx.accounts.token_bridge_wrapped_mint.decimals,
            ctx.accounts.registered_token.swap_rate,
            ctx.accounts.config.swap_rate_precision,
            ctx.accounts.config.relayer_fee_precision
        ).ok_or(TokenBridgeRelayerError::FeeCalculationError)?;

        // Confirm that the user has sent enough tokens to cover the native
        // swap on the target chain and to the pay relayer fee.
        require!(
            amount > to_native_token_amount + relayer_fee,
            TokenBridgeRelayerError::InsufficientFunds
        );

        // These seeds are used to:
        // 1.  Sign the Sender Config's token account to delegate approval
        //     of amount.
        // 2.  Sign Token Bridge program's transfer_wrapped instruction.
        // 3.  Close tmp_token_account.
        let config_seeds = &[
            SenderConfig::SEED_PREFIX.as_ref(),
            &[ctx.accounts.config.bump],
        ];

        // First transfer tokens from payer to tmp_token_account.
        anchor_spl::token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                anchor_spl::token::Transfer {
                    from: ctx.accounts.from_token_account.to_account_info(),
                    to: ctx.accounts.tmp_token_account.to_account_info(),
                    authority: ctx.accounts.payer.to_account_info(),
                },
            ),
            amount,
        )?;

        // Delegate spending to Token Bridge program's authority signer.
        anchor_spl::token::approve(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                anchor_spl::token::Approve {
                    to: ctx.accounts.tmp_token_account.to_account_info(),
                    delegate: ctx.accounts.token_bridge_authority_signer.to_account_info(),
                    authority: ctx.accounts.config.to_account_info(),
                },
                &[&config_seeds[..]],
            ),
            amount,
        )?;

        // Serialize TokenBridgeRelayerMessage as encoded payload for Token Bridge
        // transfer.
        let payload = TokenBridgeRelayerMessage::TransferWithRelay {
            target_relayer_fee: relayer_fee,
            to_native_token_amount,
            recipient: recipient_address
        }
        .try_to_vec()?;

        // Bridge wrapped token with encoded payload.
        token_bridge::transfer_wrapped_with_payload(
            CpiContext::new_with_signer(
                ctx.accounts.token_bridge_program.to_account_info(),
                token_bridge::TransferWrappedWithPayload {
                    payer: ctx.accounts.payer.to_account_info(),
                    config: ctx.accounts.token_bridge_config.to_account_info(),
                    from: ctx.accounts.tmp_token_account.to_account_info(),
                    from_owner: ctx.accounts.config.to_account_info(),
                    wrapped_mint: ctx.accounts.token_bridge_wrapped_mint.to_account_info(),
                    wrapped_metadata: ctx.accounts.token_bridge_wrapped_meta.to_account_info(),
                    authority_signer: ctx.accounts.token_bridge_authority_signer.to_account_info(),
                    wormhole_bridge: ctx.accounts.wormhole_bridge.to_account_info(),
                    wormhole_message: ctx.accounts.wormhole_message.to_account_info(),
                    wormhole_emitter: ctx.accounts.token_bridge_emitter.to_account_info(),
                    wormhole_sequence: ctx.accounts.token_bridge_sequence.to_account_info(),
                    wormhole_fee_collector: ctx.accounts.wormhole_fee_collector.to_account_info(),
                    clock: ctx.accounts.clock.to_account_info(),
                    sender: ctx.accounts.config.to_account_info(),
                    rent: ctx.accounts.rent.to_account_info(),
                    system_program: ctx.accounts.system_program.to_account_info(),
                    token_program: ctx.accounts.token_program.to_account_info(),
                    wormhole_program: ctx.accounts.wormhole_program.to_account_info(),
                },
                &[
                    &config_seeds[..],
                    &[
                        SEED_PREFIX_BRIDGED,
                        &ctx.accounts
                            .token_bridge_sequence
                            .next_value()
                            .to_le_bytes()[..],
                        &[*ctx
                            .bumps
                            .get("wormhole_message")
                            .ok_or(TokenBridgeRelayerError::BumpNotFound)?],
                    ],
                ],
            ),
            batch_id,
            amount,
            ctx.accounts.foreign_contract.address,
            recipient_chain,
            payload,
            &ctx.program_id.key(),
        )?;

        // Finish instruction by closing tmp_token_account.
        anchor_spl::token::close_account(CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            anchor_spl::token::CloseAccount {
                account: ctx.accounts.tmp_token_account.to_account_info(),
                destination: ctx.accounts.payer.to_account_info(),
                authority: ctx.accounts.config.to_account_info(),
            },
            &[&config_seeds[..]],
        ))
    }

    /// This instruction is used to redeem token transfers from foreign emitters.
    /// It takes custody of the minted wrapped tokens and sends the tokens to the
    /// encoded `recipient`. It pays the `fee_recipient` in the wrapped-token
    /// denomination. If requested by the user, it will perform a swap with the
    /// off-chain relayer to provide the user with lamports.
    ///
    /// # Arguments
    ///
    /// * `ctx` - `RedeemNativeTransferWithPayload` context
    /// * `vaa_hash` - Hash of the VAA that triggered the transfer
    pub fn redeem_wrapped_transfer_with_payload(
        ctx: Context<RedeemWrappedTransferWithPayload>,
        _vaa_hash: [u8; 32],
    ) -> Result<()> {
        // The Token Bridge program's claim account is only initialized when
        // a transfer is redeemed (and the boolean value `true` is written as
        // its data).
        //
        // The Token Bridge program will automatically fail if this transfer
        // is redeemed again. But we choose to short-circuit the failure as the
        // first evaluation of this instruction.
        require!(
            ctx.accounts.token_bridge_claim.data_is_empty(),
            TokenBridgeRelayerError::AlreadyRedeemed
        );

        // Confirm that the mint is a registered token.
        require!(
            ctx.accounts.registered_token.is_registered,
            TokenBridgeRelayerError::TokenNotRegistered
        );

       // The intended recipient must agree with the recipient account.
       let TokenBridgeRelayerMessage::TransferWithRelay {
        target_relayer_fee,
        to_native_token_amount,
        recipient
        } = ctx.accounts.vaa.message().data();
        require!(
            ctx.accounts.recipient.key().to_bytes() == *recipient,
            TokenBridgeRelayerError::InvalidRecipient
        );

        // These seeds are used to:
        // 1.  Redeem Token Bridge program's
        //     complete_transfer_wrapped_with_payload.
        // 2.  Transfer tokens to relayer if it exists.
        // 3.  Transfer remaining tokens to recipient.
        // 4.  Close tmp_token_account.
        let config_seeds = &[
            RedeemerConfig::SEED_PREFIX.as_ref(),
            &[ctx.accounts.config.bump],
        ];

        // Redeem the token transfer to the tmp_token_account.
        token_bridge::complete_transfer_wrapped_with_payload(CpiContext::new_with_signer(
            ctx.accounts.token_bridge_program.to_account_info(),
            token_bridge::CompleteTransferWrappedWithPayload {
                payer: ctx.accounts.payer.to_account_info(),
                config: ctx.accounts.token_bridge_config.to_account_info(),
                vaa: ctx.accounts.vaa.to_account_info(),
                claim: ctx.accounts.token_bridge_claim.to_account_info(),
                foreign_endpoint: ctx.accounts.token_bridge_foreign_endpoint.to_account_info(),
                to: ctx.accounts.tmp_token_account.to_account_info(),
                redeemer: ctx.accounts.config.to_account_info(),
                wrapped_mint: ctx.accounts.token_bridge_wrapped_mint.to_account_info(),
                wrapped_metadata: ctx.accounts.token_bridge_wrapped_meta.to_account_info(),
                mint_authority: ctx.accounts.token_bridge_mint_authority.to_account_info(),
                rent: ctx.accounts.rent.to_account_info(),
                system_program: ctx.accounts.system_program.to_account_info(),
                token_program: ctx.accounts.token_program.to_account_info(),
                wormhole_program: ctx.accounts.wormhole_program.to_account_info(),
            },
            &[&config_seeds[..]],
        ))?;

        // Denormalize the transfer amount and target relayer fee encoded in
        // the VAA.
        let amount = token_bridge::denormalize_amount(
            ctx.accounts.vaa.data().amount(),
            ctx.accounts.token_bridge_wrapped_mint.decimals,
        );
        let denormalized_relayer_fee = token_bridge::denormalize_amount(
            *target_relayer_fee,
            ctx.accounts.token_bridge_wrapped_mint.decimals,
        );

        // Handle self redemptions. If the payer is the recipient, we should
        // send the entire transfer amount.
        if ctx.accounts.payer.key() == ctx.accounts.recipient.key() {
            // Transfer tokens from tmp_token_account to recipient.
            anchor_spl::token::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    anchor_spl::token::Transfer {
                        from: ctx.accounts.tmp_token_account.to_account_info(),
                        to: ctx.accounts.recipient_token_account.to_account_info(),
                        authority: ctx.accounts.config.to_account_info(),
                    },
                    &[&config_seeds[..]],
                ),
                amount,
            )?;
        } else {
            // Denormalize the to_native_token_amount.
            let denormalized_to_native_token_amount =
                token_bridge::denormalize_amount(
                    *to_native_token_amount,
                    ctx.accounts.token_bridge_wrapped_mint.decimals,
                );

            // Calculate the amount of SOL that should be sent to the
            // recipient.
            let (token_amount_in, native_amount_out) =
                ctx.accounts.registered_token.calculate_native_swap_amounts(
                    ctx.accounts.token_bridge_wrapped_mint.decimals,
                    ctx.accounts.native_registered_token.swap_rate,
                    ctx.accounts.config.swap_rate_precision,
                    denormalized_to_native_token_amount
                ).ok_or(TokenBridgeRelayerError::InvalidSwapCalculation)?;

            // Transfer lamports from the payer to the recipient if the
            // native_amount_out is nonzero.
            if native_amount_out > 0 {
                system_program::transfer(
                    CpiContext::new(
                        ctx.accounts.system_program.to_account_info(),
                        Transfer {
                            from: ctx.accounts.payer.to_account_info(),
                            to: ctx.accounts.recipient.to_account_info(),
                        },
                    ),
                    native_amount_out
                )?;
            }

            // Calculate the amount for the fee recipient.
            let amount_for_fee_recipient = token_amount_in + denormalized_relayer_fee;

            // Transfer tokens from tmp_token_account to the fee recipient.
            if amount_for_fee_recipient > 0 {
                anchor_spl::token::transfer(
                    CpiContext::new_with_signer(
                        ctx.accounts.token_program.to_account_info(),
                        anchor_spl::token::Transfer {
                            from: ctx.accounts.tmp_token_account.to_account_info(),
                            to: ctx.accounts.fee_recipient_token_account.to_account_info(),
                            authority: ctx.accounts.config.to_account_info(),
                        },
                        &[&config_seeds[..]],
                    ),
                    amount_for_fee_recipient
                )?;
            }

            // Transfer tokens from tmp_token_account to recipient.
            anchor_spl::token::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    anchor_spl::token::Transfer {
                        from: ctx.accounts.tmp_token_account.to_account_info(),
                        to: ctx.accounts.recipient_token_account.to_account_info(),
                        authority: ctx.accounts.config.to_account_info(),
                    },
                    &[&config_seeds[..]],
                ),
                amount - amount_for_fee_recipient
            )?;
        }

        // Finish instruction by closing tmp_token_account.
        anchor_spl::token::close_account(CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            anchor_spl::token::CloseAccount {
                account: ctx.accounts.tmp_token_account.to_account_info(),
                destination: ctx.accounts.payer.to_account_info(),
                authority: ctx.accounts.config.to_account_info(),
            },
            &[&config_seeds[..]],
        ))
    }
}
