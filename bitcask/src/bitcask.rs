use std::path::Path;
use std::sync::{Arc, RwLock};

use log::info;

use crate::database::{DataBaseOptions, Database};
use crate::error::{BitcaskError, BitcaskResult};
use crate::file_id::FileIdGenerator;
use crate::file_manager;
use crate::keydir::KeyDir;
use crate::utils::{is_tombstone, TOMBSTONE_VALUE};

pub const DEFAULT_BITCASK_OPTIONS: BitcaskOptions = BitcaskOptions {
    max_file_size: 128 * 1024 * 1024,
    max_key_size: 64,
    max_value_size: 100 * 1024,
};

#[derive(Debug, Clone, Copy)]
pub struct BitcaskOptions {
    pub max_file_size: usize,
    pub max_key_size: usize,
    pub max_value_size: usize,
}

impl BitcaskOptions {
    fn validate(&self) -> Option<BitcaskError> {
        if self.max_file_size <= 0 {
            Some(BitcaskError::InvalidParameter(
                "max_file_size".into(),
                "need a positive value".into(),
            ));
        }
        if self.max_key_size <= 0 {
            Some(BitcaskError::InvalidParameter(
                "max_key_size".into(),
                "need a positive value".into(),
            ));
        }
        if self.max_value_size <= 0 {
            Some(BitcaskError::InvalidParameter(
                "max_value_size".into(),
                "need a positive value".into(),
            ));
        }
        None
    }

    fn get_database_options(&self) -> DataBaseOptions {
        return DataBaseOptions {
            max_file_size: self.max_file_size,
        };
    }
}

#[derive(PartialEq)]
enum FoldStatus {
    Stopped,
    Continue,
}
pub struct FoldResult<T> {
    accumulator: T,
    status: FoldStatus,
}

pub struct Bitcask {
    keydir: RwLock<KeyDir>,
    file_id_generator: Arc<FileIdGenerator>,
    options: BitcaskOptions,
    database: Database,
}

impl Bitcask {
    pub fn open(directory: &Path, options: BitcaskOptions) -> BitcaskResult<Bitcask> {
        let valid_opt = options.validate();
        if valid_opt.is_some() {
            return Err(valid_opt.unwrap());
        }
        let file_id_generator = Arc::new(FileIdGenerator::new());
        let database = Database::open(
            &directory,
            file_id_generator.clone(),
            options.get_database_options(),
        )?;
        let keydir = KeyDir::new(&database)?;
        Ok(Bitcask {
            keydir: RwLock::new(keydir),
            file_id_generator,
            database,
            options,
        })
    }

    pub fn put(&self, key: Vec<u8>, value: &[u8]) -> BitcaskResult<()> {
        if key.len() > self.options.max_key_size {
            return Err(BitcaskError::InvalidParameter(
                "key".into(),
                "key size overflow".into(),
            ));
        }
        if value.len() > self.options.max_value_size {
            return Err(BitcaskError::InvalidParameter(
                "value".into(),
                "values size overflow".into(),
            ));
        }

        let kd = self.keydir.write().unwrap();
        let ret = self.database.write(&key, value)?;
        kd.put(key, ret);
        Ok(())
    }

    pub fn get(&self, key: &Vec<u8>) -> BitcaskResult<Option<Vec<u8>>> {
        let row_pos = {
            self.keydir
                .read()
                .unwrap()
                .get(key)
                .and_then(|r| Some(r.value().clone()))
        };

        match row_pos {
            Some(e) => {
                let v = self.database.read_value(&e)?;
                if is_tombstone(&v) {
                    return Ok(None);
                }
                Ok(Some(v))
            }
            None => Ok(None),
        }
    }

    pub fn foreach_key<T>(&self, func: fn(key: &Vec<u8>) -> FoldResult<T>) {
        let kd = self.keydir.read().unwrap();
        for r in kd.iter() {
            if func(r.key()).status == FoldStatus::Stopped {
                break;
            }
        }
    }

    pub fn delete(&self, key: &Vec<u8>) -> BitcaskResult<()> {
        let kd = self.keydir.write().unwrap();

        if kd.contains_key(key) {
            self.database.write(key, TOMBSTONE_VALUE.as_bytes())?;
            kd.delete(&key);
        }

        Ok(())
    }

    pub fn merge(&self) -> BitcaskResult<()> {
        let dir_path = file_manager::create_merge_file_dir(self.database.get_database_dir())?;
        let (kd, known_max_file_id) = self.flush_writing_file()?;
        let (file_ids, new_kd) = self.write_merged_files(&dir_path, &kd)?;

        file_manager::commit_merge_files(self.database.get_database_dir())?;

        let kd = self.keydir.write().unwrap();
        for (k, v) in new_kd.into_iter() {
            kd.put(k, v)
        }

        self.database.load_files(file_ids)?;
        self.database.purge_outdated_files(known_max_file_id)?;
        Ok(())
    }

    fn flush_writing_file(&self) -> BitcaskResult<(KeyDir, u32)> {
        // stop writing and switch the writing file to stable files
        let _kd = self.keydir.write().unwrap();
        self.database.flush_writing_file()?;
        let known_max_file_id = self.database.get_max_file_id();
        Ok((_kd.clone(), known_max_file_id))
    }

    fn write_merged_files(
        &self,
        merge_file_dir: &Path,
        key_dir_to_write: &KeyDir,
    ) -> BitcaskResult<(Vec<u32>, KeyDir)> {
        let new_kd = KeyDir::new_empty_key_dir();
        let merge_db = Database::open(
            merge_file_dir,
            self.file_id_generator.clone(),
            self.options.get_database_options(),
        )?;

        for r in key_dir_to_write.iter() {
            let k = r.key();
            let v = self.database.read_value(r.value())?;
            if !is_tombstone(&v) {
                let pos = merge_db.write_with_timestamp(k, &v, r.value().tstmp)?;
                new_kd.checked_put(k.clone(), pos)
            }
        }
        merge_db.flush_writing_file()?;
        let file_ids = merge_db.get_file_ids();
        Ok((file_ids, new_kd))
    }
}
