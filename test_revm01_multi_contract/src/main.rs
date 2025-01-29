use alloy_sol_types::sol;
use alloy_sol_types::SolCall;

use revm::Database;
use revm::DatabaseCommit;
use revm::DatabaseRef;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;

use revm::db::InMemoryDB;
use revm::{
    primitives::{Address, Bytes, ExecutionResult, Output, TxKind, U256},
    Evm,
};
use std::{fs::File, io::Write};
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
        .ok_or(anyhow::anyhow!("failed to get contract"))?;
    let file_name_contract = contracts
        .get(file_name)
        .ok_or(anyhow::anyhow!("failed to get {file_name}"))?;
    let test_data = file_name_contract
        .get(contract_name)
        .ok_or(anyhow::anyhow!("failed to get test"))?;
    let evm_data = test_data
        .get("evm")
        .ok_or(anyhow::anyhow!("failed to get evm"))?;
    let bytecode = evm_data
        .get("bytecode")
        .ok_or(anyhow::anyhow!("failed to get bytecode"))?;
    let object = bytecode
        .get("object")
        .ok_or(anyhow::anyhow!("failed to get object"))?;
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

    let ExecutionResult::Success {
        reason: _,
        gas_used: _,
        gas_refunded: _,
        logs: _,
        output,
    } = result
    else {
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
    let bytecode1 = {
        let source_code = r#"
contract ExampleCodeFirst {{

    function test_function_first(uint256 input) external {{
      require(input == 2);
    }}

}}
"#
        .to_string();
        get_bytecode(&source_code, "ExampleCodeFirst")?
    };

    let bytecode2 = {
        let source_code = r#"
interface IExternalContract {{
    function test_function_first(uint256 value) external;
}}

contract ExampleCodeSecond {{

    function test_function_second(address contract_address1, uint256 input) external {{
      IExternalContract externalContract = IExternalContract(contract_address1);
      externalContract.test_function_first(input);
    }}

}}
"#
        .to_string();
        get_bytecode(&source_code, "ExampleCodeSecond")?
    };

    let mut database = InMemoryDB::default();
    let contract_address1 = deploy_contract(&mut database, bytecode1)?;
    let contract_address2 = deploy_contract(&mut database, bytecode2)?;

    sol! {
        function test_function_second(address contract_address, uint256 input);
    }

    let input = U256::from(2);
    let fct_args = test_function_secondCall {
        contract_address: contract_address1,
        input,
    };
    let fct_args = fct_args.abi_encode().into();

    single_execution(&mut database, contract_address2, fct_args).unwrap();

    println!("The single_execution has been successful");
    Ok(())
}
