pub mod file_data_storage;
pub mod mmap_data_storage;

use log::{debug, error, info};
use std::{
    fs::{File, Metadata},
    ops::Deref,
    path::{Path, PathBuf},
};
use thiserror::Error;

use common::{
    create_file,
    formatter::{
        self, get_formatter_from_file, BitcaskFormatter, FormatterError, RowToWrite,
        FILE_HEADER_SIZE,
    },
    fs::{self, FileType},
    storage_id::StorageId,
};

use self::file_data_storage::FileDataStorage;

use super::{
    common::{RowToRead, Value},
    RowLocation, TimedValue,
};

#[derive(Error, Debug)]
#[error("{}")]
pub enum DataStorageError {
    #[error("Write data file with id: {0} failed. error: {1}")]
    WriteRowFailed(StorageId, String),
    #[error("Read data file with id: {0} failed. error: {1}")]
    ReadRowFailed(StorageId, String),
    #[error("Flush writing storage with id: {0} failed. error: {1}")]
    FlushStorageFailed(StorageId, String),
    #[error("Transit writing storage with id: {0} to readonly failed. error: {1}")]
    TransitToReadOnlyFailed(StorageId, String),
    #[error("Storage with id: {0} overflow, need replace with a new one")]
    StorageOverflow(StorageId),
    #[error("No permission to write storage with id: {0}")]
    PermissionDenied(StorageId),
    #[error("Got IO Error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Got IO Error: {0}")]
    DataStorageFormatter(#[from] FormatterError),
    #[error("Failed to read file header for storage with id: {1}")]
    ReadFileHeaderError(#[source] FormatterError, StorageId),
}

pub type Result<T> = std::result::Result<T, DataStorageError>;

pub trait DataStorageWriter {
    fn write_row<V: Deref<Target = [u8]>>(&mut self, row: &RowToWrite<V>) -> Result<RowLocation>;

    fn transit_to_readonly(self) -> Result<DataStorage>;

    fn flush(&mut self) -> Result<()>;
}

pub trait DataStorageReader {
    /// Read value from this storage at row_offset
    fn read_value(&mut self, row_offset: u64) -> Result<TimedValue<Value>>;

    /// Read next value from this storage
    fn read_next_row(&mut self) -> Result<Option<RowToRead>>;
}
#[derive(Debug)]
enum DataStorageImpl {
    FileStorage(FileDataStorage),
}

#[derive(Debug, Clone, Copy)]
pub struct DataStorageOptions {
    pub max_data_file_size: usize,
    pub init_data_file_capacity: usize,
}

impl Default for DataStorageOptions {
    fn default() -> Self {
        Self {
            max_data_file_size: 128 * 1024 * 1024,
            init_data_file_capacity: 1024 * 1024,
        }
    }
}

impl DataStorageOptions {
    pub fn max_data_file_size(mut self, size: usize) -> DataStorageOptions {
        assert!(size > 0);
        self.max_data_file_size = size;
        self
    }

    pub fn init_data_file_capacity(mut self, capacity: usize) -> DataStorageOptions {
        assert!(capacity > 0);
        self.init_data_file_capacity = capacity;
        self
    }
}

#[derive(Debug)]
pub struct DataStorage {
    database_dir: PathBuf,
    storage_id: StorageId,
    storage_impl: DataStorageImpl,
    readonly: bool,
    options: DataStorageOptions,
    dirty: bool,
}

impl DataStorage {
    pub fn new<P: AsRef<Path>>(
        database_dir: P,
        storage_id: StorageId,
        options: DataStorageOptions,
    ) -> Result<Self> {
        let path = database_dir.as_ref().to_path_buf();
        let formatter = BitcaskFormatter::default();
        let data_file = create_file(
            &path,
            FileType::DataFile,
            Some(storage_id),
            &formatter,
            options.init_data_file_capacity,
        )?;

        debug!(
            "Create storage under path: {:?} with storage id: {}",
            &path, storage_id
        );
        let meta = data_file.metadata()?;

        DataStorage::open_by_file(
            &path,
            storage_id,
            data_file,
            meta,
            FILE_HEADER_SIZE as u64,
            formatter,
            options,
        )
    }

    pub fn open<P: AsRef<Path>>(
        database_dir: P,
        storage_id: StorageId,
        options: DataStorageOptions,
    ) -> Result<Self> {
        let path = database_dir.as_ref().to_path_buf();
        let mut data_file = fs::open_file(&path, FileType::DataFile, Some(storage_id))?;
        debug!(
            "Open storage under path: {:?} with storage id: {}",
            &path, storage_id
        );
        let meta = data_file.file.metadata()?;
        let formatter = get_formatter_from_file(&mut data_file.file)?;
        if !meta.permissions().readonly() {
            let storage = DataStorage::open_by_file(
                &path,
                storage_id,
                data_file.file,
                meta,
                FILE_HEADER_SIZE as u64,
                formatter,
                options,
            );
            let iter = storage.iter();
            // Todo need to optimise this
            for _ in iter {
                debug!("loop iter");
            }
            return storage;
        }

        let offset = meta.len();
        DataStorage::open_by_file(
            &path,
            storage_id,
            data_file.file,
            meta,
            offset,
            formatter,
            options,
        )
    }

    pub fn storage_id(&self) -> StorageId {
        self.storage_id
    }

    pub fn is_readonly(&self) -> Result<bool> {
        Ok(self.readonly)
    }

    pub fn is_dirty(&mut self) -> bool {
        self.dirty
    }

    pub fn iter(&self) -> Result<StorageIter> {
        let mut data_file = fs::open_file(
            &self.database_dir,
            FileType::DataFile,
            Some(self.storage_id),
        )?;
        debug!(
            "Create iterator under path: {:?} with storage id: {}",
            &self.database_dir, self.storage_id
        );
        let formatter = formatter::get_formatter_from_file(&mut data_file.file)
            .map_err(|e| DataStorageError::ReadFileHeaderError(e, self.storage_id))?;
        let meta = data_file.file.metadata()?;
        let offset = meta.len();
        Ok(StorageIter {
            storage: DataStorage::open_by_file(
                &self.database_dir,
                self.storage_id,
                data_file.file,
                meta,
                offset,
                formatter,
                self.options,
            )?,
        })
    }

    fn open_by_file(
        database_dir: &PathBuf,
        storage_id: StorageId,
        data_file: File,
        meta: Metadata,
        write_offset: u64,
        formatter: BitcaskFormatter,
        options: DataStorageOptions,
    ) -> Result<Self> {
        let capacity = meta.len();
        Ok(DataStorage {
            storage_impl: DataStorageImpl::FileStorage(FileDataStorage::new(
                database_dir,
                storage_id,
                data_file,
                write_offset,
                capacity,
                formatter,
                options,
            )?),
            storage_id,
            database_dir: database_dir.clone(),
            readonly: meta.permissions().readonly(),
            options,
            dirty: false,
        })
    }
}

impl DataStorageWriter for DataStorage {
    fn write_row<V: Deref<Target = [u8]>>(&mut self, row: &RowToWrite<V>) -> Result<RowLocation> {
        if self.readonly {
            return Err(DataStorageError::PermissionDenied(self.storage_id));
        }
        let r = match &mut self.storage_impl {
            DataStorageImpl::FileStorage(s) => s.write_row(row),
        }?;
        self.dirty = true;
        Ok(r)
    }

    fn transit_to_readonly(self) -> Result<DataStorage> {
        match self.storage_impl {
            DataStorageImpl::FileStorage(s) => {
                let storage_id = s.storage_id;
                s.transit_to_readonly().map_err(|e| {
                    DataStorageError::TransitToReadOnlyFailed(storage_id, e.to_string())
                })
            }
        }
    }

    fn flush(&mut self) -> Result<()> {
        match &mut self.storage_impl {
            DataStorageImpl::FileStorage(s) => s
                .flush()
                .map_err(|e| DataStorageError::FlushStorageFailed(s.storage_id, e.to_string())),
        }
    }
}

impl DataStorageReader for DataStorage {
    fn read_value(&mut self, row_offset: u64) -> Result<TimedValue<Value>> {
        match &mut self.storage_impl {
            DataStorageImpl::FileStorage(s) => s
                .read_value(row_offset)
                .map_err(|e| DataStorageError::ReadRowFailed(s.storage_id, e.to_string())),
        }
    }

    fn read_next_row(&mut self) -> Result<Option<RowToRead>> {
        match &mut self.storage_impl {
            DataStorageImpl::FileStorage(s) => s.read_next_row(),
        }
    }
}

#[derive(Debug)]
pub struct StorageIter {
    storage: DataStorage,
}

impl Iterator for StorageIter {
    type Item = Result<RowToRead>;

    fn next(&mut self) -> Option<Self::Item> {
        let ret = self.storage.read_next_row();
        match ret {
            Ok(o) => o.map(Ok),
            Err(e) => {
                error!(target: "Storage", "Data file with file id {} was corrupted. Error: {}", 
                self.storage.storage_id(), &e);
                None
            }
        }
    }
}
