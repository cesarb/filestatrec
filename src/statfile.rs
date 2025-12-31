use rustix::fs::{AtFlags, CWD, Mode, RawMode, Timespec, Timestamps, chmodat, utimensat};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::error;
use std::fs::{File, Metadata, read, rename};
use std::io::{BufWriter, Error, ErrorKind, Result, Write};
use std::os::unix::fs::MetadataExt;
use std::str;

pub type StatFile<'a> = BTreeMap<Cow<'a, [u8]>, Cow<'a, [u8]>>;
pub type StatFileEntry<'a> = (Cow<'a, [u8]>, Cow<'a, [u8]>);

pub const STATFILE: &str = ".filestat";

pub fn read_stat_file(filename: &str, create: bool) -> Result<Vec<u8>> {
    match read(filename) {
        Err(ref err) if create && err.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        result => result,
    }
}

pub fn parse_stat_file(data: &[u8]) -> Result<StatFile<'_>> {
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
            file.write_all(b"\n")?;
        }
        file.into_inner()?.sync_all()?;
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

fn extract_name(line: &[u8]) -> Result<StatFileEntry<'_>> {
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
            _ => {}
        }
    }
    Ok(apply)
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct StatApply {
    mode: Option<RawMode>,
    mtime: Option<Timespec>,
}

impl StatApply {
    pub fn new() -> Self {
        StatApply::default()
    }

    pub fn set_mode(&mut self, data: &[u8]) -> Result<()> {
        let data = str::from_utf8(data).map_err(invalid_data)?;
        self.mode = Some(RawMode::from_str_radix(data, 8).map_err(invalid_data)?);
        Ok(())
    }

    #[allow(clippy::similar_names)]
    pub fn set_mtime(&mut self, data: &[u8]) -> Result<()> {
        let data = str::from_utf8(data).map_err(invalid_data)?;
        let mut iter = data.split('.');
        let sec = iter
            .next()
            .map(str::parse)
            .transpose()
            .map_err(invalid_data)?;
        let (tv_sec, tv_nsec) = match (sec, iter.next(), iter.next()) {
            (Some(sec), None, _) => (sec, 0),
            (Some(sec), Some(nsec), None) if nsec.len() == 9 => {
                (sec, str::parse(nsec).map_err(invalid_data)?)
            }
            _ => return Err(invalid_data(format!("invalid mtime {data}"))),
        };
        self.mtime = Some(Timespec { tv_sec, tv_nsec });
        Ok(())
    }

    fn is_link(&self) -> bool {
        self.mode
            .is_some_and(|mode| (mode & 0o170_000) == 0o120_000)
    }

    pub fn apply(&self, name: &[u8], follow: bool) -> Result<()> {
        if name
            .split(|&b| b == b'/')
            .any(|c| c.is_empty() || c == b"..")
        {
            return Err(invalid_data("invalid path"));
        }

        let follow = follow && !self.is_link();
        let flags = if follow {
            AtFlags::empty()
        } else {
            AtFlags::SYMLINK_NOFOLLOW
        };

        if let Some(mode) = self.mode {
            if follow {
                chmodat(CWD, name, Mode::from_bits_truncate(mode & 0o777), flags)?;
            }
        }

        if let Some(mtime) = self.mtime {
            let times = Timestamps {
                last_access: mtime,
                last_modification: mtime,
            };
            utimensat(CWD, name, &times, flags)?;
        }

        Ok(())
    }
}

const HEXDIGIT: &[u8] = b"0123456789abcdef";

fn escape(name: &[u8]) -> Cow<'_, [u8]> {
    let escape_high = str::from_utf8(name).is_err();
    let escape_byte = |c: u8| c.is_ascii_control() || c == b'\\' || escape_high && c >= 0x80;
    let count = name.iter().filter(|&&c| escape_byte(c)).count();
    debug_assert!(count > 0 || !escape_high);

    if count > 0 {
        let mut buf = Vec::with_capacity(name.len() + count * 3);

        for &c in name {
            match c {
                b'\\' => buf.extend_from_slice(b"\\\\"),
                c if escape_byte(c) => buf.extend_from_slice(&[
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

fn unescape(name: &[u8]) -> Result<Cow<'_, [u8]>> {
    Ok(if name.contains(&b'\\') {
        let mut buf = Vec::with_capacity(name.len());

        let mut iter = name.iter();
        while let Some(&c) = iter.next() {
            buf.push(if c == b'\\' {
                match iter.next() {
                    Some(&b'\\') => b'\\',
                    Some(&b'x') => match (iter.next(), iter.next()) {
                        (Some(&hi), Some(&lo)) => {
                            let (hi, lo) = (char::from(hi), char::from(lo));
                            match (hi.to_digit(16), lo.to_digit(16)) {
                                #[allow(clippy::cast_possible_truncation)]
                                (Some(hi), Some(lo)) => (hi * 16 + lo) as u8,
                                _ => {
                                    return Err(invalid_data(format!(
                                        "invalid hexadecimal escape \\x{hi}{lo}"
                                    )));
                                }
                            }
                        }
                        _ => return Err(invalid_data("truncated hexadecimal escape")),
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

#[cfg(test)]
mod tests {
    #[test]
    fn escape_backslash() {
        test_escape(b"a\\b", b"a\\\\b")
    }

    #[test]
    fn escape_control() {
        test_escape(b"a\x7fb", b"a\\x7fb")
    }

    #[test]
    fn escape_tab() {
        test_escape(b"a\tb", b"a\\x09b")
    }

    #[test]
    fn escape_newline() {
        test_escape(b"a\nb", b"a\\x0ab")
    }

    #[test]
    fn escape_latin1() {
        test_escape(b"codifica\xe7\xe3o", b"codifica\\xe7\\xe3o")
    }

    #[test]
    fn escape_utf8() {
        test_escape("codificação".as_bytes(), "codificação".as_bytes())
    }

    #[test]
    fn escape_control_latin1() {
        test_escape(b"codi\x7ffica\xe7\xe3o", b"codi\\x7ffica\\xe7\\xe3o")
    }

    #[test]
    fn escape_control_utf8() {
        test_escape("codi\x7fficação".as_bytes(), "codi\\x7fficação".as_bytes())
    }

    #[test]
    fn escape_spaces() {
        let name = b"long name with spaces.txt";
        test_escape(name, name)
    }

    fn test_escape(data: &[u8], escaped: &[u8]) {
        use super::{escape, unescape};

        assert_eq!(escape(data), escaped);
        assert_eq!(unescape(escaped).unwrap(), data);
    }

    #[test]
    fn escape_roundtrip() {
        use super::{escape, unescape};

        let name: Vec<u8> = (0..=255).collect();
        assert_eq!(name, &*unescape(&escape(&name)).unwrap());
    }

    #[test]
    fn parse_line() {
        use super::{StatApply, Timespec, parse_line};

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
                mtime: Some(Timespec {
                    tv_sec: 4321,
                    tv_nsec: 123456789
                })
            }
        );
        assert_eq!(
            parse_line(b"name\tfoo\tbar=\tbaz=0\tmode=100644\tmtime=4321.123456789").unwrap(),
            StatApply {
                mode: Some(0o100644),
                mtime: Some(Timespec {
                    tv_sec: 4321,
                    tv_nsec: 123456789
                })
            }
        );
    }

    #[test]
    fn invalid_path() {
        test_invalid_path(b"/root", false);
        test_invalid_path(b"/root", true);

        test_invalid_path(b"../dir", false);
        test_invalid_path(b"../dir", true);

        test_invalid_path(b"a/../b", false);
        test_invalid_path(b"a/../b", true);

        test_invalid_path(b"dir/..", false);
        test_invalid_path(b"dir/..", true);
    }

    fn test_invalid_path(name: &[u8], follow: bool) {
        use super::StatApply;
        use std::io::ErrorKind;

        let error = StatApply::new().apply(name, follow).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidData);
    }
}
