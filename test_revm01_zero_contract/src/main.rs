use std::{
    collections::{btree_map, BTreeMap, HashMap},
    fs::File,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};
use futures::executor::block_on;
use linera_views::{
    batch::Batch,
    memory::MemoryStore,
    store::TestKeyValueStore,
    views::ViewError,
};

use alloy::primitives::B256;
use alloy_sol_types::{sol, SolCall};
use anyhow::Context;
use revm::{
    db::AccountState,
    primitives::{Address, Bytes, ExecutionResult, keccak256, Output, TxKind, U256, state::{Account, AccountInfo}},
    Database, DatabaseCommit, DatabaseRef, Evm,
};
use serde::de::DeserializeOwned;
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

fn from_bytes_option<V: DeserializeOwned, E>(
    key_opt: &Option<Vec<u8>>,
) -> Option<V>
where
    E: From<bcs::Error>,
{
    match key_opt {
        Some(bytes) => {
            let value = bcs::from_bytes(bytes).unwrap();
            Some(value)
        }
        None => None,
    }
}

#[repr(u8)]
pub enum KeyTag {
    /// Key prefix for the storage of the zero contract.
    ZeroContractAddress,
    /// Key prefix for the storage of the contract address.
    ContractAddress,
}

#[repr(u8)]
pub enum KeyCategory {
    AccountInfo,
    AccountState,
    Storage,
}


#[derive(Default)]
struct StorageStats {
    number_reset: u64,
    number_set: u64,
    number_release: u64,
    number_warm_read: u64,
    map: BTreeMap<U256, U256>,
}

struct LineraDatabase<C>
where
    C: TestKeyValueStore,
{
    commit_error: Option<C::Error>,
    storage_stats: Arc<Mutex<StorageStats>>,
    db: C,
}

impl<C> Database for LineraDatabase<C>
where
    C: TestKeyValueStore,
{
    type Error = C::Error;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.basic_ref(address)
    }

    fn code_by_hash(
        &mut self,
        _code_hash: B256,
    ) -> Result<revm::primitives::Bytecode, Self::Error> {
        panic!("Functionality code_by_hash not implemented");
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        self.storage_ref(address, index)
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        self.throw_error()?;
        <Self as DatabaseRef>::block_hash_ref(self, number)
    }
}

impl<C> DatabaseCommit for LineraDatabase<C>
where
    C: TestKeyValueStore,
{
    fn commit(&mut self, changes: HashMap<Address, Account>) {
        let result = self.commit_with_error(changes);
        if let Err(error) = result {
            self.commit_error = Some(error);
        }
    }
}

impl<C> LineraDatabase<C>
where
    C: TestKeyValueStore,
{
    fn commit_with_error(
        &mut self,
        changes: HashMap<Address, Account>,
    ) -> Result<(), C::Error> {
        println!("commit_with_error, beginning");
        let mut batch = Batch::new();
        let mut list_new_balances = Vec::new();
        let mut increment_number_reset = 0;
        let mut increment_number_set = 0;
        let mut increment_number_release = 0;
        for (address, account) in changes {
            if !account.is_touched() {
                continue;
            }
            let val = self.get_contract_address_key(&address);
            if let Some(val) = val {
                let key_prefix = vec![val, KeyCategory::Storage as u8];
                let key_info = vec![val, KeyCategory::AccountInfo as u8];
                let key_state = vec![val, KeyCategory::AccountState as u8];
                if account.is_selfdestructed() {
                    batch.delete_key_prefix(key_prefix);
                    batch.put_key_value(key_info, &AccountInfo::default())?;
                    batch.put_key_value(key_state, &AccountState::NotExisting)?;
                } else {
                    let is_newly_created = account.is_created();
                    batch.put_key_value(key_info, &account.info)?;

                    let account_state = if is_newly_created {
                        batch.delete_key_prefix(key_prefix);
                        AccountState::StorageCleared
                    } else {
                        let result = block_on(self.db.read_value_bytes(&key_state))?;
                        let account_state = from_bytes_option::<AccountState, ViewError>(&result)
                            .unwrap_or_default();
                        if account_state.is_storage_cleared() {
                            AccountState::StorageCleared
                        } else {
                            AccountState::Touched
                        }
                    };
                    batch.put_key_value(key_state, &account_state)?;
                    for (index, value) in account.storage {
                        let key = Self::get_uint256_key(val, index)?;
                        if value.original_value() == U256::ZERO {
                            if value.present_value() != U256::ZERO {
                                println!("DB:   WRITE(A) index={} value={}", index, value.present_value());
                                batch.put_key_value(key, &value.present_value())?;
                                increment_number_set += 1;
                            } else {
                                println!("DB:   WRITE(B) index={} value={}", index, value.present_value());
                            }
                        } else {
                            if value.present_value() != U256::ZERO {
                                if value.present_value() == value.original_value() {
                                    println!("DB:   WRITE(C) index={} value={}", index, value.present_value());
                                } else {
                                    println!("DB:   WRITE(D) index={} value={}", index, value.present_value());
                                    batch.put_key_value(key, &value.present_value())?;
                                    increment_number_reset += 1;
                                }
                            } else {
                                println!("DB:   WRITE(E) index={} value={}", index, value.present_value());
                                batch.delete_key(key);
                                increment_number_release += 1;
                            }
                        }
                    }
                }
            } else {
                if !account.storage.is_empty() {
                    panic!("For user account, storage must be empty");
                }
                // The only allowed operations are the ones for the
                // account balances.
                let new_balance = (address, account.info.balance);
                list_new_balances.push(new_balance);
            }
        }
        block_on(self.db.write_batch(batch))?;
        if !list_new_balances.is_empty() {
            panic!("The conversion Ethereum address / Linera address is not yet implemented");
        }
        println!("increment reset={} set={} release={}", increment_number_reset, increment_number_set, increment_number_release);
        let mut storage_stats = self.storage_stats.lock().expect("The lock should be possible");
        storage_stats.number_reset += increment_number_reset;
        storage_stats.number_set += increment_number_set;
        storage_stats.number_release += increment_number_release;
        Ok(())
    }
}

impl<C> DatabaseRef for LineraDatabase<C>
where
    C: TestKeyValueStore,
{
    type Error = C::Error;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, C::Error> {
        self.throw_error()?;
        let val = self.get_contract_address_key(&address);
        if let Some(val) = val {
            let key = vec![val, KeyCategory::AccountInfo as u8];
            let result = block_on(self.db.read_value_bytes(&key))?;
            let account_info = from_bytes_option::<AccountInfo, ViewError>(&result);
            return Ok(account_info);
        }
        panic!("only contract address are supported thus far address={address:?}");
    }

    fn code_by_hash_ref(
        &self,
        _code_hash: B256,
    ) -> Result<revm::primitives::Bytecode, C::Error> {
        panic!("Functionality code_by_hash_ref not implemented");
    }

    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, C::Error> {
        self.throw_error()?;
        let val = self.get_contract_address_key(&address);
        let Some(val) = val else {
            panic!("There is no storage associated to Externally Owned Account");
        };
        let mut storage_stats = self.storage_stats.lock().expect("The lock should be possible");
        match storage_stats.map.entry(index) {
            btree_map::Entry::Occupied(entry) => {
                let result = *entry.get();
                storage_stats.number_warm_read += 1;
                println!("DB:   READ(A:WARM) index={} result={}", index, result);
                Ok(result)
            },
            btree_map::Entry::Vacant(entry) => {
                let key = Self::get_uint256_key(val, index)?;
                let result = block_on(self.db.read_value_bytes(&key))?;
                let result = from_bytes_option::<U256, ViewError>(&result).unwrap_or_default();
                println!("DB:   READ(B:COLD) index={} result={}", index, result);
                entry.insert(result);
                Ok(result)
            },
        }
    }

    fn block_hash_ref(&self, number: u64) -> Result<B256, C::Error> {
        self.throw_error()?;
        Ok(keccak256(number.to_string().as_bytes()))
    }
}



impl<C> LineraDatabase<C>
where
    C: TestKeyValueStore,
{
    fn get_uint256_key(val: u8, index: U256) -> Result<Vec<u8>, C::Error> {
        let mut key = vec![val, KeyCategory::Storage as u8];
        bcs::serialize_into(&mut key, &index)?;
        Ok(key)
    }

    fn get_contract_address_key(&self, address: &Address) -> Option<u8> {
        println!("get_contract_address_key : address={}", address);
        let contract_address = Address::ZERO.create(0);
        if address == &Address::ZERO {
            return Some(KeyTag::ZeroContractAddress as u8);
        }
        if address == &contract_address {
            return Some(KeyTag::ContractAddress as u8);
        }
        None
    }

    fn throw_error(&self) -> Result<(), C::Error> {
        if let Some(error) = &self.commit_error {
            let error = format!("{:?}", error);
            panic!("Following error error={error}");
        }
        Ok(())
    }

    fn new(db: C) -> Self {
        let storage_stats = StorageStats::default();
        Self {
            commit_error: None,
            storage_stats: Arc::new(Mutex::new(storage_stats)),
            db,
        }
    }

    fn reset_storage_stats(&self) {
        let mut storage_stats = self.storage_stats.lock().expect("The lock should be possible");
        *storage_stats = StorageStats::default();
    }

    fn print_status(&self) {
        let storage_stats = self.storage_stats.lock().expect("The lock should be possible");
        println!("    number_reset = {}", storage_stats.number_reset);
        println!("      number_set = {}", storage_stats.number_set);
        println!("  number_release = {}", storage_stats.number_release);
        println!("number_warm_read = {}", storage_stats.number_warm_read);
        println!("number_cold_read = {}", storage_stats.map.len());
    }

}

#[derive(Debug)]
enum Operation {
    DeleteKey(U256),
    InsertKeyValue(U256,U256),
    InsertKeyValueBis(U256,U256),
    ReadValue(U256),
}



fn deploy_contract<DB: Database + DatabaseRef + DatabaseCommit>(
    db: &mut DB,
    bytecode: Bytes,
) -> anyhow::Result<Address> {
    println!("deploy_contract |bytecode|={}", bytecode.as_ref().len());
    let mut evm: Evm<'_, (), _> = Evm::builder()
        .with_ref_db(db)
        .modify_tx_env(|tx| {
            tx.clear();
            tx.transact_to = TxKind::Create;
            tx.data = bytecode;
        })
        .build();

    println!("Before transact_commit, deploy");
    let result = evm.transact_commit();
    println!(" After transact_commit, deploy");
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
    db: &mut DB,
    encoded_args: Bytes,
) -> anyhow::Result<Bytes> {
    let contract_address = Address::ZERO.create(0);
    let mut evm: Evm<'_, (), _> = Evm::builder()
        .with_ref_db(db)
        .modify_tx_env(|tx| {
            tx.transact_to = TxKind::Call(contract_address);
            tx.data = encoded_args;
        })
        .build();

    println!("Before transact_commit, call");
    let result = evm.transact_commit();
    println!(" After transact_commit, call");
    let Ok(result) = result else {
        anyhow::bail!("The transact_commit failed");
    };

    println!("result={:?}", result);

    let ExecutionResult::Success { output, .. } = result else {
        anyhow::bail!("Execution did not work out")
    };
    let Output::Call(result) = output else {
        anyhow::bail!("Only alternative is contract creation which is kind of unlikely")
    };
    Ok(result)
}

fn single_execution_operation<DB: Database + DatabaseRef + DatabaseCommit>(
    db: &mut DB,
    operation: Operation,
) -> anyhow::Result<()> {
    println!("--------------------------- operation={operation:?} ---------------------------------------");
    sol! {
        function insert_key_value(uint256 key, uint256 value);
        function insert_key_value_bis(uint256 key, uint256 value);
        function delete_key(uint256 key);
        function read_value(uint256 key);
    }

    let encoded_args = match operation {
        Operation::DeleteKey(key) => {
            let fct_args = delete_keyCall { key };
            fct_args.abi_encode().into()
        },
        Operation::InsertKeyValue(key, value) => {
            let fct_args = insert_key_valueCall { key, value };
            fct_args.abi_encode().into()
        },
        Operation::InsertKeyValueBis(key, value) => {
            let fct_args = insert_key_value_bisCall { key, value };
            fct_args.abi_encode().into()
        },
        Operation::ReadValue(key) => {
            let fct_args = read_valueCall { key };
            fct_args.abi_encode().into()
        },
    };
    single_execution(db, encoded_args)?;
    Ok(())
}




fn main() -> anyhow::Result<()> {
    let bytecode = {
        let source_code = r#"
contract ExampleKeyValueMap {
  mapping(uint256 => uint256) map;


  function insert_key_value(uint256 key, uint256 value) external returns (uint256) {
    map[key] = value;
  }

  function insert_key_value_bis(uint256 key, uint256 value) external returns (uint256) {
    map[key] = value;
    map[key] = value + 1;
  }

  function delete_key(uint256 key) external returns (uint256) {
    delete map[key];
  }

  function read_value(uint256 key) external returns (uint256) {
    return map[key];
  }

}
"#
        .to_string();

        get_bytecode(&source_code, "ExampleKeyValueMap")?
    };

    let vec: Vec<u8> = bytecode.to_vec();
    let tx_data = Bytes::copy_from_slice(&vec);

    let db = block_on(MemoryStore::new_test_store()).unwrap();
    let mut db = LineraDatabase::new(db);
    let contract_address = deploy_contract(&mut db, tx_data)?;
    assert_eq!(contract_address, Address::ZERO.create(0));


    for operation in [Operation::DeleteKey(U256::from(7)),
                      Operation::InsertKeyValue(U256::from(7), U256::from(5)),
                      Operation::InsertKeyValue(U256::from(7), U256::from(5)),
                      Operation::InsertKeyValue(U256::from(7), U256::from(7)),
                      Operation::InsertKeyValueBis(U256::from(7), U256::from(5)),
                      Operation::ReadValue(U256::from(7)),
                      Operation::DeleteKey(U256::from(7)),
                      Operation::ReadValue(U256::from(7)),
                      Operation::ReadValue(U256::from(5))] {
        single_execution_operation(&mut db, operation)?;
        db.print_status();
        db.reset_storage_stats();
    }

    println!("The single_execution has been successful");
    Ok(())
}
