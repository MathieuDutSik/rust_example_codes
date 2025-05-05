use std::{
    fs::File,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

use alloy_sol_types::{sol, SolCall};
use anyhow::Context;

use revm::{primitives::Bytes, ExecuteCommitEvm};
use revm_context::{
    result::{ExecutionResult, Output},
    BlockEnv, Evm, Journal, TxEnv,
};
use revm_database::{Database, DatabaseCommit, DatabaseRef, InMemoryDB, WrapDatabaseRef};
use revm_handler::{instructions::EthInstructions, EthPrecompiles};
use revm_inspector::{InspectCommitEvm, Inspector};
use revm_interpreter::{CallInputs, CallOutcome, Gas, InstructionResult, InterpreterResult};
use revm_primitives::{hardfork::SpecId, Address, TxKind, U256};

use tempfile::tempdir;

pub fn write_compilation_json(path: &Path, file_name: &str) {
    let mut source = File::create(path).unwrap();
    writeln!(
        source,
        r#"
{{
  "language": "Solidity",
  "sources": {{
    "{file_name}": {{
      "urls": ["./{file_name}"]
    }}
  }},
  "settings": {{
    "viaIR": true,
    "outputSelection": {{
      "*": {{
        "*": ["evm.bytecode"]
      }}
    }}
  }}
}}
"#
    )
    .unwrap();
}

pub fn get_bytecode_path(
    path: &Path,
    file_name: &str,
    contract_name: &str,
) -> anyhow::Result<Bytes> {
    let config_path = path.join("config.json");
    write_compilation_json(&config_path, file_name);
    let config_file = File::open(config_path)?;

    let output_path = path.join("result.json");
    let output_file = File::create(output_path.clone())?;

    let status = Command::new("solc")
        .current_dir(path)
        .arg("--standard-json")
        .stdin(Stdio::from(config_file))
        .stdout(Stdio::from(output_file))
        .status()?;
    assert!(status.success());

    let contents = std::fs::read_to_string(output_path)?;
    let json_data: serde_json::Value = serde_json::from_str(&contents)?;
    let contracts = json_data
        .get("contracts")
        .with_context(|| format!("failed to get contracts in json_data={}", json_data))?;
    let file_name_contract = contracts
        .get(file_name)
        .context("failed to get {file_name}")?;
    let test_data = file_name_contract
        .get(contract_name)
        .context("failed to get contract_name={contract_name}")?;
    let evm_data = test_data.get("evm").context("failed to get evm")?;
    let bytecode = evm_data.get("bytecode").context("failed to get bytecode")?;
    let object = bytecode.get("object").context("failed to get object")?;
    let object = object.to_string();
    let object = object.trim_matches(|c| c == '"').to_string();
    let object = hex::decode(&object)?;
    Ok(Bytes::copy_from_slice(&object))
}

pub fn get_bytecode(source_code: &str, contract_name: &str) -> anyhow::Result<Bytes> {
    let dir = tempdir().unwrap();
    let path = dir.path();
    let file_name = "test_code.sol";
    let test_code_path = path.join(file_name);
    let mut test_code_file = File::create(&test_code_path)?;
    writeln!(test_code_file, "{}", source_code)?;
    get_bytecode_path(path, file_name, contract_name)
}

fn deploy_contract<DB: Database + DatabaseRef + DatabaseCommit>(
    database: &mut DB,
    bytecode: Bytes,
) -> anyhow::Result<Address> {
    let ctx: revm_context::Context<BlockEnv, _, _, _, Journal<WrapDatabaseRef<&mut DB>>, ()> =
        revm_context::Context::new(WrapDatabaseRef(database), SpecId::default());
    let instructions = EthInstructions::new_mainnet();
    let mut evm = Evm::new(ctx, instructions, EthPrecompiles::default());
    let result = evm.transact_commit(TxEnv {
        kind: TxKind::Create,
        data: bytecode,
        nonce: 0,
        ..TxEnv::default()
    });
    println!("1: result={:?}", result);
    let Ok(result) = result else {
        anyhow::bail!("The transact_commit failed 1");
    };
    let ExecutionResult::Success { output, .. } = result else {
        anyhow::bail!("Now getting Success");
    };
    let Output::Create(_, Some(contract_address)) = output else {
        anyhow::bail!("The Output::Create function");
    };
    Ok(contract_address)
}

fn single_execution<DB: Database + DatabaseRef + DatabaseCommit>(
    database: &mut DB,
    contract_address: Address,
    encoded_args: Bytes,
) -> anyhow::Result<InterpreterResult> {
    let ctx: revm_context::Context<BlockEnv, _, _, _, Journal<WrapDatabaseRef<&mut DB>>, ()> =
        revm_context::Context::new(WrapDatabaseRef(database), SpecId::default());
    let instructions = EthInstructions::new_mainnet();
    let mut evm = Evm::new(ctx, instructions, EthPrecompiles::default());
    let result = evm.transact_commit(TxEnv {
        kind: TxKind::Call(contract_address),
        data: encoded_args,
        nonce: 1,
        ..TxEnv::default()
    });
    println!("2: result={:?}", result);
    let Ok(result) = result else {
        anyhow::bail!("The transact_commit failed 2");
    };

    let gas = Gas::new(1000000);
    match result {
        ExecutionResult::Success {
            reason,
            gas_used: _,
            gas_refunded: _,
            logs: _,
            output,
        } => {
            println!("single_execution, case ExecutionResult::Success");
            let result: InstructionResult = reason.into();
            let Output::Call(output) = output else {
                anyhow::bail!("The Output is not a call which is impossible");
            };
            Ok(InterpreterResult {
                result,
                output,
                gas,
            })
        }
        _ => {
            anyhow::bail!("The ExecutionREsult should be a Success");
        }
    }
}

#[derive(Default, Clone)]
struct CallInterceptor;

impl<CTX> Inspector<CTX> for CallInterceptor {
    fn call(&mut self, _context: &mut CTX, inputs: &mut CallInputs) -> Option<CallOutcome> {
        let contract_address = Address::ZERO.create(0);

        if inputs.target_address != contract_address {
            println!("Passing by CallInterceptor::call, call evaluate_contract1, return Some(...)");
            let fct_args = inputs.input.clone();
            let result = evaluate_contract1(fct_args).unwrap();

            let call_outcome = CallOutcome {
                result,
                memory_offset: inputs.return_memory_offset.clone(),
            };
            println!("call_outcome={call_outcome:?}");
            Some(call_outcome)
        } else {
            println!("Passing by CallInterceptor::call, return None");
            None
        }
    }
}

fn evaluate_contract1(fct_args: Bytes) -> anyhow::Result<InterpreterResult> {
    let bytecode = {
        let source_code = r#"
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

contract ExampleCodeFirst {

  function test_function_first(uint256 input) external pure returns (uint256) {
    uint256 retval = input * input;
    return retval;
  }
}
"#
        .to_string();
        get_bytecode(&source_code, "ExampleCodeFirst")?
    };

    let mut database = InMemoryDB::default();
    let contract_address = deploy_contract(&mut database, bytecode)?;

    single_execution(&mut database, contract_address, fct_args)
}

fn main() -> anyhow::Result<()> {
    let bytecode2 = {
        let source_code = r#"
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

interface IExternalContract {
  function test_function_first(uint256 value) external returns (uint256);
}

contract ExternalCodes {

  function test_external_codes(uint256 input) external {
    address contract_address = address(0x0c);

    IExternalContract externalContract = IExternalContract(contract_address);
    uint256 retval_contract = externalContract.test_function_first(input);
    require(retval_contract == 49);
  }

}
"#
        .to_string();
        get_bytecode(&source_code, "ExternalCodes")?
    };

    let mut database = InMemoryDB::default();
    let contract_address = deploy_contract(&mut database, bytecode2)?;

    sol! {
        function test_external_codes(uint256 input);
    }

    let input = U256::from(7);
    let fct_args = test_external_codesCall { input };
    let fct_args = fct_args.abi_encode().into();

    let ctx: revm_context::Context<
        BlockEnv,
        _,
        _,
        _,
        Journal<WrapDatabaseRef<&mut InMemoryDB>>,
        (),
    > = revm_context::Context::new(WrapDatabaseRef(&mut database), SpecId::default());
    let instructions = EthInstructions::new_mainnet();
    let inspector = CallInterceptor;
    let mut evm = Evm::new_with_inspector(
        ctx,
        inspector.clone(),
        instructions,
        EthPrecompiles::default(),
    );
    let result = evm.inspect_commit(
        TxEnv {
            kind: TxKind::Call(contract_address),
            data: fct_args,
            nonce: 1,
            ..TxEnv::default()
        },
        inspector,
    );
    println!("3: result={:?}", result);
    let Ok(result) = result else {
        anyhow::bail!("The transact_commit failed 3");
    };

    let ExecutionResult::Success { .. } = result else {
        anyhow::bail!("Execution did not work out")
    };
    println!("The redirection via the inspector has been successful");
    Ok(())
}
