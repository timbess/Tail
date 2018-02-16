extern crate inotify;
extern crate getopts;

use std::path::Path;
use std::iter::Iterator;
use std::fs::{File, Metadata};
use std::collections::{HashMap};
use std::io::{Read, BufRead, Seek, BufReader, SeekFrom};
use inotify::{Inotify, WatchMask, EventMask};
use getopts::Options;

#[allow(dead_code)]
static USAGE: &'static str = r#"Usage: tail [OPTION]... [FILE]...
Print the last 10 lines of each FILE to standard output.
With more than one FILE, precede each with a header giving the file name.

With no FILE, or when FILE is -, read standard input.

Mandatory arguments to long options are mandatory for short options too.
  -c, --bytes=[+]NUM      output the last NUM bytes; or use -c +NUM to
                             output starting with byte NUM of each file
  -f, --follow            output appended data as the file grows;
  -F                       same as --follow=name --retry
  -n, --lines=[+]NUM       output the last NUM lines, instead of the last 10;
                             or use -n +NUM to output starting with line NUM
  -q, --quiet              never output headers giving file names
  -v, --verbose            always output headers giving file names
  -h, --help     display this help and exit
  -V, --version  output version information and exit

NUM may have a multiplier suffix:
b 512, kB 1000, K 1024, MB 1000*1000, M 1024*1024,
GB 1000*1000*1000, G 1024*1024*1024, and so on for T, P, E, Z, Y.

With --follow (-f), tail defaults to following the file descriptor, which
means that even if a tail'ed file is renamed, tail will continue to track
its end.  This default behavior is not desirable when you really want to
track the actual name of the file, not the file descriptor (e.g., log
rotation).  Use --follow=name in that case.  That causes tail to track the
named file in a way that accommodates renaming, removal and creation.
"#;

enum ModificationType {
    Added,
    Removed,
    NoChange,
}

#[allow(dead_code)]
enum Input {
    File(File),
    Stdin(std::io::Stdin),
}

struct RingBuffer<T> {
    backing_arr: Box<[Option<T>]>,
    tail: usize,
    head: usize
}

impl<T: std::clone::Clone> RingBuffer<T> {
    fn new(cap: usize) -> Self {
        RingBuffer {
            backing_arr: vec![Default::default(); cap].into_boxed_slice(),
            tail: 0,
            head: 0
        }
    }

    fn push_front(&mut self, elm: T) {
        if self.backing_arr[self.tail].is_some() {
            self.head = (self.head + 1) % self.backing_arr.len();
        }
        std::mem::replace(&mut self.backing_arr[self.tail], Some(elm));
        self.tail = (self.tail + 1) % self.backing_arr.len();
    }

    #[allow(dead_code)]
    fn pop_front(&mut self) -> Option<T> {
        if self.head == self.tail {
            return None;
        }
        // Handle negative modulus correctly. Unforunately % is remainder not modulo
        self.tail = (((self.tail - 1) % self.backing_arr.len()) + self.backing_arr.len()) % self.backing_arr.len();
        self.backing_arr[self.tail].take()
   }

    fn pop_back(&mut self) -> Option<T> {
        if self.head == self.tail {
            return None;
        }
        let ret = self.backing_arr[self.head].take();
        self.head = (self.head + 1) % self.backing_arr.len();
        ret
   }
}

#[derive(Debug)]
struct StatefulFile {
    pub fd: BufReader<File>,
    pub old_metadata: Metadata,
    file_name: String,
    cursor: SeekFrom,
}

impl StatefulFile {
    fn new(fd: File, file_name: String) -> Self {
        StatefulFile {
            old_metadata: fd.metadata()
                .unwrap_or_else(|_| { panic!("Could not retrieve metadata for file: {}", &file_name) }),
            fd: BufReader::new(fd),
            file_name: file_name,
            cursor: SeekFrom::Start(0),
        }
    }

    fn update_metadata(&mut self) {
        self.old_metadata = self.fd.get_ref().metadata()
            .unwrap_or_else(|_| { panic!("Could not retrieve metadata for file: {}", self.file_name) });
    }

    fn modification_type(&self) -> ModificationType {
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

    fn seek_to_cursor(&mut self) {
        self.fd.seek(self.cursor).unwrap();
    }

    fn update_cursor(&mut self) {
        self.cursor = SeekFrom::Start(self.fd.seek(SeekFrom::Current(0)).unwrap());
    }

    fn reset_cursor(&mut self) {
        self.cursor = SeekFrom::Start(0);
    }
}

fn print_usage() {
    print!("{}", USAGE);
    std::process::exit(0);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut opts = Options::new();
    opts.optopt("c", "bytes", "output the last NUM bytes", "NUM");
    opts.optflag("f", "follow", "output appended as the file grows");
    opts.optflag("F", "", "same as follow with --retry");
    opts.optopt("n", "lines", "output the last NUM lines, instead of the last 10", "NUM");
    opts.optflag("h", "help", "print this help menu");
    opts.optflag("V", "version", "version of program");

    let matches = match opts.parse(&args[1..]) {
        Ok(m) => m,
        Err(f) => { panic!(f.to_string()) }
    };

    if matches.opt_present("h") {
        print_usage();
    }
    if matches.opt_present("V") {
        println!("tail version {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    if matches.free.is_empty() {
        eprintln!("Error: Must have at least one file in arguments");
        print_usage();
    }


    let follow_opt = matches.opt_present("f");
    let num_of_lines = matches.opt_str("n").unwrap_or(String::from("10"));
    let file_names: Vec<String> = matches.free;

    let mut watcher = Inotify::init().expect("Inotify failed to initialize");
    let mut files = HashMap::new();
    for file_name in file_names {
        let mut wd = watcher.add_watch(Path::new(&file_name), WatchMask::MODIFY)
            .unwrap_or_else(|_| panic!("Failed to attach watcher to file: {}", &file_name));
        let mut fd = File::open(&file_name)
            .unwrap_or_else(|_| panic!("Failed to open file handle for: {}", &file_name));
        let mut sf = StatefulFile::new(fd, file_name);
        initial_print(&mut sf, &num_of_lines);
        sf.update_cursor();
        files.insert(wd, sf);
    }

    if follow_opt {
        let mut buffer = [0u8; 4096];
        loop {
            let events = watcher.read_events_blocking(&mut buffer)
                .expect("Failed to read inotify events");

            for event in events {
                if event.mask.contains(EventMask::MODIFY) {
                    let sf = files.get_mut(&event.wd).unwrap();
                    follow(sf);
                }
            }
        }
    }
}

fn follow(sf: &mut StatefulFile) {
    match sf.modification_type() {
        ModificationType::Added => {}
        ModificationType::Removed => {
            sf.reset_cursor();
        }
        ModificationType::NoChange => {}
    }
    sf.update_metadata();
    sf.seek_to_cursor();
    print_from_cursor(sf);
    sf.update_cursor();
}

fn initial_print(sf: &mut StatefulFile, num_lines_str: &String) {
    let line_iter = sf.fd.by_ref().lines().map(|l| l.unwrap());
    if num_lines_str.starts_with("+") {
        let line_iter = line_iter.skip(num_lines_str.chars().skip(1).collect::<String>().parse::<usize>()
            .unwrap_or_else(|_| panic!("Incorrect number of lines given: {}", &num_lines_str)));
        for line in line_iter {
            println!("{}", line);
        }
        return;
    }
    let num_lines = num_lines_str.parse::<usize>()
        .unwrap_or_else(|_| panic!("Incorrect number of lines given: {}", &num_lines_str));

    let mut last_n_lines = RingBuffer::new(num_lines);
    for line in line_iter {
        last_n_lines.push_front(line);
    }
    while let Some(line) = last_n_lines.pop_back() {
        println!("{}", line);
    }
}

fn print_from_cursor(sf: &mut StatefulFile) {
    for line in sf.fd.by_ref().lines().map(|l| l.unwrap()) {
        println!("{}", line);
    }
}
