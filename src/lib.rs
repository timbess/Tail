use std::fs::{File, Metadata};
use std::io::{Seek, BufReader, SeekFrom};

pub enum ModificationType {
    Added,
    Removed,
    NoChange,
}

#[allow(dead_code)]
pub enum Input {
    File(File),
    Stdin(std::io::Stdin),
}

pub struct RingBuffer<T> {
    backing_arr: Box<[Option<T>]>,
    tail: usize,
    head: usize
}

impl<T: std::clone::Clone> RingBuffer<T> {
    pub fn new(cap: usize) -> Self {
        RingBuffer {
            backing_arr: vec![Default::default(); cap].into_boxed_slice(),
            tail: 0,
            head: 0
        }
    }

    pub fn push_front(&mut self, elm: T) {
        let new_tail = (self.tail + 1) % self.backing_arr.len();
        if new_tail == self.head {
            self.head = (self.head + 1) % self.backing_arr.len();
        }
        std::mem::replace(&mut self.backing_arr[self.tail], Some(elm));
        self.tail = new_tail;
    }

    #[allow(dead_code)]
    pub fn pop_front(&mut self) -> Option<T> {
        if self.head == self.tail {
            return None;
        }
        // Handle negative modulus correctly. Unfortunately % is remainder not modulo
        self.tail = (((self.tail - 1) % self.backing_arr.len()) + self.backing_arr.len()) % self.backing_arr.len();
        self.backing_arr[self.tail].take()
   }

    pub fn pop_back(&mut self) -> Option<T> {
        if self.head == self.tail {
            return None;
        }
        let ret = self.backing_arr[self.head].take();
        self.head = (self.head + 1) % self.backing_arr.len();
        ret
   }
}

#[derive(Debug)]
pub struct StatefulFile {
    pub fd: BufReader<File>,
    pub old_metadata: Metadata,
    file_name: String,
    cursor: SeekFrom,
}

impl StatefulFile {
    pub fn new(fd: File, file_name: String) -> Self {
        StatefulFile {
            old_metadata: fd.metadata()
                .unwrap_or_else(|_| { panic!("Could not retrieve metadata for file: {}", &file_name) }),
            fd: BufReader::new(fd),
            file_name: file_name,
            cursor: SeekFrom::Start(0),
        }
    }

    pub fn update_metadata(&mut self) {
        self.old_metadata = self.fd.get_ref().metadata()
            .unwrap_or_else(|_| { panic!("Could not retrieve metadata for file: {}", self.file_name) });
    }

    pub fn modification_type(&self) -> ModificationType {
        let new_metadata = self.fd.get_ref().metadata()
            .unwrap_or_else(|_| { panic!("Could not retrieve metadata for file: {}", self.file_name) });
        if new_metadata.len() > self.old_metadata.len() {
            ModificationType::Added
        } else if new_metadata.len() < self.old_metadata.len() {
            ModificationType::Removed
        } else {
            ModificationType::NoChange
        }
    }

    pub fn seek_to_cursor(&mut self) {
        self.fd.seek(self.cursor).unwrap();
    }

    pub fn update_cursor(&mut self) {
        self.cursor = SeekFrom::Start(self.fd.seek(SeekFrom::Current(0)).unwrap());
    }

    pub fn reset_cursor(&mut self) {
        self.cursor = SeekFrom::Start(0);
    }
}