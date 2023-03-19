use std::{ops::Add, sync::Mutex};

#[derive(Debug)]
pub struct FileIdGenerator {
    file_id: Mutex<u32>,
}

impl FileIdGenerator {
    pub fn new() -> FileIdGenerator {
        FileIdGenerator {
            file_id: Mutex::new(0),
        }
    }

    pub fn generate_next_file_id(&self) -> u32 {
        let mut id = self.file_id.lock().unwrap();
        let next_id = id.add(1);
        *id = next_id;
        next_id
    }

    pub fn update_file_id(&self, current_max_file_id: u32) {
        let mut id = self.file_id.lock().unwrap();
        *id = current_max_file_id;
    }

    pub fn get_file_id(&self) -> u32 {
        *self.file_id.lock().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_log::test;

    #[test]
    fn test_generate_id() {
        let id_gen = FileIdGenerator::new();
        assert_eq!(1, id_gen.generate_next_file_id());
        assert_eq!(2, id_gen.generate_next_file_id());
        assert_eq!(3, id_gen.generate_next_file_id());
        assert_eq!(3, id_gen.get_file_id());
    }

    #[test]
    fn test_update_file_id() {
        let id_gen = FileIdGenerator::new();
        assert_eq!(1, id_gen.generate_next_file_id());
        id_gen.update_file_id(10);
        assert_eq!(11, id_gen.generate_next_file_id());
        assert_eq!(12, id_gen.generate_next_file_id());
        assert_eq!(12, id_gen.get_file_id());
    }
}
