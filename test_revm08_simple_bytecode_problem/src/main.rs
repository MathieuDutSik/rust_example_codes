use std::{
    fs::File,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

use anyhow::Context;


use revm::{primitives::Bytes, ExecuteCommitEvm};
use revm_context::{
    result::{ExecutionResult, Output},
    BlockEnv, Evm, Journal, TxEnv,
};
use revm_database::{Database, DatabaseCommit, DatabaseRef, InMemoryDB, WrapDatabaseRef};
use revm_handler::{instructions::EthInstructions, EthPrecompiles};
use revm_primitives::{hardfork::SpecId, Address, TxKind};

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

fn main() -> anyhow::Result<()> {
    let bytecode = {
        let source_code = r#"
contract ExampleReturn {

  function test_return(uint256 input) external returns (uint256) {
    uint256 retval = input * input;
    return retval;
  }

}
"#
        .to_string();

        get_bytecode(&source_code, "ExampleReturn")?
    };

    let mut vec: Vec<u8> = bytecode.to_vec();
    vec.push(42);
    let tx_data = Bytes::copy_from_slice(&vec);

    let mut database = InMemoryDB::default();
    let _contract_address = deploy_contract(&mut database, tx_data)?;

    println!("The single_execution has been successful");
    Ok(())
}
