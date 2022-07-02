use crate::error::{with_error_path, ErrorWithPath};
use crate::statfile::{
    make_line, parse_line, parse_stat_file, read_stat_file, write_stat_file, STATFILE,
};
use clap::{
    app_from_crate, crate_authors, crate_description, crate_name, crate_version, AppSettings, Arg,
    ArgMatches, SubCommand,
};
use std::collections::btree_map::Entry;
use std::fs::{metadata, symlink_metadata};
use std::io::{self, ErrorKind};
use std::os::unix::ffi::OsStrExt;
use std::process::exit;

mod error;
mod statfile;

#[allow(clippy::upper_case_acronyms)]
enum ExitCode {
    SUCCESS,
    FAILURE,
}

impl ExitCode {
    #[inline]
    fn code(&self) -> i32 {
        match self {
            ExitCode::SUCCESS => 0,
            ExitCode::FAILURE => 1,
        }
    }
}

fn main() {
    exit(run().code())
}

fn run() -> ExitCode {
    let matches = app_from_crate!()
        .setting(AppSettings::SubcommandRequired)
        .setting(AppSettings::VersionlessSubcommands)
        .subcommand(
            SubCommand::with_name("add")
                .arg(Arg::with_name("file").multiple(true).required(true))
                .arg(
                    Arg::with_name("follow")
                        .long("follow")
                        .overrides_with("no-follow"),
                )
                .arg(
                    Arg::with_name("no-follow")
                        .long("no-follow")
                        .overrides_with("follow"),
                )
                .arg(Arg::with_name("force").long("force").short("f")),
        )
        .subcommand(
            SubCommand::with_name("apply")
                .arg(Arg::with_name("file").multiple(true).required(false))
                .arg(
                    Arg::with_name("follow")
                        .long("follow")
                        .overrides_with("no-follow"),
                )
                .arg(
                    Arg::with_name("no-follow")
                        .long("no-follow")
                        .overrides_with("follow"),
                ),
        )
        .get_matches();

    let result = match matches.subcommand() {
        ("add", Some(matches)) => add(matches),
        ("apply", Some(matches)) => apply(matches),
        _ => unreachable!(),
    };

    match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{}", err);
            ExitCode::FAILURE
        }
    }
}

fn add(arg_matches: &ArgMatches) -> Result<ExitCode, ErrorWithPath<io::Error>> {
    let follow = !arg_matches.is_present("no-follow");
    let force = arg_matches.is_present("force");

    let stat_file = with_error_path(STATFILE, || read_stat_file(STATFILE, true))?;
    let mut stat_file = with_error_path(STATFILE, || parse_stat_file(&stat_file))?;

    for name in arg_matches.values_of_os("file").unwrap() {
        let metadata = with_error_path(name.as_bytes(), || {
            if follow {
                metadata(name)
            } else {
                symlink_metadata(name)
            }
        })?;

        let name = name.as_bytes();
        let line = make_line(name, &metadata);

        match stat_file.entry(name.into()) {
            Entry::Vacant(entry) => {
                entry.insert(line.into());
            }
            Entry::Occupied(mut entry) => {
                if force {
                    entry.insert(line.into());
                }
            }
        }
    }

    with_error_path(STATFILE, || write_stat_file(STATFILE, &stat_file))?;
    Ok(ExitCode::SUCCESS)
}

fn apply(arg_matches: &ArgMatches) -> Result<ExitCode, ErrorWithPath<io::Error>> {
    let follow = !arg_matches.is_present("no-follow");
    let files = arg_matches
        .values_of_os("file")
        .map(|values| values.map(|name| name.as_bytes()));

    let stat_file = with_error_path(STATFILE, || read_stat_file(STATFILE, false))?;
    let stat_file = with_error_path(STATFILE, || parse_stat_file(&stat_file))?;

    let mut result = ExitCode::SUCCESS;
    let mut error = |err| {
        eprintln!("{}", err);
        result = ExitCode::FAILURE;
    };

    match files {
        None => {
            for (name, line) in stat_file {
                with_error_path(name.as_ref(), || parse_line(&line)?.apply(&name, follow))
                    .unwrap_or_else(&mut error);
            }
        }
        Some(files) => {
            for name in files {
                with_error_path(name, || {
                    if let Some(line) = stat_file.get(name) {
                        parse_line(line)?.apply(name, follow)
                    } else {
                        Err(io::Error::new(
                            ErrorKind::InvalidInput,
                            "Not found in stat file",
                        ))
                    }
                })
                .unwrap_or_else(&mut error);
            }
        }
    }

    Ok(result)
}
