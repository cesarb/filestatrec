mod statfile;

use clap::{
    app_from_crate, crate_authors, crate_description, crate_name, crate_version, AppSettings, Arg,
    ArgMatches, SubCommand,
};
use std::collections::btree_map::Entry;
use std::fs::{metadata, symlink_metadata};
use std::io::Result;
use std::os::unix::ffi::OsStrExt;

use crate::statfile::{
    make_line, parse_line, parse_stat_file, read_stat_file, write_stat_file, STATFILE,
};

fn main() -> Result<()> {
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

    match matches.subcommand() {
        ("add", Some(matches)) => add(matches),
        ("apply", Some(matches)) => apply(matches),
        _ => unreachable!(),
    }
}

fn add(arg_matches: &ArgMatches) -> Result<()> {
    let follow = !arg_matches.is_present("no-follow");
    let force = arg_matches.is_present("force");

    let stat_file = read_stat_file(STATFILE, true)?;
    let mut stat_file = parse_stat_file(&stat_file)?;

    for name in arg_matches.values_of_os("file").unwrap() {
        let metadata = if follow {
            metadata(name)
        } else {
            symlink_metadata(name)
        }?;

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

    write_stat_file(STATFILE, &stat_file)
}

fn apply(arg_matches: &ArgMatches) -> Result<()> {
    let follow = !arg_matches.is_present("no-follow");
    let files = arg_matches
        .values_of_os("file")
        .map(|values| values.map(|name| name.as_bytes()));

    let stat_file = read_stat_file(STATFILE, false)?;
    let stat_file = parse_stat_file(&stat_file)?;

    match files {
        None => {
            for (name, line) in stat_file {
                // TODO ignore not found errors
                parse_line(&line)?.apply(&name, follow)?;
            }
        }
        Some(files) => {
            for name in files {
                let line = stat_file.get(name);
                if let Some(line) = line {
                    parse_line(&line)?.apply(&name, follow)?;
                }
            }
        }
    }

    Ok(())
}
