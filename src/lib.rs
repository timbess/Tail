use std::fs::{File, Metadata};
use std::io::{Seek, BufReader, SeekFrom, Read, BufWriter, Write};
use std::collections::{VecDeque};

const BUFFER_SIZE: u64 = 4096;

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

pub struct BackwardsReader<'a> {
    pieces: VecDeque<VecDeque<Vec<u8>>>,
    num_of_lines: usize,
    fd: &'a mut BufReader<File>,
    total_newlines: usize,
    first_read: bool,
    last_offset: u64
}

impl<'a> BackwardsReader<'a> {
    pub fn new(num_of_lines: usize, fd: &'a mut BufReader<File>) -> Self {
        let last_offset = fd.seek(SeekFrom::End(0))
                                .unwrap_or_else(|_| { panic!("Failed to seek to end of file") });
        BackwardsReader {
            pieces: VecDeque::with_capacity(num_of_lines),
            num_of_lines: num_of_lines,
            fd: fd,
            total_newlines: 0,
            first_read: true,
            last_offset: last_offset
        }
    }

    fn read(&mut self) -> bool {
        match self.fd.seek(SeekFrom::Start((self.last_offset as u64) - BUFFER_SIZE)) {
            Ok(new_offset) => {
                self.last_offset = new_offset;
            },
            Err(_) => {
                if self.last_offset > 0 {
                    self.fd.seek(SeekFrom::Start(0)).unwrap();
                    let mut buff = vec![0; (self.last_offset) as usize];
                    self.fd.read_exact(buff.as_mut_slice())
                        .unwrap_or_else(|_| { panic!("Incorrectly handled unexpected EOF. Probably an off by one error") });
                    if self.first_read && buff[buff.len() - 1] != b'\n' {
                        self.total_newlines += 1;
                        self.first_read = false;
                        buff.push(b'\n');
                    }
                    let mut buff: VecDeque<Vec<u8>> = buff.split(|elm: &u8| {*elm == b'\n'}).map(|elm: &[u8]| elm.to_vec()).collect();
                    self.total_newlines += buff.len() - 1;
                    self.pieces.push_front(buff);
                }
                return false;
            }
        }

        let mut buff = vec![0; BUFFER_SIZE as usize];
        self.fd.read_exact(buff.as_mut_slice())
            .unwrap_or_else(|_| { panic!("Failed to read from end of file in BackwardsReader") });
        if self.first_read && buff[buff.len() - 1] != b'\n' {
            self.total_newlines += 1;
            self.first_read = false;
            buff.push(b'\n');
        }
        let buff: VecDeque<Vec<u8>> = buff.split(|elm: &u8| {*elm == b'\n'}).map(|elm: &[u8]| elm.to_vec()).collect();
        self.total_newlines += buff.len() - 1;
        self.pieces.push_front(buff);
        
        if self.first_read {
            self.first_read = false;
        }
        self.total_newlines < self.num_of_lines
    }

    pub fn read_all(&mut self, writer: &mut BufWriter<std::io::Stdout>) {
        while self.read() {}

        // If we hit the top of the file early, there's no guarantee
        // that total_newlines will be greater than num_of_lines due
        // to the way failed backward seeks are handled in read()
        if self.total_newlines > self.num_of_lines {
            let mut first_chunk = self.pieces.pop_front().unwrap();
            let pieces_to_discard = self.total_newlines - self.num_of_lines as usize;
            if pieces_to_discard > 0 {
                for _ in 0..pieces_to_discard {
                    first_chunk.pop_front().unwrap();
                }
                self.total_newlines -= pieces_to_discard;
            }
            self.pieces.push_front(first_chunk);
        }

        if self.pieces.is_empty() { return; }

        let mut line: Vec<u8> = Vec::new();
        while let Some(mut piece) = self.pieces.pop_front() {
            if piece.len() == 1 {
                line.append(piece.pop_front().unwrap().as_mut());
            } else if piece.len() > 1 {
                let mut last_chunk = piece.pop_back().unwrap();
                for mut chunk in piece {
                    line.append(&mut chunk);
                    line.push(b'\n');
                    writer.write(&line).unwrap();
                    line.clear();
                }
                line.append(&mut last_chunk);
            }
        }
        if !line.is_empty() {
            writer.write(&line).unwrap();
        }
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