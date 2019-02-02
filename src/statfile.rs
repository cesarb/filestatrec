use nix;
use nix::sys::stat::{fchmodat, utimensat, FchmodatFlags, Mode, UtimensatFlags};
use nix::sys::time::{TimeSpec, TimeValLike};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::error;
use std::ffi::CString;
use std::fs::{read, rename, File, Metadata};
use std::io::{BufWriter, Error, ErrorKind, Result, Write};
use std::os::unix::fs::MetadataExt;
use std::str;

pub type StatFile<'a> = BTreeMap<Cow<'a, [u8]>, Cow<'a, [u8]>>;
pub type StatFileEntry<'a> = (Cow<'a, [u8]>, Cow<'a, [u8]>);

pub const STATFILE: &str = ".filestat";

pub fn read_stat_file(filename: &str) -> Result<Vec<u8>> {
    match read(filename) {
        Err(ref err) if err.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        result => result,
    }
}

pub fn parse_stat_file(data: &[u8]) -> Result<StatFile> {
    data.split(|&b| b == b'\n')
        .filter(|s| !s.is_empty())
        .map(extract_name)
        .collect()
}

pub fn write_stat_file(filename: &str, data: &StatFile) -> Result<()> {
    let tmp = filename.to_owned() + ".tmp";
    {
        let mut file = BufWriter::new(File::create(&tmp)?);
        for entry in data {
            file.write_all(entry.1)?;
            file.write_all(&[b'\n'])?;
        }
    }
    rename(tmp, filename)
}

pub fn make_line(name: &[u8], metadata: &Metadata) -> Vec<u8> {
    let mut line = escape(name).into_owned();
    line.append(
        &mut format!(
            "\tmode={:03o}\tmtime={}.{:09}",
            metadata.mode(),
            metadata.mtime(),
            metadata.mtime_nsec(),
        )
        .into_bytes(),
    );
    line
}

fn extract_name(line: &[u8]) -> Result<StatFileEntry> {
    let name = line.split(|&b| b == b'\t').next().unwrap();
    Ok((unescape(name)?, line.into()))
}

pub fn parse_line(line: &[u8]) -> Result<StatApply> {
    let mut apply = StatApply::new();
    for item in line.split(|&b| b == b'\t').skip(1) {
        match item.split_at(
            item.iter()
                .position(|&b| b == b'=')
                .map_or(item.len(), |p| p + 1),
        ) {
            (b"mode=", data) => apply.set_mode(data)?,
            (b"mtime=", data) => apply.set_mtime(data)?,
            (_, _) => {}
        }
    }
    Ok(apply)
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct StatApply {
    mode: Option<u32>,
    mtime: Option<(i64, i64)>,
}

impl StatApply {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn set_mode(&mut self, data: &[u8]) -> Result<()> {
        self.mode = Some(
            u32::from_str_radix(str::from_utf8(data).map_err(invalid_data)?, 8)
                .map_err(invalid_data)?,
        );
        Ok(())
    }

    pub fn set_mtime(&mut self, data: &[u8]) -> Result<()> {
        let data = str::from_utf8(data).map_err(invalid_data)?;
        let mut iter = data.split('.');
        self.mtime = Some(match (iter.next(), iter.next(), iter.next()) {
            (Some(sec), None, _) => (str::parse(sec).map_err(invalid_data)?, 0),
            (Some(sec), Some(nsec), None) if nsec.len() == 9 => (
                str::parse(sec).map_err(invalid_data)?,
                str::parse(nsec).map_err(invalid_data)?,
            ),
            (_, _, _) => return Err(invalid_data(format!("invalid mtime {}", data))),
        });
        Ok(())
    }

    fn is_link(&self) -> bool {
        self.mode
            .map_or(false, |mode| (mode & 0o170_000) == 0o120_000)
    }

    pub fn apply(&self, name: &[u8], follow: bool) -> Result<()> {
        let name = CString::new(name)?;
        let follow = follow && !self.is_link();

        // TODO check for .. in name and/or and link target

        if let Some(mode) = self.mode {
            if follow {
                fchmodat(
                    None,
                    &*name,
                    Mode::from_bits_truncate(mode & 0o777),
                    FchmodatFlags::FollowSymlink,
                )
                .map_err(nix_error)?;
            }
        }

        if let Some(mtime) = self.mtime {
            let mtime = TimeSpec::nanoseconds(mtime.0 * 1_000_000_000 + mtime.1);
            let flags = if follow {
                UtimensatFlags::FollowSymlink
            } else {
                UtimensatFlags::NoFollowSymlink
            };
            utimensat(None, &*name, &mtime, &mtime, flags).map_err(nix_error)?;
        }

        Ok(())
    }
}

const HEXDIGIT: &[u8] = b"0123456789abcdef";

fn escape(name: &[u8]) -> Cow<[u8]> {
    let escape = name.iter().any(|&c| c < 0x20) || std::str::from_utf8(name).is_err();
    let count = if escape {
        name.iter().filter(|&&c| c < 0x20 || c >= 0x7f).count()
    } else {
        0
    };

    if count > 0 {
        let mut buf = Vec::with_capacity(name.len() + count * 3);

        for &c in name {
            match c {
                b'\\' => buf.extend_from_slice(&[b'\\', b'\\']),
                c if c < 0x20 || c >= 0x7f => buf.extend_from_slice(&[
                    b'\\',
                    b'x',
                    HEXDIGIT[(c / 16) as usize],
                    HEXDIGIT[(c % 16) as usize],
                ]),
                c => buf.push(c),
            }
        }

        buf.shrink_to_fit();
        buf.into()
    } else {
        name.into()
    }
}

fn unescape(name: &[u8]) -> Result<Cow<[u8]>> {
    Ok(if name.contains(&b'\\') {
        let mut buf = Vec::with_capacity(name.len());

        let mut iter = name.into_iter();
        while let Some(&c) = iter.next() {
            buf.push(if c == b'\\' {
                match iter.next() {
                    Some(&b'\\') => b'\\',
                    Some(&b'x') => match (iter.next(), iter.next()) {
                        (Some(&hi), Some(&lo)) => {
                            let (hi, lo) = (char::from(hi), char::from(lo));
                            match (hi.to_digit(16), lo.to_digit(16)) {
                                (Some(hi), Some(lo)) => (hi * 16 + lo) as u8,
                                (_, _) => {
                                    return Err(invalid_data(format!(
                                        "invalid hexadecimal escape \\x{}{}",
                                        hi, lo
                                    )));
                                }
                            }
                        }
                        (_, _) => return Err(invalid_data("truncated hexadecimal escape")),
                    },
                    Some(&c) => {
                        return Err(invalid_data(format!(
                            "unknown escape character \\{}",
                            char::from(c)
                        )));
                    }
                    None => return Err(invalid_data("unterminated escape character")),
                }
            } else {
                c
            });
        }

        buf.shrink_to_fit();
        buf.into()
    } else {
        name.into()
    })
}

fn invalid_data<E>(error: E) -> Error
where
    E: Into<Box<dyn error::Error + Send + Sync>>,
{
    Error::new(ErrorKind::InvalidData, error)
}

fn nix_error(error: nix::Error) -> Error {
    match error {
        nix::Error::Sys(errno) => errno.into(),
        error => Error::new(ErrorKind::Other, error),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn escape_roundtrip() {
        use super::{escape, unescape};

        let name: Vec<u8> = (0..=255).collect();
        assert_eq!(*name, *unescape(&escape(&name)).unwrap());
    }

    #[test]
    fn parse_line() {
        use super::{parse_line, StatApply};

        assert_eq!(
            parse_line(b"name").unwrap(),
            StatApply {
                mode: None,
                mtime: None
            }
        );
        assert_eq!(
            parse_line(b"name\tmode=100644\tmtime=4321.123456789").unwrap(),
            StatApply {
                mode: Some(0o100644),
                mtime: Some((4321, 123456789))
            }
        );
        assert_eq!(
            parse_line(b"name\tfoo\tbar=\tbaz=0\tmode=100644\tmtime=4321.123456789").unwrap(),
            StatApply {
                mode: Some(0o100644),
                mtime: Some((4321, 123456789))
            }
        );
    }
}
