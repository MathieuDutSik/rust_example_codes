use std::{
    collections::HashMap,
    path::Path,
    process::{Command, Stdio},
    fs::File, io::Write,
};

use alloy_sol_types::{sol, SolCall};
use revm::{
    db::InMemoryDB,
    primitives::{Address, Bytes, ExecutionResult, Output, TxKind, U256},
    Evm,
    Database, DatabaseCommit, DatabaseRef,
};
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
    link_info: &HashMap<String, String>,
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
        .ok_or(anyhow::anyhow!("failed to get contract"))?;
    let file_name_contract = contracts
        .get(file_name)
        .ok_or(anyhow::anyhow!("failed to get {file_name}"))?;
    let test_data = file_name_contract
        .get(contract_name)
        .ok_or(anyhow::anyhow!("failed to get {contract_name}"))?;
    let evm_data = test_data
        .get("evm")
        .ok_or(anyhow::anyhow!("failed to get evm"))?;
    println!("get_bytecode_path evm_data={}", evm_data);
    let bytecode = evm_data
        .get("bytecode")
        .ok_or(anyhow::anyhow!("failed to get bytecode"))?;
    println!("get_bytecode_path bytecode={}", bytecode);
    let object = bytecode
        .get("object")
        .ok_or(anyhow::anyhow!("failed to get object"))?;
    println!("get_bytecode_path 1: object={}", object);
    let object = object.to_string();
    println!("get_bytecode_path 2: object={}", object);
    let mut object = object.trim_matches(|c| c == '"').to_string();
    println!("get_bytecode_path 3: object={}", object);
    for (key, value) in link_info {
        let ext_key = format!("__${}$__", key);
        object = object.replace(&ext_key, value);
    }
    let object = hex::decode(&object)?;
    println!("get_bytecode_path 4: object={:?}", object);
    Ok(Bytes::copy_from_slice(&object))
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
) -> anyhow::Result<()> {
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
    let dir = tempdir().unwrap();
    let path = dir.path();

    let bytecode1 = {
        let file_name = "library.sol";
        let library_code_path = path.join(file_name);
        let mut library_code_file = File::create(&library_code_path)?;
        writeln!(
            library_code_file,
            r#"
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

library MathLibrary {{
    function add(uint256 a, uint256 b) external pure returns (uint256) {{
        return a + b;
    }}
}}
"#
        )?;

        get_bytecode_path(path, file_name, "MathLibrary", &HashMap::new())?
    };

    let mut database = InMemoryDB::default();
    let contract_address1 = deploy_contract(&mut database, bytecode1)?;

    let bytecode2 = {
        let file_name = "test_code.sol";
        let test_code_path = path.join(file_name);
        let mut test_code_file = File::create(&test_code_path)?;
        writeln!(
            test_code_file,
            r#"
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import "./library.sol";

contract Calculator {{

    function test_function_add(uint256 a, uint256 b) external {{
      uint256 result = MathLibrary.add(a, b);
      require(result == 8);
    }}

}}
"#
        )?;
        let contract_address1_str = format!("{}", contract_address1)
            .to_lowercase()
            .chars()
            .skip(2)
            .collect();
        println!("contract_address1_str={}", contract_address1_str);
        // The substitudes string is actually the FQN, we should find a way to obtain it directly.
        let link_info: HashMap<String, String> = [(
            "a84a4509d4be6a9963dbd14cd4f071d291".to_string(),
            contract_address1_str,
        )]
        .into_iter()
        .collect();
        get_bytecode_path(path, file_name, "Calculator", &link_info)?
    };

    let contract_address2 = deploy_contract(&mut database, bytecode2)?;

    sol! {
        function test_function_add(uint256 a, uint256 b);
    }

    let a = U256::from(3);
    let b = U256::from(5);
    let fct_args = test_function_addCall { a, b };
    let fct_args = fct_args.abi_encode().into();

    single_execution(&mut database, contract_address2, fct_args).unwrap();

    println!("The single_execution has been successful");
    Ok(())
}
