import { ethers } from "ethers";
import { RELEASE_CHAIN_ID, RELEASE_RPC, RELEASE_BRIDGE_ADDRESS, ZERO_ADDRESS } from "./consts";
import { tryHexToNativeString, tryUint8ArrayToNative } from "@certusone/wormhole-sdk";
import {
  ITokenBridge,
  ITokenBridgeRelayer,
  ITokenBridgeRelayer__factory,
  ITokenBridge__factory,
} from "../src/ethers-contracts";
import { SwapRateUpdate } from "../helpers/interfaces";
import * as fs from "fs";
import { Config, ConfigArguments, SupportedChainId, isChain, configArgsParser } from "./config";
import { SignerArguments, addSignerArgsParser, getSigner } from "./signer";
import { Check, TxResult, buildOverrides, handleFailure } from "./tx";

interface CustomArguments {
  setSwapRates: boolean;
  setMaxNativeAmounts: boolean;
}

type Arguments = CustomArguments & SignerArguments & ConfigArguments;

async function parseArgs(): Promise<Arguments> {
  const parsed = await addSignerArgsParser(configArgsParser())
    .option("setSwapRates", {
      string: false,
      boolean: true,
      description: "sets swaps rates if true",
      required: true,
    })
    .option("setMaxNativeAmount", {
      string: false,
      boolean: true,
      description: "sets max native swap amounts if true",
      required: true,
    }).argv;

  const args: Arguments = {
    useLedger: parsed.ledger,
    derivationPath: parsed.derivationPath,
    config: parsed.config,
    setSwapRates: parsed.setSwapRates,
    setMaxNativeAmounts: parsed.setMaxNativeAmount,
  };

  return args;
}

async function registerToken(
  relayer: ITokenBridgeRelayer,
  chainId: SupportedChainId,
  contract: string
): Promise<TxResult> {
  const overrides = await buildOverrides(
    () => relayer.estimateGas.registerToken(RELEASE_CHAIN_ID, contract),
    RELEASE_CHAIN_ID
  );

  const tx = await relayer.registerToken(RELEASE_CHAIN_ID, contract, overrides);
  console.log(`Token register tx sent, chainId=${chainId}, token=${contract}, txHash=${tx.hash}`);
  const receipt = await tx.wait();

  const successMessage = `Success: token registered, chainId=${chainId}, token=${contract}, txHash=${receipt.transactionHash}`;
  const failureMessage = `Failed: could not register token, chainId=${chainId}`;
  return TxResult.create(receipt, successMessage, failureMessage, async () => {
    // query the contract and see if the token was registered successfully
    return relayer.isAcceptedToken(contract);
  });
}

async function updateSwapRate(
  relayer: ITokenBridgeRelayer,
  batch: SwapRateUpdate[]
): Promise<TxResult> {
  const overrides = await buildOverrides(
    () => relayer.estimateGas.updateSwapRate(RELEASE_CHAIN_ID, batch),
    RELEASE_CHAIN_ID
  );

  const tx = await relayer.updateSwapRate(RELEASE_CHAIN_ID, batch, overrides);
  console.log(`Swap rates update tx sent, txHash=${tx.hash}`);
  const receipt = await tx.wait();
  let successMessage = `Success: swap rates updated, txHash=${receipt.transactionHash}`;
  for (const update of batch) {
    successMessage += `  token: ${update.token}, swap rate: ${update.value.toString()}`;
  }
  const failureMessage = `Failed: could not update swap rates, txHash=${receipt.transactionHash}`;

  return TxResult.create(receipt, successMessage, failureMessage, async () => true);
}

async function updateMaxNativeSwapAmount(
  relayer: ITokenBridgeRelayer,
  chainId: SupportedChainId,
  contract: string,
  originalTokenAddress: string,
  maxNativeSwapAmount: string
): Promise<TxResult> {
  const maxNativeToUpdate = ethers.BigNumber.from(maxNativeSwapAmount);

  const overrides = await buildOverrides(
    () =>
      relayer.estimateGas.updateMaxNativeSwapAmount(RELEASE_CHAIN_ID, contract, maxNativeToUpdate),
    RELEASE_CHAIN_ID
  );

  const tx = await relayer.updateMaxNativeSwapAmount(
    RELEASE_CHAIN_ID,
    contract,
    maxNativeToUpdate,
    overrides
  );
  console.log(
    `Max swap amount update tx sent, chainId=${chainId}, token=${contract}, max=${maxNativeSwapAmount}, txHash=${tx.hash}`
  );
  const receipt = await tx.wait();
  const successMessage = `Success: max swap amount updated, chainId=${chainId}, token=${contract}, max=${maxNativeSwapAmount}, txHash=${receipt.transactionHash}`;
  const failureMessage = `Failed: could not update max native swap amount, chainId=${chainId}, token=${originalTokenAddress}`;

  return TxResult.create(receipt, successMessage, failureMessage, async () => {
    // query the contract and see if the max native swap amount was set correctly
    const maxNativeInContract = await relayer.maxNativeSwapAmount(contract);

    return maxNativeInContract.eq(maxNativeToUpdate);
  });
}

async function getLocalTokenAddress(
  tokenBridge: ITokenBridge,
  chainId: number,
  address: Uint8Array
) {
  // fetch the wrapped of native address
  let localTokenAddress: string;
  if (chainId == RELEASE_CHAIN_ID) {
    localTokenAddress = tryUint8ArrayToNative(address, chainId);
  } else {
    // fetch the wrapped address
    localTokenAddress = await tokenBridge.wrappedAsset(chainId, address);
    if (localTokenAddress === ZERO_ADDRESS) {
      console.log(
        `Failed: token not attested, chainId=${chainId}, token=${Buffer.from(address).toString(
          "hex"
        )}`
      );
    }
  }

  return localTokenAddress;
}

async function main() {
  const args = await parseArgs();

  // read config
  const {
    deployedContracts: contracts,
    acceptedTokensList: tokenConfig,
    maxNativeSwapAmount: maxNativeSwapAmounts,
  } = JSON.parse(fs.readFileSync(args.config, "utf8")) as Config;

  if (!isChain(RELEASE_CHAIN_ID)) {
    throw new Error(`Unknown wormhole chain id ${RELEASE_CHAIN_ID}`);
  }

  // set up ethers wallet
  const provider = new ethers.providers.StaticJsonRpcProvider(RELEASE_RPC);
  const wallet = await getSigner(args, provider);

  // fetch relayer address from config
  const relayerAddress = tryHexToNativeString(contracts[RELEASE_CHAIN_ID], RELEASE_CHAIN_ID);

  // set up relayer contract
  const relayer = ITokenBridgeRelayer__factory.connect(relayerAddress, wallet);

  // set up token bridge contract
  const tokenBridge = ITokenBridge__factory.connect(RELEASE_BRIDGE_ADDRESS, wallet);

  // placeholder for swap rate batch
  const swapRateUpdates: SwapRateUpdate[] = [];

  const checks: Check[] = [];
  for (const [chainIdString, tokens] of Object.entries(tokenConfig)) {
    const chainIdToRegister = Number(chainIdString);
    if (!isChain(chainIdToRegister)) {
      throw new Error(`Unknown wormhole chain id ${chainIdToRegister}`);
    }
    console.log("\n");
    console.log(`ChainId ${chainIdToRegister}`);

    // loop through tokens and register them
    for (const { contract: tokenContract, swapRate } of tokens) {
      const tokenAddress = "0x" + tokenContract;
      const formattedAddress = ethers.utils.arrayify(tokenAddress);

      // fetch the address on the target chain
      const localTokenAddress = await getLocalTokenAddress(
        tokenBridge,
        chainIdToRegister,
        formattedAddress
      );

      // Query the contract and see if the token has been registered. If it hasn't,
      // register the token.
      const isTokenRegistered = await relayer.isAcceptedToken(localTokenAddress);
      if (!isTokenRegistered) {
        const result = await registerToken(relayer, chainIdToRegister, localTokenAddress);

        handleFailure(checks, result);
      } else {
        console.log(`Token already registered. token=${tokenAddress}`);
      }

      if (args.setMaxNativeAmounts) {
        const result = await updateMaxNativeSwapAmount(
          relayer,
          chainIdToRegister,
          localTokenAddress,
          tokenAddress,
          maxNativeSwapAmounts[RELEASE_CHAIN_ID]
        );

        handleFailure(checks, result);
      }

      if (args.setSwapRates) {
        swapRateUpdates.push({
          token: localTokenAddress,
          value: ethers.BigNumber.from(swapRate),
        });
      }
    }
  }

  if (args.setSwapRates) {
    console.log("\n");
    const result = await updateSwapRate(relayer, swapRateUpdates);
    handleFailure(checks, result);
  }

  const messages = (await Promise.all(checks.map((check) => check()))).join("\n");
  console.log(messages);

  console.log("\n");
  console.log("Accepted tokens list:");
  console.log(await relayer.getAcceptedTokensList());
}

main();
