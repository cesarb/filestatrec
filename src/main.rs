use crate::error::{with_error_path, ErrorWithPath};
use crate::statfile::{
    make_line, parse_line, parse_stat_file, read_stat_file, write_stat_file, STATFILE,
};
use clap::builder::ValueParser;
use clap::{arg, command, ArgMatches, Command};
use std::collections::btree_map::Entry;
use std::fs::{metadata, symlink_metadata};
use std::io::{self, ErrorKind};
use std::os::unix::ffi::OsStrExt;
use std::process::ExitCode;

mod error;
mod statfile;

fn cmd() -> Command {
    command!().subcommand_required(true).subcommands([
        Command::new("add").args([
            arg!(file: <FILE> ...).value_parser(ValueParser::os_string()),
            arg!(--"follow").overrides_with("no-follow"),
            arg!(--"no-follow").overrides_with("follow"),
            arg!(-f --force),
        ]),
        Command::new("apply").args([
            arg!(file: [FILE] ...).value_parser(ValueParser::os_string()),
            arg!(--"follow").overrides_with("no-follow"),
            arg!(--"no-follow").overrides_with("follow"),
        ]),
    ])
}

#[test]
fn verify_cmd() {
    cmd().debug_assert();
}

fn main() -> ExitCode {
    let result = match cmd().get_matches().subcommand() {
        Some(("add", matches)) => add(matches),
        Some(("apply", matches)) => apply(matches),
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

fn add(matches: &ArgMatches) -> Result<ExitCode, ErrorWithPath<io::Error>> {
    let follow = !matches.get_flag("no-follow");
    let force = matches.get_flag("force");

    let stat_file = with_error_path(STATFILE, || read_stat_file(STATFILE, true))?;
    let mut stat_file = with_error_path(STATFILE, || parse_stat_file(&stat_file))?;

    for name in matches.get_raw("file").unwrap() {
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

fn apply(matches: &ArgMatches) -> Result<ExitCode, ErrorWithPath<io::Error>> {
    let follow = !matches.get_flag("no-follow");
    let files = matches
        .get_raw("file")
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
