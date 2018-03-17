extern crate tail;

use tail::BackwardsReader;
use std::io::{BufReader, BufWriter};
use std::fs::File;

fn main() {
    let filename = std::env::args().nth(1).unwrap_or("/var/log/syslog".to_string());
    let fd = File::open(filename).unwrap();
    let mut fd = BufReader::new(fd);
    let mut reader = BackwardsReader::new(10, &mut fd);

    let mut out = BufWriter::new(std::io::stdout());
    reader.read_all(&mut out);
}