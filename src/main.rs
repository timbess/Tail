extern crate inotify;
extern crate argparse;

use std::path::Path;
use std::iter::Iterator;
use std::fs::{File, Metadata};
use std::os::linux::fs::MetadataExt;
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Lines, BufRead, Seek, BufReader, SeekFrom};
use inotify::{Inotify, WatchMask, WatchDescriptor, EventMask, Event};
use argparse::{ArgumentParser, Print, Collect, StoreTrue};

static MAN_PAGE: &'static str = r#"
NAME
       tail - output the last part of files

SYNOPSIS
       tail [OPTION]... [FILE]...

DESCRIPTION
       Print the last 10 lines of each FILE to standard output.  With more than one FILE, precede each
       with a header giving the file name.

       With no FILE, or when FILE is -, read standard input.

       Mandatory arguments to long options are mandatory for short options too.

       -c, --bytes=[+]NUM
              output the last NUM bytes; or use -c +NUM to output starting with byte NUM of each file

       -f, --follow[={name|descriptor}]
              output appended data as the file grows;

              an absent option argument means 'descriptor'

       -F     same as --follow=name --retry

       -n, --lines=[+]NUM
              output the last NUM lines, instead of the last 10; or use -n  +NUM  to  output  starting
              with line NUM

       --max-unchanged-stats=N
              with --follow=name, reopen a FILE which has not

              changed  size  after  N (default 5) iterations to see if it has been unlinked or renamed
              (this is the usual case of rotated log files); with inotify, this option is rarely  use‐
              ful

       --pid=PID
              with -f, terminate after process ID, PID dies

       -q, --quiet, --silent
              never output headers giving file names

       --retry
              keep trying to open a file if it is inaccessible

       -s, --sleep-interval=N
              with  -f,  sleep for approximately N seconds (default 1.0) between iterations; with ino‐
              tify and --pid=P, check process P at least once every N seconds

       -v, --verbose
              always output headers giving file names

       -z, --zero-terminated
              line delimiter is NUL, not newline

       --help display this help and exit

       --version
              output version information and exit

       NUM may have a multiplier suffix: b 512, kB  1000,  K  1024,  MB  1000*1000,  M  1024*1024,  GB
       1000*1000*1000, G 1024*1024*1024, and so on for T, P, E, Z, Y.

       With  --follow (-f), tail defaults to following the file descriptor, which means that even if a
       tail'ed file is renamed, tail will continue to track its end.  This  default  behavior  is  not
       desirable  when  you  really want to track the actual name of the file, not the file descriptor
       (e.g., log rotation).  Use --follow=name in that case.  That causes tail  to  track  the  named
       file in a way that accommodates renaming, removal and creation.

AUTHOR
       Written by Timothy Bess

"#;

enum ModificationType {
    Added,
    Removed,
    NoChange,
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
        self.fd.seek(self.cursor);
        let offset = self.fd.seek(SeekFrom::Current(0)).unwrap();
        println!("Seek to offset: {}", offset);
    }

    fn update_cursor(&mut self) {
        let offset = self.fd.seek(SeekFrom::Current(0)).unwrap();
        println!("Updating offset to: {}", offset);
        self.cursor = SeekFrom::Start(self.fd.seek(SeekFrom::Current(0)).unwrap());
    }

    fn reset_cursor(&mut self) {
        self.cursor = SeekFrom::Start(0);
    }
}

fn main() {
    let mut file_names: Vec<String> = Vec::new();
    let mut follow_opt = false;
    {
        let mut parser = ArgumentParser::new();
        parser.set_description("Print the last 10 lines of each FILES to standard output.  With more than one FILES, precede each \
                                with a header giving the file name. \
                                With no FILES, or when FILES is -, read standard input.");
        parser.add_option(&["-V", "--version"],
                          Print(env!("CARGO_PKG_VERSION").to_string()), "Print version");
        parser.refer(&mut follow_opt).add_option(&["-f", "--follow"],
                                                 StoreTrue, "Output appended data as the file grows");
        parser.refer(&mut file_names)
            .add_argument("files", Collect, "FILES");
        parser.parse_args_or_exit();
    }
    let mut watcher = Inotify::init().expect("Inotify failed to initialize");
    let mut files = HashMap::new();
    for file_name in file_names {
        let mut wd = watcher.add_watch(Path::new(&file_name),
                                       WatchMask::CLOSE_WRITE | WatchMask::MODIFY)
            .unwrap_or_else(|_| panic!("Failed to attach watcher to file: {}", &file_name));
        let mut fd = File::open(&file_name)
            .unwrap_or_else(|_| panic!("Failed to open file handle for: {}", &file_name));
        let mut sf = StatefulFile::new(fd, file_name);
        initial_print(&mut sf, 10);
        sf.update_cursor();
        files.insert(wd, sf);
    }

    if follow_opt {
        let mut buffer = [0u8; 4096];
        let mut prev_mod_type: Option<ModificationType> = None;
        loop {
            let events = watcher.read_events_blocking(&mut buffer)
                .expect("Failed to read inotify events");

            for event in events {
                // println!("{:?}", event);
                if event.mask.intersects(EventMask::CLOSE_WRITE | EventMask::MODIFY) {
                    let sf = files.get_mut(&event.wd).unwrap();
                    prev_mod_type = Some(follow(sf, event.mask, &prev_mod_type));
                }
            }
        }
    }
}

fn follow(sf: &mut StatefulFile, event: EventMask, prev_mod_type: &Option<ModificationType>) -> ModificationType {
    let mod_type = sf.modification_type();
    match mod_type {
        ModificationType::Added => {
            // println!("Added")
        }
        ModificationType::Removed => {
            // println!("Removed");
            if event.contains(EventMask::MODIFY) {
                // println!("Modify");
                return mod_type;
            }
            // println!("Close");
            sf.reset_cursor();
        }
        ModificationType::NoChange => {
            // println!("No Change");
            if event.contains(EventMask::MODIFY) {
                // println!("Modify");
                return mod_type;
            }
            // println!("Close");
            match prev_mod_type {
                &Some(ref prev_mod_type) => {
                    match prev_mod_type {
                        &ModificationType::Removed => {sf.reset_cursor()}
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    sf.update_metadata();
    sf.seek_to_cursor();
    print_from_cursor(sf);
    sf.update_cursor();
    return mod_type;
}

fn initial_print(sf: &mut StatefulFile, num_lines: usize) {
    let mut last_n_lines = VecDeque::new();
    for line in sf.fd.by_ref().lines().map(|l| l.unwrap()) {
        last_n_lines.push_front(line);
        if last_n_lines.len() > num_lines {
            last_n_lines.pop_back();
        }
    }
    while !last_n_lines.is_empty() {
        println!("{}", last_n_lines.pop_back().unwrap());
    }
}

fn print_from_cursor(sf: &mut StatefulFile) {
    for line in sf.fd.by_ref().lines().map(|l| l.unwrap()) {
        println!("{}", line);
    }
}
