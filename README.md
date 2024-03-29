# filestatrec - record mtime and mode for files in a git repository

The git tree format, as used by git-annex, records only a minimum amount
of per-file metadata. When recording more metadata is a requirement, it
has to be recorded out of band; filestatrec records it in a hidden text
file (.filestat). Currently it records the file's mode and modification
time.

The file format is designed to make it easy to manually resolve merge
conflicts, which is why it's a simple text file. Since merge drivers
usually treat each line of text as an indivisible unit, the metadata for
each file is stored in a single line. To minimize accidental conflicts,
the file is sorted by the file path; to make it more readable during
merges, unusual characters are escaped.

## Why not tags?

Both git and git-annex are content-addressed filesystems, so two files
with the same content map to the same object, even if their metadata
should be different.

## License

Licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
