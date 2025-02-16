use std::{
    fs::File,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

use alloy_sol_types::{sol, SolCall};
use anyhow::Context;
use core::ops::Range;
use revm::{
    db::InMemoryDB,
    inspector_handle_register,
    primitives::{Address, Bytes, ExecutionResult, Output, TxKind, U256},
    Database, DatabaseCommit, DatabaseRef, Evm, EvmContext, Inspector,
};
use revm_interpreter::{CallInputs, CallOutcome, Gas, InstructionResult, InterpreterResult};
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
    println!("json_data={}", json_data);
    println!();
    println!();
    println!();
    println!();
    let contracts = json_data
        .get("contracts")
        .context("failed to get contracts")?;
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
    let mut evm: Evm<'_, (), _> = Evm::builder()
        .with_ref_db(database)
        .modify_tx_env(|tx| {
            tx.clear();
            tx.transact_to = TxKind::Create;
            tx.data = bytecode;
        })
        .build();

    let result = evm.transact_commit();
    let Ok(result) = result else {
        anyhow::bail!("The transact_commit failed");
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
) -> anyhow::Result<CallOutcome> {
    let mut evm: Evm<'_, (), _> = Evm::builder()
        .with_ref_db(database)
        .modify_tx_env(|tx| {
            tx.transact_to = TxKind::Call(contract_address);
            tx.data = encoded_args;
        })
        .build();

    let result = evm.transact_commit();
    let Ok(result) = result else {
        anyhow::bail!("The transact_commit failed");
    };

    let memory_offset = Range::default();
    let gas = Gas::new(1000000);
    let result = match result {
        ExecutionResult::Success {
            reason,
            gas_used: _,
            gas_refunded: _,
            logs: _,
            output,
        } => {
            let result: InstructionResult = reason.into();
            let Output::Call(output) = output else {
                anyhow::bail!("The Output is not a call which is impossible");
            };
            InterpreterResult {
                result,
                output,
                gas,
            }
        }
        ExecutionResult::Revert {
            gas_used: _,
            output,
        } => {
            let result = InstructionResult::Revert;
            InterpreterResult {
                result,
                output,
                gas,
            }
        }
        ExecutionResult::Halt {
            reason,
            gas_used: _,
        } => {
            let result = reason.into();
            let output = Bytes::default();
            InterpreterResult {
                result,
                output,
                gas,
            }
        }
    };
    let call_outcome = CallOutcome {
        result,
        memory_offset,
    };
    Ok(call_outcome)
}

struct CallInterceptor {
    is_first: bool,
}

impl Default for CallInterceptor {
    fn default() -> Self {
        CallInterceptor { is_first: true }
    }
}

impl<DB: Database> Inspector<DB> for CallInterceptor {
    fn call(
        &mut self,
        _context: &mut EvmContext<DB>,
        inputs: &mut CallInputs,
    ) -> Option<CallOutcome> {
        println!("Passing by CallInterceptor::call");

        if self.is_first {
            self.is_first = false;
            None
        } else {
            let fct_args = inputs.input.clone();
            let mut call_outcome = evaluate_contract1(fct_args).unwrap();
            call_outcome.memory_offset = inputs.return_memory_offset.clone();
            println!("call_outcome={call_outcome:?}");
            Some(call_outcome)
        }
    }

    fn call_end(
        &mut self,
        context: &mut EvmContext<DB>,
        inputs: &CallInputs,
        outcome: CallOutcome,
    ) -> CallOutcome {
        let _ = context;
        let _ = inputs;
        outcome
    }
}

// We have CallOutcome = InterpreterResult + memory_offset
// Itself, we have InterpreterResult = InstructionResult + Output(Bytes) + gas
// InstructionResult contains the various bad thing that can happen while
// executing the code.
//
// Also, we have
// CallInputs = Bytes(input) + return_memory_offset + bytecode_address
//       + target_address + caller + value + scheme(WTF?) + is_static + is_eof
// So, yes, bytes in input, bytes in output, but a lot more than that.
//

fn evaluate_contract1(fct_args: Bytes) -> anyhow::Result<CallOutcome> {
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

contract ExampleCallInterceptor {

  function test_call_interceptor(uint256 input) external {
    address contract_address = address(0);
    IExternalContract externalContract = IExternalContract(contract_address);
    uint256 retval = externalContract.test_function_first(input);
    require(retval == 49);
  }

}
"#
        .to_string();
        get_bytecode(&source_code, "ExampleCallInterceptor")?
    };

    let mut database = InMemoryDB::default();
    let contract_address = deploy_contract(&mut database, bytecode2)?;

    sol! {
        function test_call_interceptor(uint256 input);
    }

    let input = U256::from(7);
    let fct_args = test_call_interceptorCall { input };
    let fct_args = fct_args.abi_encode().into();

    let mut insp = CallInterceptor::default();
    let mut evm: Evm<'_, _, _> = Evm::builder()
        .with_ref_db(database)
        .with_external_context(&mut insp)
        .modify_tx_env(|tx| {
            tx.transact_to = TxKind::Call(contract_address);
            tx.data = fct_args;
        })
        .append_handler_register(inspector_handle_register)
        .build();

    let result = evm.transact_commit();
    let Ok(result) = result else {
        anyhow::bail!("The transact_commit failed");
    };
    println!("result={result:?}");

    let ExecutionResult::Success { .. } = result else {
        anyhow::bail!("Execution did not work out")
    };
    println!("The single_execution has been successful");
    Ok(())
}
