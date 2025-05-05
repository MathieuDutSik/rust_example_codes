use std::{
    fs::File,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
};

use alloy_sol_types::{sol, SolCall, SolValue};
use anyhow::Context;
use revm::{
    db::InMemoryDB,
    primitives::{Address, Bytes, ExecutionResult, Output, TxKind, U256},
    ContextPrecompile, ContextStatefulPrecompile, Database, DatabaseCommit, DatabaseRef, Evm,
    InnerEvmContext,
};
use revm_precompile::{PrecompileOutput, PrecompileResult};
use revm_primitives::address;
use tempfile::tempdir;

fn precompile_address() -> Address {
    address!("000000000000000000000000000000000000000b")
}

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
        .context("failed to get contract")?;
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

struct SquarePrecompile;

pub fn test_square(input: &Bytes, _gas_limit: u64) -> PrecompileResult {
    sol! {
        struct ValueEncaps {
            uint256 val;
        }
    }
    let value: ValueEncaps = ValueEncaps::abi_decode(input, true).unwrap();
    println!("val={}", value.val);
    let val_sqr = value.val * value.val;
    println!("val_sqr={}", val_sqr);
    let value_sqr = ValueEncaps { val: val_sqr };
    let result: Bytes = value_sqr.abi_encode().into();
    let output = PrecompileOutput::new(10, result);
    Ok(output)
}

impl<DB: Database> ContextStatefulPrecompile<DB> for SquarePrecompile {
    fn call(
        &self,
        input: &Bytes,
        gas_limit: u64,
        _context: &mut InnerEvmContext<DB>,
    ) -> PrecompileResult {
        test_square(input, gas_limit)
    }
}

fn single_execution<DB: Database + DatabaseRef + DatabaseCommit>(
    database: &mut DB,
    contract_address: Address,
    encoded_args: Bytes,
) -> anyhow::Result<()> {
    let mut evm: Evm<'_, (), _> = Evm::builder()
        .with_ref_db(database)
        .append_handler_register(|handler| {
            let precompiles = handler.pre_execution.load_precompiles();
            handler.pre_execution.load_precompiles = Arc::new(move || {
                let mut precompiles = precompiles.clone();
                precompiles.extend([(
                    precompile_address(),
                    ContextPrecompile::ContextStateful(Arc::new(SquarePrecompile)),
                )]);
                precompiles
            });
        })
        .modify_tx_env(|tx| {
            tx.transact_to = TxKind::Call(contract_address);
            tx.data = encoded_args;
        })
        .build();

    let result = evm.transact_commit();
    let Ok(result) = result else {
        anyhow::bail!("The transact_commit failed");
    };

    let ExecutionResult::Success {
        reason: _,
        gas_used: _,
        gas_refunded: _,
        logs: _,
        output: _,
    } = result
    else {
        anyhow::bail!("Execution did not work out")
    };
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let bytecode = {
        let source_code = r#"
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

contract PrecompileCaller {

    function test_precompile(uint256 input) external {
      address precompile = address(0x0b);
      bytes memory input_bytes = abi.encodePacked(input);
      (bool success, bytes memory result_bytes) = precompile.call(input_bytes);
      uint256 result = abi.decode(result_bytes, (uint256));
      require(success);
      require(result == 49);
    }

}
"#
        .to_string();
        get_bytecode(&source_code, "PrecompileCaller")?
    };

    let mut database = InMemoryDB::default();
    let contract_address = deploy_contract(&mut database, bytecode)?;

    sol! {
        function test_precompile(uint256 input);
    }

    let input = U256::from(7);
    let fct_args = test_precompileCall { input };
    let fct_args = fct_args.abi_encode().into();

    single_execution(&mut database, contract_address, fct_args).unwrap();

    println!("The single_execution has been successful");
    Ok(())
}
